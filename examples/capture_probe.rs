//! Manual Windows check for the Task 03 capture worker.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use audioportal::audio::windows::start_capture;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().collect();
    let [_, mode, id] = args.as_slice() else {
        return Err("usage: capture_probe <run|stall|restart> <capture-id>".into());
    };
    if !["run", "stall", "restart"].contains(&mode.as_str()) {
        return Err("usage: capture_probe <run|stall|restart> <capture-id>".into());
    }

    for attempt in 0..if mode == "restart" { 2 } else { 1 } {
        let capture = start_capture(id)
            .map_err(|err| std::io::Error::other(format!("{:?}: {}", err.code, err.message)))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut count, mut gaps, mut next_index) = (0u64, 0u64, None);
        let (mut left, mut right, mut peak, mut max_depth) = (0i16, 0i16, 0f32, 0usize);
        while Instant::now() < deadline {
            if capture.worker.errors.try_recv().is_ok() {
                break;
            }
            max_depth = max_depth.max(capture.blocks.len());
            if mode == "stall" {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            while let Some(block) = capture.blocks.pop() {
                if let Some(next) = next_index {
                    if block.frame_index > next {
                        gaps += block.frame_index - next;
                    }
                }
                next_index = Some(block.frame_index + 240);
                count += 240;
                for pair in block.samples.as_chunks::<2>().0 {
                    left = left.max(pair[0].saturating_abs());
                    right = right.max(pair[1].saturating_abs());
                }
            }
            peak = peak.max(f32::from_bits(capture.input_peak.load(Ordering::Relaxed)));
            thread::sleep(Duration::from_millis(5));
        }
        let queued = capture.blocks.len();
        capture.worker.cancel();
        let result = capture.worker.join();
        println!(
            "attempt {}: frames={count} gaps={gaps} queued={queued} max_queue={max_depth} left_peak={left} right_peak={right} input_peak={peak:.3}",
            attempt + 1
        );
        result.map_err(|err| std::io::Error::other(format!("{:?}: {}", err.code, err.message)))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("capture_probe requires Windows audio hardware".into())
}
