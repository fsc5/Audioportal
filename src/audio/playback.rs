//! Bounded playback conversion, kept platform-independent for regression checks.
use super::{AudioBlock, BlockQueue, ConsumptionRatio, PlaybackMetrics, FRAMES_PER_BLOCK};
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::sync::atomic::Ordering;

#[derive(Default)]
struct BlockReader {
    current: Option<AudioBlock>,
    offset: usize,
}

impl BlockReader {
    fn remaining(&self) -> usize {
        self.current
            .as_ref()
            .map_or(0, |_| FRAMES_PER_BLOCK - self.offset)
    }

    fn fill(&mut self, blocks: &BlockQueue, output: &mut [f32]) -> u64 {
        let mut frames = output.as_chunks_mut::<2>().0.iter_mut();
        while let Some(frame) = frames.next() {
            if self.current.is_none() {
                self.current = blocks.pop();
                self.offset = 0;
            }
            if let Some(block) = &self.current {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = block.samples[self.offset * 2 + channel] as f32 / 32768.;
                }
                self.offset += 1;
                if self.offset == FRAMES_PER_BLOCK {
                    self.current = None;
                }
            } else {
                frame.fill(0.);
                let missing = 1 + frames.len() as u64;
                for frame in frames {
                    frame.fill(0.);
                }
                return missing;
            }
        }
        0
    }
}

pub(super) struct PlaybackBuffer {
    reader: BlockReader,
    resampler: Async<f32>,
    input: Vec<f32>,
    output: Vec<f32>,
    offset: usize,
    rate: u32,
    applied: f64,
}

impl PlaybackBuffer {
    pub(super) fn new(rate: u32) -> Result<Self, String> {
        // Bound memory/work even if an endpoint reports an implausible mix rate.
        if !(8_000..=192_000).contains(&rate) {
            return Err(format!(
                "unsupported playback rate {rate}; expected 8000–192000 Hz"
            ));
        }
        let chunk = (rate as usize / 200).max(1); // at most 5 ms of output
        let params = SincInterpolationParameters::new(128, WindowFunction::Blackman2)
            .oversampling_factor(256)
            .interpolation(SincInterpolationType::Linear);
        let resampler = Async::new_sinc(
            rate as f64 / 48_000.,
            1.01,
            &params,
            chunk,
            2,
            FixedAsync::Output,
        )
        .map_err(|err| err.to_string())?;
        let input = vec![0.; resampler.input_frames_max() * 2];
        Ok(Self {
            reader: BlockReader::default(),
            resampler,
            input,
            output: vec![0.; chunk * 2],
            offset: chunk * 2,
            rate,
            applied: 1.,
        })
    }

    /// Fill interleaved stereo f32 bytes without allocating. Rubato linearly ramps
    /// inverse resampling ratio, i.e. precisely the input-consumption units we expose.
    pub(super) fn fill(
        &mut self,
        bytes: &mut [u8],
        blocks: &BlockQueue,
        ratio: &ConsumptionRatio,
        metrics: &PlaybackMetrics,
    ) -> Result<f32, String> {
        if !bytes.len().is_multiple_of(8) {
            return Err("playback buffer is not whole stereo frames".into());
        }
        let mut peak = 0f32;
        for frame in bytes.as_chunks_mut::<8>().0 {
            if self.offset == self.output.len() {
                let step = 0.0001 * (self.output.len() / 2) as f64 / (self.rate as f64 * 0.005);
                self.applied += (ratio.get() as f64 - self.applied).clamp(-step, step);
                self.resampler
                    .set_resample_ratio_relative(1. / self.applied, true)
                    .map_err(|err| err.to_string())?;
                let frames = self.resampler.input_frames_next();
                let missing = self.reader.fill(blocks, &mut self.input[..frames * 2]);
                metrics
                    .missing_input_frames
                    .fetch_add(missing, Ordering::Relaxed);
                let input = InterleavedSlice::new(&self.input[..frames * 2], 2, frames)
                    .map_err(|err| err.to_string())?;
                let output_frames = self.output.len() / 2;
                let mut output = InterleavedSlice::new_mut(&mut self.output, 2, output_frames)
                    .map_err(|err| err.to_string())?;
                self.resampler
                    .process_into_buffer(&input, &mut output, None)
                    .map_err(|err| err.to_string())?;
                self.offset = 0;
            }
            for (channel, dst) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let sample = self.output[self.offset + channel];
                let sample = if sample.is_finite() {
                    sample.clamp(-1., 1.)
                } else {
                    0.
                };
                peak = peak.max(sample.abs());
                dst.copy_from_slice(&sample.to_le_bytes());
            }
            self.offset += 2;
        }
        metrics
            .pending_input_frames
            .store(self.reader.remaining() as u32, Ordering::Relaxed);
        metrics.pending_output_frames.store(
            ((self.output.len() - self.offset) / 2) as u32,
            Ordering::Relaxed,
        );
        metrics
            .resampler_delay_frames
            .store(self.resampler.output_delay() as u32, Ordering::Relaxed);
        metrics
            .applied_ratio
            .store((self.applied as f32).to_bits(), Ordering::Relaxed);
        Ok(peak)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{PLAYBACK_QUEUE_BLOCKS, SAMPLES_PER_BLOCK};

