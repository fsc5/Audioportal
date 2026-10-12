//! WASAPI capture; COM interfaces are created and used exclusively on this worker thread.
use super::selected_device;
use crate::audio::{
    AudioBlock, BlockQueue, CaptureWorker, WorkerHandle, CAPTURE_QUEUE_BLOCKS, FRAMES_PER_BLOCK,
    SAMPLES_PER_BLOCK,
};
use crate::contracts::{ApiError, ErrorCode};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use wasapi::{DeviceState, Direction, SampleType, StreamMode, WaveFormat};

const BYTES_PER_FRAME: usize = 8; // requested interleaved stereo f32

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
            .map_err(|err| failed(format!("cannot initialize COM for capture: {err}")))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

/// Opens and starts the selected recording endpoint before returning. The caller owns
/// cancellation and must join the worker even after receiving an asynchronous error.
pub fn start_capture(id: &str) -> Result<CaptureWorker, ApiError> {
    let blocks = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
    let input_peak = Arc::new(AtomicU32::new(0f32.to_bits()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let (errors_tx, errors) = mpsc::sync_channel(1);
    let id = id.to_owned();
    let thread_blocks = blocks.clone();
    let thread_peak = input_peak.clone();
    let thread_cancelled = cancelled.clone();
    let thread = thread::Builder::new()
        .name("audioportal-capture".into())
        .spawn(move || {
            let mut started = false;
            let result = run_capture(
                &id,
                &thread_blocks,
                &thread_peak,
                &thread_cancelled,
                &startup_tx,
                &mut started,
            );
            if !started {
                let _ = startup_tx.send(result.clone());
            }
            if let Err(err) = &result {
                let _ = errors_tx.try_send(err.clone());
            }
            result
        })
        .map_err(|err| failed(format!("cannot spawn capture worker: {err}")))?;

    match startup_rx.recv() {
        Ok(Ok(())) => Ok(CaptureWorker {
            worker: WorkerHandle {
                cancelled,
                errors,
                thread,
            },
            blocks,
            input_peak,
        }),
        outcome => {
            cancelled.store(true, Ordering::Release);
            let joined = thread
                .join()
                .unwrap_or_else(|_| Err(failed("capture worker panicked")));
            Err(match outcome {
                Ok(Err(err)) => err,
                _ => joined
                    .err()
                    .unwrap_or_else(|| failed("capture worker exited before startup")),
            })
        }
    }
}

fn run_capture(
    id: &str,
    blocks: &BlockQueue,
    input_peak: &AtomicU32,
    cancelled: &AtomicBool,
    startup: &SyncSender<Result<(), ApiError>>,
    started: &mut bool,
) -> Result<(), ApiError> {
    let _com = ComApartment::initialize()?;
    let device = selected_device(id, Direction::Capture).map_err(|err| ApiError {
        code: ErrorCode::DeviceUnavailable,
        message: err.to_string(),
    })?;
    let mut client = device.get_iaudioclient().map_err(|err| {
        failed(format!(
            "cannot open capture endpoint {id:?}: {err}; reconnect it"
        ))
    })?;
    let (period, _) = client.get_device_period().map_err(|err| {
        failed(format!(
            "cannot query capture endpoint {id:?} period: {err}"
        ))
    })?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
    client
        .initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: period,
            },
        )
        .map_err(|err| {
            failed(format!(
                "cannot initialize 48 kHz stereo capture on {id:?}: {err}; check its format"
            ))
        })?;
    let event = client
        .set_get_eventhandle()
        .map_err(|err| failed(format!("cannot set capture event for {id:?}: {err}")))?;
    let reader = client
        .get_audiocaptureclient()
        .map_err(|err| failed(format!("cannot get capture reader for {id:?}: {err}")))?;
    let buffer_len = (client
        .get_buffer_size()
        .map_err(|err| failed(format!("cannot get capture buffer size for {id:?}: {err}")))?
        as usize)
        .checked_mul(BYTES_PER_FRAME)
        .ok_or_else(|| failed("capture buffer size overflow"))?;
    let mut bytes = vec![0; buffer_len];
    let mut assembler = BlockAssembler::default();

    client.start_stream().map_err(|err| {
        failed(format!(
            "cannot start capture on {id:?}: {err}; reconnect it"
        ))
    })?;
    let result = (|| {
        startup
            .send(Ok(()))
            .map_err(|_| failed("capture startup abandoned"))?;
        *started = true;
        while !cancelled.load(Ordering::Acquire) {
            match event.wait_for_event(200) {
                Ok(()) | Err(wasapi::WasapiError::EventTimeout) => {}
                Err(err) => {
                    return Err(failed(format!(
                        "capture event failed on {id:?}: {err}; reconnect it"
                    )));
                }
            }
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let state = device.get_state().map_err(|err| {
                failed(format!("capture endpoint {id:?} became unavailable: {err}"))
            })?;
            if state != DeviceState::Active {
                return Err(failed(format!(
                    "capture endpoint {id:?} is {state}; reconnect it"
                )));
            }
            let mut peak = 0f32;
            while !cancelled.load(Ordering::Acquire) {
                let Some(frames) = reader.get_next_packet_size().map_err(|err| {
                    failed(format!("capture failed on {id:?}: {err}; reconnect it"))
                })?
                else {
                    break;
                };
                if frames == 0 {
                    break;
                }
                let len = frames as usize * BYTES_PER_FRAME;
                if len > bytes.len() {
                    return Err(failed("capture packet exceeds WASAPI buffer size"));
                }
                let (read, info) = reader.read_from_device(&mut bytes[..len]).map_err(|err| {
                    failed(format!(
                        "capture read failed on {id:?}: {err}; reconnect it"
                    ))
                })?;
                if read == 0 || read > frames {
                    return Err(failed("capture returned an invalid frame count"));
                }
                if info.flags.timestamp_error {
                    return Err(failed(format!("capture timestamp invalid on {id:?}")));
                }
                peak = peak.max(
                    assembler
                        .push(
                            info.index,
                            read as usize,
                            &bytes[..read as usize * BYTES_PER_FRAME],
                            info.flags.silent,
                            info.flags.data_discontinuity,
                            blocks,
                        )
                        .map_err(failed)?,
                );
            }
            input_peak.store(peak.to_bits(), Ordering::Relaxed);
        }
        Ok(())
    })();
    let stop = client
        .stop_stream()
        .map_err(|err| failed(format!("cannot stop capture on {id:?}: {err}")));
    result.and(stop)
}

