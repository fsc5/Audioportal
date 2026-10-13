//! Bounded playout driven by caller-supplied monotonic time.
//!
//! Transport validates packets and peer/session identity before insertion, checks
//! its idle timeout before polling, and resets this buffer on session expiry.
use crate::audio::{AudioBlock, FRAMES_PER_BLOCK, JITTER_MAX_PACKETS, SAMPLES_PER_BLOCK};
use crate::contracts::{ApiError, ErrorCode};
use std::time::{Duration, Instant};

const BLOCK_TIME: Duration = Duration::from_millis(5);
const MAX_BACKLOG: Duration = Duration::from_millis(20);

struct Timeline {
    started: Instant,
    first_frame: u64,
    // Wide arithmetic prevents peer timestamps or elapsed time wrapping the wire
    // frame index. Once u64 is exhausted, only reset can establish a new timeline.
    next_slot: u128,
}

pub struct JitterBuffer {
    target: Duration,
    timeline: Option<Timeline>,
    blocks: [Option<AudioBlock>; JITTER_MAX_PACKETS],
    local_skipped: u64,
}

impl JitterBuffer {
    pub fn new(buffer_ms: u16) -> Result<Self, ApiError> {
        if !(10..=100).contains(&buffer_ms) || !buffer_ms.is_multiple_of(5) {
            return Err(ApiError {
                code: ErrorCode::InvalidSettings,
                message: "receive buffer must be 10–100 ms in 5 ms steps".into(),
            });
        }
        Ok(Self {
            target: Duration::from_millis(u64::from(buffer_ms)),
            timeline: None,
            blocks: std::array::from_fn(|_| None),
            local_skipped: 0,
        })
    }

    /// Accepts aligned, on-time audio within 40 slots of the next unplayed block.
    /// A block may arrive exactly at its deadline if that slot has not been polled;
    /// arrivals after their deadline are late even if the caller has not polled yet.
    pub fn insert(&mut self, block: AudioBlock, now: Instant) -> bool {
        if !block.frame_index.is_multiple_of(FRAMES_PER_BLOCK as u64) {
            return false;
        }
        let timeline = self.timeline.get_or_insert(Timeline {
            started: now,
            first_frame: block.frame_index,
            next_slot: 0,
        });
        let Some(offset) = block.frame_index.checked_sub(timeline.first_frame) else {
            return false;
        };
        let slot = u128::from(offset / FRAMES_PER_BLOCK as u64);
        if slot < timeline.next_slot || slot - timeline.next_slot >= JITTER_MAX_PACKETS as u128 {
            return false;
        }
        let deadline = self.target.as_nanos() + slot * BLOCK_TIME.as_nanos();
        if now.saturating_duration_since(timeline.started).as_nanos() > deadline {
            return false;
        }
        let index =
            (block.frame_index / FRAMES_PER_BLOCK as u64 % JITTER_MAX_PACKETS as u64) as usize;
        // Every slot in the forward window has a distinct index. A full window
        // admits only duplicates or out-of-window blocks, never a 41st block.
        if self.blocks[index].is_some() {
            return false;
        }
        self.blocks[index] = Some(block);
        true
    }

    /// Returns one due block and whether missing network audio required silence.
    /// Poll again at the same `now` until None; at most five blocks can be due.
    /// Only true loss flags contribute to the caller's `packets_lost` counter.
    pub fn poll(&mut self, now: Instant) -> Option<(AudioBlock, bool)> {
        let timeline = self.timeline.as_mut()?;
        let elapsed = now
            .checked_duration_since(timeline.started)?
            .checked_sub(self.target)?;
        let latest_slot = elapsed.as_nanos() / BLOCK_TIME.as_nanos();
        if timeline.next_slot > latest_slot {
            return None;
        }
        if elapsed.as_nanos() - timeline.next_slot * BLOCK_TIME.as_nanos() > MAX_BACKLOG.as_nanos()
        {
            let skipped = latest_slot - timeline.next_slot;
            self.local_skipped = self
                .local_skipped
                .saturating_add(u64::try_from(skipped).unwrap_or(u64::MAX));
            timeline.next_slot = latest_slot;
            let next_frame =
                u128::from(timeline.first_frame) + timeline.next_slot * FRAMES_PER_BLOCK as u128;
            // Always visits 40 slots, regardless of the size of the scheduling gap.
            for block in &mut self.blocks {
                if block
                    .as_ref()
                    .is_some_and(|b| u128::from(b.frame_index) < next_frame)
                {
                    *block = None;
                }
            }
        }
        let frame_index = u64::try_from(
            u128::from(timeline.first_frame) + timeline.next_slot * FRAMES_PER_BLOCK as u128,
        )
        .ok()?;
        timeline.next_slot += 1;
        let index = (frame_index / FRAMES_PER_BLOCK as u64 % JITTER_MAX_PACKETS as u64) as usize;
        Some(match self.blocks[index].take() {
            Some(block) => (block, false),
            None => (
                AudioBlock {
                    frame_index,
                    samples: [0; SAMPLES_PER_BLOCK],
                },
                true,
            ),
        })
    }