    fn block() -> AudioBlock {
        let mut samples = [0; SAMPLES_PER_BLOCK];
        for (index, pair) in samples.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            pair.copy_from_slice(&[index as i16 + 1, -(index as i16 + 1)]);
        }
        AudioBlock {
            frame_index: 0,
            samples,
        }
    }

    #[test]
    fn partial_blocks_keep_order_then_zero_fill_and_fresh_reader_is_empty() {
        let queue = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        queue.push(block());
        let mut reader = BlockReader::default();
        let mut first = [0.; 137 * 2];
        assert_eq!(reader.fill(&queue, &mut first), 0);
        assert_eq!(reader.remaining(), 103);
        let mut rest = [1.; 120 * 2];
        assert_eq!(reader.fill(&queue, &mut rest), 17);
        let samples: Vec<_> = first
            .into_iter()
            .chain(rest[..206].iter().copied())
            .collect();
        for (i, pair) in samples.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(pair, &[(i + 1) as f32 / 32768., -((i + 1) as f32) / 32768.]);
        }
        assert!(rest[206..].iter().all(|&v| v == 0.));
        assert_eq!(
            BlockReader::default().fill(&BlockQueue::new(4), &mut rest),
            120
        );
        assert!(rest.iter().all(|&v| v == 0.));
    }

    #[test]
    fn conversion_ramps_consumption_at_multiple_rates_and_drains_to_silence() {
        for rate in [44_100, 48_000, 96_000] {
            let queue = BlockQueue::new(4);
            let metrics = PlaybackMetrics::default();
            let ratio = ConsumptionRatio::default();
            let mut buffer = PlaybackBuffer::new(rate).unwrap();
            let frames = rate as usize / 200;
            let mut bytes = vec![0; frames * 8];
            for target in [1.005, 0.995] {
                ratio.set(target).unwrap();
                let mut consumed = 0;
                for chunk in 0..110 {
                    while queue.len() < 4 {
                        queue.push(block());
                    }
                    let available = queue.len() * FRAMES_PER_BLOCK + buffer.reader.remaining();
                    let before = buffer.applied;
                    let peak = buffer.fill(&mut bytes, &queue, &ratio, &metrics).unwrap();
                    if chunk >= 100 {
                        consumed +=
                            available - queue.len() * FRAMES_PER_BLOCK - buffer.reader.remaining();
                    }
                    assert!((buffer.applied - before).abs() <= 0.0001 + 1e-12);
                    assert!((0. ..=1.).contains(&peak));
                    for pair in bytes.as_chunks::<8>().0 {
                        let left = f32::from_le_bytes(pair[..4].try_into().unwrap());
                        let right = f32::from_le_bytes(pair[4..].try_into().unwrap());
                        assert!((left + right).abs() < 1e-6);
                    }
                }
                assert!((buffer.applied - target as f64).abs() < 1e-7);
                let expected = frames as f64 * 10. * 48_000. / rate as f64 * target as f64;
                assert!(
                    (consumed as f64 - expected).abs() < 2.,
                    "{rate}: consumed {consumed}, expected {expected}"
                );
            }
            assert_eq!(metrics.missing_input_frames.load(Ordering::Relaxed), 0);
            let mut peak = 1.;
            for _ in 0..20 {
                peak = buffer.fill(&mut bytes, &queue, &ratio, &metrics).unwrap();
            }
            assert_eq!(peak, 0.);
            assert!(metrics.missing_input_frames.load(Ordering::Relaxed) > 0);
            assert_eq!(metrics.pending_output_frames.load(Ordering::Relaxed), 0);
            assert_eq!(metrics.pending_input_frames.load(Ordering::Relaxed), 0);
            let mut fresh = PlaybackBuffer::new(rate).unwrap();
            assert_eq!(
                fresh
                    .fill(
                        &mut bytes,
                        &BlockQueue::new(4),
                        &ConsumptionRatio::default(),
                        &PlaybackMetrics::default()
                    )
                    .unwrap(),
                0.
            );
        }
    }

    #[test]
    fn callback_sizes_do_not_change_samples_or_accounting() {
        let make_queue = || {
            let queue = BlockQueue::new(4);
            for _ in 0..4 {
                queue.push(block());
            }
            queue
        };
        for rate in [44_100, 48_000] {
            let ratio = ConsumptionRatio::default();
            ratio.set(1.005).unwrap();
            let one_metrics = PlaybackMetrics::default();
            let split_metrics = PlaybackMetrics::default();
            let mut one = PlaybackBuffer::new(rate).unwrap();
            let mut split = PlaybackBuffer::new(rate).unwrap();
            let mut expected = vec![0; 1500 * 8];
            one.fill(&mut expected, &make_queue(), &ratio, &one_metrics)
                .unwrap();
            let queue = make_queue();
            let mut actual = Vec::new();
            for frames in [1, 137, 509, 3, 850] {
                let mut bytes = vec![0; frames * 8];
                split
                    .fill(&mut bytes, &queue, &ratio, &split_metrics)
                    .unwrap();
                actual.extend(bytes);
            }
            assert_eq!(actual, expected);
            assert_eq!(
                split_metrics.missing_input_frames.load(Ordering::Relaxed),
                one_metrics.missing_input_frames.load(Ordering::Relaxed)
            );
            assert_eq!(
                split_metrics.pending_output_frames.load(Ordering::Relaxed),
                one_metrics.pending_output_frames.load(Ordering::Relaxed)
            );
            assert!(PlaybackBuffer::new(0).is_err());
            assert!(PlaybackBuffer::new(192_001).is_err());
        }
    }

    #[test]
    fn native_rate_conversion_preserves_tone_frequency_and_stereo_channels() {
        for rate in [44_100, 48_000, 96_000] {
            let mut converter = PlaybackBuffer::new(rate).unwrap();
            let queue = BlockQueue::new(4);
            let ratio = ConsumptionRatio::default();
            let metrics = PlaybackMetrics::default();
            let mut index = 0;
            let mut last = [0f32; 2];
            let mut crossings = [0i32; 2];
            let mut bytes = vec![0; (rate as usize / 200) * 8];
            for _ in 0..200 {
                while queue.len() < 4 {
                    let mut samples = [0; SAMPLES_PER_BLOCK];
                    for frame in samples.as_chunks_mut::<2>().0 {
                        for (ch, sample) in frame.iter_mut().enumerate() {
                            *sample = ((std::f64::consts::TAU * [440., 880.][ch] * index as f64
                                / 48_000.)
                                .sin()
                                * 8192.) as i16;
                        }
                        index += 1;
                    }
                    queue.push(AudioBlock {
                        frame_index: index - 240,
                        samples,
                    });
                }
                converter
                    .fill(&mut bytes, &queue, &ratio, &metrics)
                    .unwrap();
                for frame in bytes.as_chunks::<8>().0 {
                    for ch in 0..2 {
                        let sample =
                            f32::from_le_bytes(frame[ch * 4..ch * 4 + 4].try_into().unwrap());
                        // Ignore tiny startup/filter ringing around silence.
                        if sample.abs() > 0.01 {
                            if last[ch] <= 0. && sample > 0. {
                                crossings[ch] += 1;
                            }
                            last[ch] = sample;
                        }
                    }
                }
            }
            assert!((crossings[0] - 440).abs() <= 4, "{rate}: {crossings:?}");
            assert!((crossings[1] - 880).abs() <= 4, "{rate}: {crossings:?}");
        }
    }
}