struct BlockAssembler {
    origin: Option<u64>,
    last_end: u64,
    next_frame: u64,
    filled: usize,
    samples: [i16; SAMPLES_PER_BLOCK],
}

impl Default for BlockAssembler {
    fn default() -> Self {
        Self {
            origin: None,
            last_end: 0,
            next_frame: 0,
            filled: 0,
            samples: [0; SAMPLES_PER_BLOCK],
        }
    }
}

impl BlockAssembler {
    // Device position is in requested 48 kHz frames. Discard incomplete blocks
    // across gaps, and begin again at the next aligned wire-block boundary.
    fn push(
        &mut self,
        position: u64,
        frames: usize,
        bytes: &[u8],
        silent: bool,
        discontinuity: bool,
        blocks: &BlockQueue,
    ) -> Result<f32, &'static str> {
        if frames.checked_mul(BYTES_PER_FRAME) != Some(bytes.len()) {
            return Err("capture packet has an invalid byte count");
        }
        let first = self.origin.is_none();
        let origin = *self.origin.get_or_insert(position);
        let start = position
            .checked_sub(origin)
            .ok_or("capture timeline moved before start")?;
        if start < self.last_end {
            return Err("capture timeline regressed");
        }
        if start > self.last_end || (discontinuity && !first) {
            // When the flag has no position jump, the missing duration is unknown.
            // Do not stitch the incomplete block across the glitch.
            self.filled = 0;
            self.next_frame = start
                .checked_add(
                    (FRAMES_PER_BLOCK as u64 - start % FRAMES_PER_BLOCK as u64)
                        % FRAMES_PER_BLOCK as u64,
                )
                .ok_or("capture timeline overflow")?;
        }
        let end = start
            .checked_add(frames as u64)
            .ok_or("capture timeline overflow")?;
        self.last_end = end;
        let mut peak = 0f32;
        for frame in 0..frames {
            let pair = if silent {
                [0., 0.]
            } else {
                let offset = frame * BYTES_PER_FRAME;
                [
                    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
                    f32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()),
                ]
            };
            for (channel, sample) in pair.into_iter().enumerate() {
                let finite = if sample.is_finite() { sample } else { 0. };
                peak = peak.max(finite.abs().min(1.));
                if start + (frame as u64) >= self.next_frame {
                    self.samples[self.filled * 2 + channel] =
                        (finite.clamp(-1., 1.) * 32768.) as i16;
                }
            }
            if start + (frame as u64) < self.next_frame {
                continue;
            }
            self.filled += 1;
            self.next_frame += 1;
            if self.filled == FRAMES_PER_BLOCK {
                let samples = std::mem::replace(&mut self.samples, [0; SAMPLES_PER_BLOCK]);
                blocks.push(AudioBlock {
                    frame_index: self.next_frame - FRAMES_PER_BLOCK as u64,
                    samples,
                });
                self.filled = 0;
            }
        }
        Ok(peak)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(left: f32, right: f32, frames: usize) -> Vec<u8> {
        (0..frames)
            .flat_map(|_| left.to_le_bytes().into_iter().chain(right.to_le_bytes()))
            .collect()
    }

    #[test]
    fn assembles_variable_packets_and_converts_channels_and_silence() {
        let blocks = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let mut assembler = BlockAssembler::default();
        assert_eq!(
            assembler.push(100, 137, &stereo(0.5, -1., 137), false, false, &blocks),
            Ok(1.)
        );
        assert_eq!(
            assembler.push(237, 103, &stereo(0., 0., 103), true, false, &blocks),
            Ok(0.)
        );
        assert_eq!(
            assembler.push(340, 240, &stereo(-0.25, 1., 240), false, false, &blocks),
            Ok(1.)
        );
        let first = blocks.pop().unwrap();
        assert_eq!(first.frame_index, 0);
        assert_eq!(&first.samples[..2], &[16384, -32768]);
        assert_eq!(&first.samples[274..276], &[0, 0]);
        let second = blocks.pop().unwrap();
        assert_eq!(second.frame_index, 240);
        assert_eq!(&second.samples[..2], &[-8192, 32767]);
    }

    #[test]
    fn gaps_discard_partials_and_overflow_keeps_recent_indices() {
        let blocks = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let mut assembler = BlockAssembler::default();
        assembler
            .push(0, 300, &stereo(0.5, 0.5, 300), false, false, &blocks)
            .unwrap();
        assembler
            .push(350, 50, &stereo(0.25, 0.25, 50), false, true, &blocks)
            .unwrap();
        assembler
            .push(400, 80, &stereo(0.25, 0.25, 80), false, false, &blocks)
            .unwrap();
        assembler
            .push(
                480,
                240 * 22,
                &stereo(0.25, 0.25, 240 * 22),
                false,
                false,
                &blocks,
            )
            .unwrap();
        assert_eq!(blocks.len(), CAPTURE_QUEUE_BLOCKS);
        assert_eq!(blocks.pop().unwrap().frame_index, 4 * 240);
        assert_eq!(blocks.pop().unwrap().frame_index, 5 * 240);
    }

    #[test]
    fn unmeasured_discontinuity_drops_partial_without_stopping_capture() {
        let blocks = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let mut assembler = BlockAssembler::default();
        assembler
            .push(0, 300, &stereo(0.5, 0.5, 300), false, false, &blocks)
            .unwrap();
        assembler
            .push(300, 180, &stereo(0.25, 0.25, 180), false, true, &blocks)
            .unwrap();
        assembler
            .push(480, 240, &stereo(0.25, 0.25, 240), false, false, &blocks)
            .unwrap();
        assert_eq!(blocks.pop().unwrap().frame_index, 0);
        assert_eq!(blocks.pop().unwrap().frame_index, 480);
        assert!(assembler
            .push(700, 10, &stereo(0., 0., 10), true, false, &blocks)
            .is_err());
    }

    #[test]
    fn peak_and_pcm_remain_bounded_for_bad_float_samples() {
        let blocks = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let mut assembler = BlockAssembler::default();
        assert_eq!(
            assembler.push(0, 240, &stereo(f32::NAN, -2., 240), false, false, &blocks),
            Ok(1.)
        );
        assert_eq!(blocks.pop().unwrap().samples[..2], [0, -32768]);
    }
}