    /// Number of stored audio blocks, including prefill and out-of-order audio.
    /// Holes, emitted silence, and downstream playback/device buffers are excluded.
    /// Obsolete stored blocks remain counted until poll discards them.
    pub fn occupancy(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    /// Deadline slots bypassed during local catch-up, including any holes.
    /// This saturating counter excludes network loss and downstream queue eviction.
    pub fn local_skipped_blocks(&self) -> u64 {
        self.local_skipped
    }

    /// Discards audio, schedule, and local skip count; retains the configured target.
    /// The next accepted block starts a fresh prefill interval.
    pub fn reset(&mut self) {
        self.blocks.fill(None);
        self.timeline = None;
        self.local_skipped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{BlockQueue, PLAYBACK_QUEUE_BLOCKS};
    use crate::protocol::{decode, encode, Packet};

    fn audio(slot: u64) -> AudioBlock {
        // Exercise the real codec, including sequence wrap while u64 frames advance.
        decode(&encode(&Packet {
            session_id: 1,
            sequence: slot as u32,
            block: AudioBlock {
                frame_index: slot * FRAMES_PER_BLOCK as u64,
                samples: [123; SAMPLES_PER_BLOCK],
            },
        }))
        .unwrap()
        .block
    }

    #[test]
    fn synthetic_timeline_reorders_across_sequence_wrap_and_resets() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut jitter = JitterBuffer::new(30).unwrap();
        let base = u64::from(u32::MAX) - 1;
        assert_eq!(jitter.poll(start), None);
        assert!(jitter.insert(audio(base), start));
        assert!(jitter.insert(audio(base + 2), at(2)));
        assert!(jitter.insert(audio(base + 1), at(3)));
        assert!(!jitter.insert(audio(base + 2), at(4)));
        assert!(!jitter.insert(audio(base - 1), at(4)));
        assert_eq!(jitter.occupancy(), 3);
        assert_eq!(jitter.poll(at(30) - Duration::from_nanos(1)), None);
        for (offset, time) in [(0, 30), (1, 35), (2, 40)] {
            assert_eq!(jitter.poll(at(time)), Some((audio(base + offset), false)));
            assert_eq!(jitter.poll(at(time)), None);
        }
        let (silence, lost) = jitter.poll(at(45)).unwrap();
        assert!(lost);
        assert_eq!(silence.frame_index, (base + 3) * FRAMES_PER_BLOCK as u64);
        assert_eq!(silence.samples, [0; SAMPLES_PER_BLOCK]);
        assert!(!jitter.insert(audio(base + 3), at(46)));
        assert!(jitter.insert(audio(base + 4), at(50)));
        assert_eq!(jitter.poll(at(50)), Some((audio(base + 4), false)));
        assert!(jitter.insert(audio(base + 6), at(51)));
        // An expired deadline cannot be rescued just because poll was delayed.
        assert!(!jitter.insert(audio(base + 5), at(56)));
        assert_eq!(jitter.local_skipped_blocks(), 0);
        jitter.reset();
        assert_eq!(jitter.occupancy(), 0);
        assert_eq!(jitter.poll(at(100)), None);
        assert!(jitter.insert(audio(0), at(100)));
        assert_eq!(jitter.poll(at(129)), None);
        assert_eq!(jitter.poll(at(130)), Some((audio(0), false)));
    }

    #[test]
    fn full_window_rejects_far_future_and_reuses_storage() {
        let start = Instant::now();
        let mut jitter = JitterBuffer::new(100).unwrap();
        assert!(jitter.insert(audio(0), start));
        for slot in (1..40).rev() {
            assert!(jitter.insert(audio(slot), start));
        }
        assert_eq!(jitter.occupancy(), JITTER_MAX_PACKETS);
        for slot in [0, 39, 40, u64::MAX / FRAMES_PER_BLOCK as u64] {
            assert!(!jitter.insert(audio(slot), start));
            assert_eq!(jitter.occupancy(), JITTER_MAX_PACKETS);
        }
        // Exercise several revolutions of the fixed storage, retaining all audio.
        for slot in 0..120 {
            let now = start + Duration::from_millis(100 + slot * 5);
            assert_eq!(jitter.poll(now), Some((audio(slot), false)));
            assert_eq!(jitter.occupancy(), JITTER_MAX_PACKETS - 1);
            assert!(jitter.insert(audio(slot + 40), now));
            assert_eq!(jitter.occupancy(), JITTER_MAX_PACKETS);
        }
        assert_eq!(jitter.local_skipped_blocks(), 0);
    }

    #[test]
    fn backlog_boundary_and_playback_overflow_do_not_report_network_loss() {
        let start = Instant::now();
        for extra_ns in [0, 1] {
            let mut jitter = JitterBuffer::new(10).unwrap();
            for slot in 0..8 {
                assert!(jitter.insert(audio(slot), start));
            }
            let queue = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
            let now = start + Duration::from_millis(30) + Duration::from_nanos(extra_ns);
            let (mut emitted, mut lost, mut evicted) = (0, 0, 0);
            while let Some((block, silence)) = jitter.poll(now) {
                emitted += 1;
                assert!(emitted <= 5);
                lost += usize::from(silence);
                evicted += usize::from(queue.push(block));
                assert!(queue.len() <= PLAYBACK_QUEUE_BLOCKS);
            }
            assert_eq!(lost, 0);
            if extra_ns == 0 {
                assert_eq!((emitted, evicted, jitter.local_skipped_blocks()), (5, 1, 0));
                for slot in 1..5 {
                    assert_eq!(queue.pop(), Some(audio(slot)));
                }
            } else {
                assert_eq!((emitted, evicted, jitter.local_skipped_blocks()), (1, 0, 4));
                assert_eq!(queue.pop(), Some(audio(4)));
            }
            assert_eq!(jitter.occupancy(), 3);
            assert_eq!(jitter.poll(start + Duration::from_millis(34)), None);
            assert_eq!(
                jitter.poll(start + Duration::from_millis(35)),
                Some((audio(5), false))
            );
        }
    }

    #[test]
    fn long_pause_skips_directly_and_preserves_deadline_phase() {
        let start = Instant::now();
        let mut jitter = JitterBuffer::new(30).unwrap();
        for slot in 0..40 {
            assert!(jitter.insert(audio(slot), start));
        }
        // Deliberately bypass transport's one-second idle reset to stress catch-up.
        let now = start + Duration::from_millis(600_003);
        let latest = 119_994;
        let (silence, lost) = jitter.poll(now).unwrap();
        assert!(lost); // One emitted missing block, not 119,994 network losses.
        assert_eq!(silence.frame_index, latest * FRAMES_PER_BLOCK as u64);
        assert_eq!(silence.samples, [0; SAMPLES_PER_BLOCK]);
        assert_eq!(jitter.local_skipped_blocks(), latest);
        assert_eq!(jitter.occupancy(), 0);
        assert_eq!(jitter.poll(now), None);
        assert!(!jitter.insert(audio(39), now));
        assert!(jitter.insert(audio(latest + 1), now));
        assert_eq!(jitter.poll(start + Duration::from_millis(600_004)), None);
        assert_eq!(
            jitter.poll(start + Duration::from_millis(600_005)),
            Some((audio(latest + 1), false))
        );
        jitter.reset();
        assert_eq!(jitter.local_skipped_blocks(), 0);
        assert_eq!(jitter.poll(now), None);
    }

    #[test]
    fn frame_index_limit_never_wraps_or_restarts_without_reset() {
        let start = Instant::now();
        let mut jitter = JitterBuffer::new(10).unwrap();
        let last = u64::MAX / FRAMES_PER_BLOCK as u64;
        assert!(jitter.insert(audio(last - 1), start));
        assert!(jitter.insert(audio(last), start));
        assert!(!jitter.insert(audio(0), start));
        assert_eq!(
            jitter.poll(start + Duration::from_millis(10)),
            Some((audio(last - 1), false))
        );
        assert_eq!(
            jitter.poll(start + Duration::from_millis(15)),
            Some((audio(last), false))
        );
        assert_eq!(jitter.poll(start + Duration::from_millis(20)), None);
        assert_eq!(jitter.poll(start + Duration::from_secs(600)), None);
        assert!(!jitter.insert(audio(0), start + Duration::from_secs(600)));
        jitter.reset();
        assert!(jitter.insert(audio(0), start + Duration::from_secs(600)));
    }

    #[test]
    fn validates_targets_and_alignment_before_starting_prefill() {
        for target in (10..=100).step_by(5) {
            let start = Instant::now();
            let mut jitter = JitterBuffer::new(target).unwrap();
            let mut invalid = audio(0);
            invalid.frame_index = 1;
            assert!(!jitter.insert(invalid, start));
            assert_eq!(jitter.occupancy(), 0);
            assert_eq!(jitter.poll(start + Duration::from_secs(1)), None);
            assert!(jitter.insert(audio(0), start + Duration::from_secs(1)));
            let deadline =
                start + Duration::from_secs(1) + Duration::from_millis(u64::from(target));
            assert_eq!(jitter.poll(deadline - Duration::from_nanos(1)), None);
            assert_eq!(jitter.poll(deadline), Some((audio(0), false)));
        }
        for target in [0, 5, 9, 11, 99, 101, 105, u16::MAX] {
            assert!(matches!(
                JitterBuffer::new(target),
                Err(ApiError {
                    code: ErrorCode::InvalidSettings,
                    ..
                })
            ));
        }
    }
}
