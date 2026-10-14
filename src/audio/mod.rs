//! Shared, platform-independent audio values. Windows endpoints stay in `windows`.
use crate::contracts::ApiError;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const FRAMES_PER_BLOCK: usize = 240;
pub const SAMPLES_PER_BLOCK: usize = FRAMES_PER_BLOCK * 2;
pub const CAPTURE_QUEUE_BLOCKS: usize = 20; // 100 ms
pub const PLAYBACK_QUEUE_BLOCKS: usize = 4; // 20 ms beyond the jitter target
pub const JITTER_MAX_PACKETS: usize = 40; // 200 ms, independent of frame-index gaps

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioBlock {
    pub frame_index: u64,
    pub samples: [i16; SAMPLES_PER_BLOCK],
}

/// Memory-only FIFO; on overflow, evicts the oldest block and retains the latest.
/// Both operations hold a short mutex; never perform I/O while holding it.
#[derive(Clone, Debug)]
pub struct BlockQueue {
    inner: Arc<Mutex<VecDeque<AudioBlock>>>,
    capacity: usize,
}

impl BlockQueue {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "block queue needs a positive capacity");
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    /// Returns true if an older block was evicted. Frame indices are never renumbered.
    pub fn push(&self, block: AudioBlock) -> bool {
        let mut queue = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let dropped = queue.len() == self.capacity;
        if dropped {
            queue.pop_front();
        }
        queue.push_back(block);
        dropped
    }

    pub fn pop(&self) -> Option<AudioBlock> {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop_front()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Task implementations publish startup failures before returning a handle; later
/// errors are sent through a sync_channel(1) with try_send, and returned by join.
/// The error receiver is polled by session code, never awaited on an audio thread.
pub struct WorkerHandle {
    pub cancelled: Arc<AtomicBool>,
    pub errors: Receiver<ApiError>,
    pub thread: JoinHandle<Result<(), ApiError>>,
}

impl WorkerHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn join(self) -> Result<(), ApiError> {
        self.thread.join().unwrap_or_else(|_| {
            Err(ApiError {
                code: crate::contracts::ErrorCode::WorkerFailed,
                message: "worker panicked".into(),
            })
        })
    }
}

pub struct CaptureWorker {
    pub worker: WorkerHandle,
    pub blocks: BlockQueue,
    pub input_peak: Arc<AtomicU32>, // f32 bits; finite, clamped to 0..=1
}

pub struct PlaybackWorker {
    pub worker: WorkerHandle,
    pub blocks: BlockQueue,
    pub output_peak: Arc<AtomicU32>, // f32 bits; finite, clamped to 0..=1
    pub ratio: ConsumptionRatio,
    pub metrics: Arc<PlaybackMetrics>,
}

/// Independently sampled counters, not a coherent clock snapshot. All frame counts
/// are stream-rate output frames except `missing_input_frames` (48 kHz) and
/// `pending_input_frames` (unconsumed portion of a wire block).
#[derive(Default)]
pub struct PlaybackMetrics {
    pub output_rate: AtomicU32,
    pub submitted_frames: AtomicU64,
    pub padding_frames: AtomicU32,
    pub missing_input_frames: AtomicU64,
    pub pending_input_frames: AtomicU32,
    pub pending_output_frames: AtomicU32,
    pub resampler_delay_frames: AtomicU32,
    pub applied_ratio: AtomicU32, // f32 bits; ramp endpoint of the latest converted chunk
}

/// Input frames consumed per nominal output frame. 1.0 is uncorrected.
/// The playback worker reads this atomically and smoothly ramps changes.
#[derive(Clone, Debug)]
pub struct ConsumptionRatio(Arc<AtomicU32>);

impl Default for ConsumptionRatio {
    fn default() -> Self {
        Self(Arc::new(AtomicU32::new(1.0f32.to_bits())))
    }
}

impl ConsumptionRatio {
    pub fn set(&self, ratio: f32) -> Result<(), &'static str> {
        if !ratio.is_finite() || !(0.995..=1.005).contains(&ratio) {
            return Err("consumption ratio must be finite and between 0.995 and 1.005");
        }
        self.0.store(ratio.to_bits(), Ordering::Relaxed);
        Ok(())
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

#[cfg(windows)]
pub mod windows;

#[cfg(any(windows, test))]
mod playback;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_evicts_oldest_without_rewriting_timeline() {
        let queue = BlockQueue::new(2);
        for frame_index in [0, 240, 480] {
            let dropped = queue.push(AudioBlock {
                frame_index,
                samples: [0; SAMPLES_PER_BLOCK],
            });
            assert_eq!(dropped, frame_index == 480);
        }
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop().unwrap().frame_index, 240);
        assert_eq!(queue.pop().unwrap().frame_index, 480);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn ratio_rejects_nonfinite_and_out_of_range_values() {
        let ratio = ConsumptionRatio::default();
        assert!(ratio.set(f32::NAN).is_err());
        assert!(ratio.set(1.006).is_err());
        assert_eq!(ratio.get(), 1.0);
        ratio.set(0.999).unwrap();
        assert_eq!(ratio.get(), 0.999);
    }
}
