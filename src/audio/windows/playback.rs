//! Selected-endpoint WASAPI playback. All COM objects stay on the owning thread.
use super::selected_device;
use crate::audio::playback::PlaybackBuffer;
use crate::audio::{
    BlockQueue, ConsumptionRatio, PlaybackMetrics, PlaybackWorker, WorkerHandle,
    PLAYBACK_QUEUE_BLOCKS,
};
use crate::contracts::{ApiError, ErrorCode};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use wasapi::{DeviceState, Direction, SampleType, StreamMode, WaveFormat};

fn failed(message: impl Into<String>) -> ApiError {
    ApiError {
        code: ErrorCode::WorkerFailed,
        message: message.into(),
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, ApiError> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|err| failed(format!("cannot initialize COM for playback: {err}")))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

/// Returns only after stream startup. Cancel and join even after an error notification.
pub fn start_playback(id: &str) -> Result<PlaybackWorker, ApiError> {
    let blocks = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
    let output_peak = Arc::new(AtomicU32::new(0f32.to_bits()));
    let ratio = ConsumptionRatio::default();
    let metrics = Arc::new(PlaybackMetrics::default());
    metrics
        .applied_ratio
        .store(1f32.to_bits(), Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let (errors_tx, errors) = mpsc::sync_channel(1);
    let id = id.to_owned();
    let thread_blocks = blocks.clone();
    let thread_peak = output_peak.clone();
    let thread_ratio = ratio.clone();
    let thread_metrics = metrics.clone();
    let thread_cancelled = cancelled.clone();
    let thread = thread::Builder::new()
        .name("audioportal-playback".into())
        .spawn(move || {
            let result = run_playback(
                &id,
                &thread_blocks,
                &thread_peak,
                &thread_ratio,
                &thread_metrics,
                &thread_cancelled,
                &startup_tx,
            );
            // Startup channel has capacity one; after startup this notification is irrelevant.
            if let Err(err) = &result {
                let _ = startup_tx.try_send(Err(err.clone()));
                let _ = errors_tx.try_send(err.clone());
            }
            thread_peak.store(0f32.to_bits(), Ordering::Relaxed);
            for _ in 0..PLAYBACK_QUEUE_BLOCKS {
                thread_blocks.pop();
            }
            thread_metrics
                .pending_input_frames
                .store(0, Ordering::Relaxed);
            thread_metrics
                .pending_output_frames
                .store(0, Ordering::Relaxed);
            thread_metrics.padding_frames.store(0, Ordering::Relaxed);
            result
        })
        .map_err(|err| failed(format!("cannot spawn playback worker: {err}")))?;
    match startup_rx.recv() {
        Ok(Ok(())) => Ok(PlaybackWorker {
            worker: WorkerHandle {
                cancelled,
                errors,
                thread,
            },
            blocks,
            output_peak,
            ratio,
            metrics,
        }),
        outcome => {
            cancelled.store(true, Ordering::Release);
            let joined = thread
                .join()
                .unwrap_or_else(|_| Err(failed("playback worker panicked")));
            Err(match outcome {
                Ok(Err(err)) => err,
                _ => joined
                    .err()
                    .unwrap_or_else(|| failed("playback worker exited before startup")),
            })
        }
    }
}

fn run_playback(
    id: &str,
    blocks: &BlockQueue,
    peak: &AtomicU32,
    ratio: &ConsumptionRatio,
    metrics: &PlaybackMetrics,
    cancelled: &AtomicBool,
    startup: &SyncSender<Result<(), ApiError>>,
) -> Result<(), ApiError> {
    let _com = ComApartment::initialize()?;
    let device = selected_device(id, Direction::Render).map_err(|err| ApiError {
        code: ErrorCode::DeviceUnavailable,
        message: err.to_string(),
    })?;
    let context = |err| {
        failed(format!(
            "playback endpoint {id:?}: {err}; reconnect it or check its format"
        ))
    };
    let mut client = device.get_iaudioclient().map_err(context)?;
    let rate = client.get_mixformat().map_err(context)?.get_samplespersec();
    let mut converter = PlaybackBuffer::new(rate).map_err(failed)?;
    let (period, _) = client.get_device_period().map_err(context)?;
    // Software resampling uses the mix rate; WASAPI handles float/stereo conversion
    // to the endpoint's channel layout and sample representation in shared mode.
    let format = WaveFormat::new(32, 32, &SampleType::Float, rate as usize, 2, None);
    client
        .initialize_client(
            &format,
            &Direction::Render,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: period,
            },
        )
        .map_err(context)?;
    let event = client.set_get_eventhandle().map_err(context)?;
    let writer = client.get_audiorenderclient().map_err(context)?;
    let capacity = client.get_buffer_size().map_err(context)? as usize;
    if capacity == 0 || capacity > rate as usize {
        return Err(failed(
            "playback endpoint buffer must contain between one frame and one second",
        ));
    }
    let mut bytes = vec![0; capacity * 8];
    writer
        .write_to_device(capacity, &bytes, None)
        .map_err(context)?;
    metrics.output_rate.store(rate, Ordering::Relaxed);
    metrics
        .submitted_frames
        .store(capacity as u64, Ordering::Relaxed);
    metrics
        .padding_frames
        .store(capacity as u32, Ordering::Relaxed);
    client.start_stream().map_err(context)?;
    let result = (|| {
        startup
            .send(Ok(()))
            .map_err(|_| failed("playback startup abandoned"))?;
        while !cancelled.load(Ordering::Acquire) {
            match event.wait_for_event(200) {
                Ok(()) | Err(wasapi::WasapiError::EventTimeout) => {}
                Err(err) => return Err(context(err)),
            }
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let state = device.get_state().map_err(context)?;
            if state != DeviceState::Active {
                return Err(failed(format!(
                    "playback endpoint {id:?} is {state}; reconnect it"
                )));
            }
            let padding = client.get_current_padding().map_err(context)? as usize;
            if padding > capacity {
                return Err(failed("invalid playback padding reported by WASAPI"));
            }
            metrics
                .padding_frames
                .store(padding as u32, Ordering::Relaxed);
            let frames = capacity - padding;
            if frames == 0 {
                continue;
            }
            let buffer = &mut bytes[..frames * 8];
            let output_peak = converter
                .fill(buffer, blocks, ratio, metrics)
                .map_err(failed)?;
            writer
                .write_to_device(frames, buffer, None)
                .map_err(context)?;
            peak.store(output_peak.to_bits(), Ordering::Relaxed);
            metrics
                .submitted_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
            metrics
                .padding_frames
                .store(capacity as u32, Ordering::Relaxed);
        }
        Ok(())
    })();
    let stop = client.stop_stream().map_err(context);
    result.and(stop)
}
