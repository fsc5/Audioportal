//! Manual task 04 check: generated tones, no capture/network dependencies.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use audioportal::audio::{windows::start_playback, AudioBlock, SAMPLES_PER_BLOCK};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().collect();
    let [_, mode, id] = args.as_slice() else {
        return Err("usage: playback_probe <run|underflow|ratio|restart> <playback-id>".into());
    };
    if !["run", "underflow", "ratio", "restart"].contains(&mode.as_str()) {
        return Err("usage: playback_probe <run|underflow|ratio|restart> <playback-id>".into());
    }
    let api_error = |err: audioportal::contracts::ApiError| {
        std::io::Error::other(format!("{:?}: {}", err.code, err.message))
    };
    for attempt in 0..if mode == "restart" { 2 } else { 1 } {
        let playback = start_playback(id).map_err(api_error)?;
        println!("attempt {}: stream rate {} Hz; alternating 440 Hz LEFT / 880 Hz RIGHT every two signal seconds",
            attempt + 1, playback.metrics.output_rate.load(Ordering::Relaxed));
        if mode == "underflow" {
            println!("withholding input from second 3 to 5");
        }
        if attempt == 1 {
            println!("first two seconds must be silent after restart");
        }
        let start = Instant::now();
        let mut index = 0u64;
        let mut requested = 1.;
        let mut max_peak = 0f32;
        while start.elapsed() < Duration::from_secs(10) {
            if playback.worker.errors.try_recv().is_ok() {
                break;
            }
            let seconds = start.elapsed().as_secs_f32();
            if mode == "ratio" {
                let target = if seconds < 3. {
                    1.
                } else if seconds < 6. {
                    1.005
                } else {
                    0.995
                };
                if requested != target {
                    // Constants are within the frozen ratio bounds.
                    playback.ratio.set(target).expect("valid probe ratio");
                    requested = target;
                    println!("requested consumption ratio {target}");
                }
            }
            let silent = (mode == "underflow" && (3. ..5.).contains(&seconds))
                || (attempt == 1 && seconds < 2.);
            if !silent {
                // Keep a bounded prefill; this probe isolates playback from producer jitter.
                for _ in playback.blocks.len()..3 {
                    let mut samples = [0; SAMPLES_PER_BLOCK];
                    for pair in samples.as_chunks_mut::<2>().0 {
                        let channel = ((index / 96_000) % 2) as usize;
                        let hz = if channel == 0 { 440. } else { 880. };
                        pair[channel] = ((std::f64::consts::TAU * hz * index as f64 / 48_000.)
                            .sin()
                            * 4096.) as i16;
                        index += 1;
                    }
                    playback.blocks.push(AudioBlock {
                        frame_index: index - 240,
                        samples,
                    });
                }
            }
            max_peak = max_peak.max(f32::from_bits(playback.output_peak.load(Ordering::Relaxed)));
            std::thread::sleep(Duration::from_millis(2));
        }
        let stop = Instant::now();
        playback.worker.cancel();
        let result = playback.worker.join();
        println!("join={:?}; submitted={} missing_input={} peak={max_peak:.3} applied_ratio={:.6} queue_after_join={}",
            stop.elapsed(), playback.metrics.submitted_frames.load(Ordering::Relaxed),
            playback.metrics.missing_input_frames.load(Ordering::Relaxed),
            f32::from_bits(playback.metrics.applied_ratio.load(Ordering::Relaxed)), playback.blocks.len());
        result.map_err(api_error)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("playback_probe requires Windows audio hardware".into())
}
