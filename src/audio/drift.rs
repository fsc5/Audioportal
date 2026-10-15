//! Receive-clock feedback. Ratios are input frames consumed per nominal output
//! frame: above 1.0 drains buffered input faster; below 1.0 drains it slower.
use super::{PlaybackMetrics, FRAMES_PER_BLOCK};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const WIRE_RATE: f64 = 48_000.; // input frames/second
const FILTER_SECONDS: f64 = 2.; // occupancy low-pass time constant
const GAIN: f64 = 0.0002 / FRAMES_PER_BLOCK as f64; // ratio / input frame
const LIMIT: f64 = 0.001; // +/- 1000 ppm, within ConsumptionRatio's +/- 5000 ppm
const SLEW_PER_SECOND: f64 = 0.002; // at most 20 ppm per 10 ms update

/// Filtered proportional feedback, with no integral windup after a stall.
pub struct DriftController {
    target_frames: f64,
    filtered_frames: Option<f64>,
    ratio: f64,
    last: Option<Instant>,
}

impl DriftController {
    pub fn new(target_frames: usize) -> Self {
        Self {
            target_frames: target_frames as f64,
            filtered_frames: None,
            ratio: 1.,
            last: None,
        }
    }

    pub fn reset(&mut self) {
        self.filtered_frames = None;
        self.ratio = 1.;
        self.last = None;
    }

    /// A measurement every ~10 ms; jitter from a single packet is attenuated
    /// over two seconds. Large scheduling gaps reinitialize the filter.
    pub fn update(&mut self, frames: f64, now: Instant) -> f32 {
        if !frames.is_finite() || frames < 0. {
            return self.ratio as f32;
        }
        let elapsed = self.last.map(|last| now.saturating_duration_since(last));
        self.last = Some(now);
        let seconds = elapsed.map_or(0., |dt| dt.as_secs_f64());
        if seconds > 1. {
            self.filtered_frames = None;
        }
        let previous = self.filtered_frames.unwrap_or(frames);
        let filtered = previous + (frames - previous) * (1. - (-seconds / FILTER_SECONDS).exp());
        self.filtered_frames = Some(filtered);
        let desired = 1. + ((filtered - self.target_frames) * GAIN).clamp(-LIMIT, LIMIT);
        let max_step = SLEW_PER_SECOND * seconds.min(1.);
        self.ratio += (desired - self.ratio).clamp(-max_step, max_step);
        self.ratio as f32
    }

    pub fn ratio(&self) -> f32 {
        self.ratio as f32
    }
}

/// Playback reports independently sampled atomics. These are an estimate, not
/// a device presentation timestamp. Convert output-rate frames to 48 kHz input
/// frames; the nominal device-rate conversion is independent of clock feedback.
pub fn occupancy_frames(
    jitter_blocks: usize,
    queue_blocks: usize,
    playback: &PlaybackMetrics,
) -> Option<f64> {
    let rate = playback.output_rate.load(Ordering::Relaxed);
    if rate == 0 {
        return None;
    }
    let input = ((jitter_blocks + queue_blocks) * FRAMES_PER_BLOCK) as f64
        + playback.pending_input_frames.load(Ordering::Relaxed) as f64;
    let output = (playback.pending_output_frames.load(Ordering::Relaxed) as f64
        + playback.resampler_delay_frames.load(Ordering::Relaxed) as f64
        + playback.padding_frames.load(Ordering::Relaxed) as f64)
        * WIRE_RATE
        / rate as f64;
    Some(input + output)
}

pub fn frames_duration(frames: f64) -> Duration {
    Duration::from_secs_f64(frames / WIRE_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn occupancy_includes_partial_converter_and_device_at_native_rate() {
        let playback = PlaybackMetrics {
            output_rate: AtomicU32::new(44_100),
            pending_input_frames: AtomicU32::new(120),
            pending_output_frames: AtomicU32::new(220),
            resampler_delay_frames: AtomicU32::new(44),
            padding_frames: AtomicU32::new(441),
            ..PlaybackMetrics::default()
        };
        let expected = 4. * 240. + 120. + 705. * 48_000. / 44_100.;
        assert_eq!(occupancy_frames(3, 1, &playback), Some(expected));
        assert_eq!(frames_duration(480.), Duration::from_millis(10));
        playback.output_rate.store(0, Ordering::Relaxed);
        assert_eq!(occupancy_frames(3, 1, &playback), None);
    }

    #[test]
    fn filtered_bounded_controller_clears_history_on_restart() {
        let now = Instant::now();
        let mut control = DriftController::new(1_440);
        for tick in 0..500 {
            control.update(1_680., now + Duration::from_millis(tick * 10));
        }
        assert!((1.0001..=1.001).contains(&control.ratio()));
        control.reset();
        assert_eq!(control.ratio(), 1.);
        control.update(1_200., now + Duration::from_secs(10));
        assert_eq!(control.ratio(), 1.); // first sample seeds the filter
        control.update(
            1_200.,
            now + Duration::from_secs(10) + Duration::from_millis(10),
        );
        assert!(control.ratio() < 1.);
    }
}
