#[cfg(windows)]
mod windows_probe {
    use audioportal::audio::windows as audio;
    use std::collections::VecDeque;
    use std::error::Error;
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
    use std::thread;
    use std::time::{Duration, Instant};
    use wasapi::{self, Direction, SampleType, StreamMode, WaveFormat};

    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
    const FORMAT_BYTES_PER_FRAME: usize = 8; // stereo, float32
    const MAX_QUEUED_BYTES: usize = 48_000 * FORMAT_BYTES_PER_FRAME / 10; // 100 ms

    fn push_latest(queue: &mut VecDeque<u8>, chunk: Vec<u8>) {
        let excess = queue
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(MAX_QUEUED_BYTES);
        let old = excess.min(queue.len());
        queue.drain(..old);
        queue.extend(chunk.into_iter().skip(excess - old));
    }

    fn check_playback_target(capture_adapter: &str, playback_adapter: &str) -> Result<()> {
        // A virtual cable's capture and render endpoints belong to the same adapter.
        // Adapter identity is independent of a user-renamed endpoint display name.
        if capture_adapter == playback_adapter {
            return Err("capture and playback share an audio adapter; select headphones or speakers on another adapter to avoid feedback".into());
        }
        Ok(())
    }

    fn with_com<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
        wasapi::initialize_mta().ok()?;
        let result = f();
        wasapi::deinitialize();
        result
    }

    pub fn run() -> Result<()> {
        let args: Vec<String> = std::env::args().collect();
        match args.as_slice() {
            [_, command] if command == "list" => with_com(|| {
                for endpoint in audio::list_devices()? {
                    println!("{}: {}\n  id: {}\n  mix: {:?}\n  device: {:?}\n  default/min period: {:.2}/{:.2} ms\n  48k stereo f32: {}",
                        endpoint.kind, endpoint.name, endpoint.id, endpoint.mix_format,
                        endpoint.device_format, endpoint.default_period_hns as f64 / 10_000.,
                        endpoint.min_period_hns as f64 / 10_000., endpoint.float_48k_stereo);
                }
                Ok(())
            }),
            [_, command, capture_id, playback_id] if command == "run" => {
                if capture_id == playback_id {
                    return Err("capture and playback IDs must be different".into());
                }
                // Verify both before starting either stream; no fallback to default devices.
                with_com(|| {
                    let capture = audio::selected_device(capture_id, Direction::Capture)?;
                    let playback = audio::selected_device(playback_id, Direction::Render)?;
                    check_playback_target(
                        &capture.get_interface_friendlyname()?,
                        &playback.get_interface_friendlyname()?,
                    )?;
                    Ok(())
                })?;
                let (tx, rx) = mpsc::sync_channel(4);
                let capture_id = capture_id.clone();
                let playback_id = playback_id.clone();
                let deadline = Instant::now() + Duration::from_secs(10);
                let capture =
                    thread::spawn(move || with_com(|| capture(&capture_id, tx, deadline)));
                let playback =
                    thread::spawn(move || with_com(|| playback(&playback_id, rx, deadline)));
                let capture_result = capture.join().map_err(|_| "capture worker panicked")?;
                let playback_result = playback.join().map_err(|_| "playback worker panicked")?;
                capture_result?;
                playback_result?;
                Ok(())
            }
            _ => Err("usage: audio_probe list | run <capture-id> <playback-id>".into()),
        }
    }

    fn open(id: &str, direction: Direction) -> Result<(wasapi::AudioClient, wasapi::Handle)> {
        let device = audio::selected_device(id, direction)?;
        let mut client = device.get_iaudioclient().map_err(|err| {
            format!("endpoint {id:?} became unavailable ({err}); run `audio_probe list` again")
        })?;
        let (default_period, _) = client.get_device_period().map_err(|err| {
            format!("endpoint {id:?} cannot report its period ({err}); reconnect it and run `audio_probe list` again")
        })?;
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
        client.initialize_client(
            &format,
            &direction,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: default_period,
            },
        )
        .map_err(|err| {
            format!("endpoint {id:?} could not start in shared mode ({err}); reconnect it and run `audio_probe list` again")
        })?;
        let event = client.set_get_eventhandle()?;
        println!(
            "{direction:?} {:?}: 48k stereo f32, buffer {} frames",
            device.get_friendlyname()?,
            client.get_buffer_size()?
        );
        Ok((client, event))
    }

    fn capture(id: &str, tx: SyncSender<Vec<u8>>, deadline: Instant) -> Result<()> {
        let (client, event) = open(id, Direction::Capture)?;
        let reader = client.get_audiocaptureclient()?;
        let mut bytes = vec![0u8; client.get_buffer_size()? as usize * FORMAT_BYTES_PER_FRAME];
        let mut frames_total = 0u64;
        let mut dropped = 0u64;
        let mut peak = 0f32;
        client.start_stream()?;
        let result = (|| -> Result<()> {
            while Instant::now() < deadline {
                event.wait_for_event(200)?;
                while let Some(frames) = reader.get_next_packet_size()? {
                    if frames == 0 || Instant::now() >= deadline {
                        break;
                    }
                    let len = frames as usize * FORMAT_BYTES_PER_FRAME;
                    if len > bytes.len() {
                        return Err("capture packet exceeds WASAPI buffer size".into());
                    }
                    let (read, info) = reader.read_from_device(&mut bytes[..len])?;
                    frames_total += read as u64;
                    for &sample in bytes[..read as usize * FORMAT_BYTES_PER_FRAME]
                        .as_chunks::<4>()
                        .0
                    {
                        peak = peak.max(f32::from_le_bytes(sample).abs());
                    }
                    if info.flags.data_discontinuity {
                        eprintln!("capture discontinuity at frame {}", info.index);
                    }
                    if read > 0
                        && tx
                            .try_send(bytes[..read as usize * FORMAT_BYTES_PER_FRAME].to_vec())
                            .is_err()
                    {
                        dropped += read as u64;
                    }
                }
            }
            Ok(())
        })();
        let stop = client.stop_stream();
        println!("captured {frames_total} frames; peak {peak:.3}; dropped {dropped} queued frames");
        result?;
        stop?;
        Ok(())
    }

    fn playback(id: &str, rx: Receiver<Vec<u8>>, deadline: Instant) -> Result<()> {
        let (client, event) = open(id, Direction::Render)?;
        let writer = client.get_audiorenderclient()?;
        let mut queue = VecDeque::<u8>::with_capacity(MAX_QUEUED_BYTES);
        let mut buffer = vec![0u8; client.get_buffer_size()? as usize * FORMAT_BYTES_PER_FRAME];
        let mut played = 0u64;
        // Prime with silence before starting, then refill after each event.
        let initial = client.get_available_space_in_frames()? as usize;
        writer.write_to_device(initial, &buffer[..initial * FORMAT_BYTES_PER_FRAME], None)?;
        client.start_stream()?;
        let result = (|| -> Result<()> {
            while Instant::now() < deadline {
                event.wait_for_event(200)?;
                loop {
                    match rx.try_recv() {
                        Ok(chunk) => push_latest(&mut queue, chunk),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break,
                    }
                }
                let frames = client.get_available_space_in_frames()? as usize;
                let len = frames * FORMAT_BYTES_PER_FRAME;
                if len > buffer.len() {
                    return Err("render space exceeds WASAPI buffer size".into());
                }
                buffer[..len].fill(0);
                for byte in &mut buffer[..len] {
                    if let Some(sample) = queue.pop_front() {
                        *byte = sample;
                    } else {
                        break;
                    }
                }
                writer.write_to_device(frames, &buffer[..len], None)?;
                played += frames as u64;
            }
            Ok(())
        })();
        let stop = client.stop_stream();
        println!("rendered {played} frames");
        result?;
        stop?;
        Ok(())
    }

    #[test]
    fn queue_keeps_recent_audio_when_full() {
        let mut queue = VecDeque::from(vec![1; MAX_QUEUED_BYTES]);
        push_latest(&mut queue, vec![2; MAX_QUEUED_BYTES + 8]);
        assert_eq!(queue.len(), MAX_QUEUED_BYTES);
        assert!(queue.iter().all(|sample| *sample == 2));
    }

    #[test]
    fn same_adapter_is_not_a_monitor_target() {
        assert!(check_playback_target("VB-Audio Virtual Cable", "VB-Audio Virtual Cable").is_err());
        assert!(check_playback_target("VB-Audio Virtual Cable", "Realtek Audio").is_ok());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(windows)]
    return windows_probe::run();
    #[cfg(not(windows))]
    Err("audio_probe requires Windows audio hardware".into())
}
