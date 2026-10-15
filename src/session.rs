//! One owned session. The monitor runs independently of HTTP polling.
use crate::audio::WorkerHandle;
use crate::contracts::{ApiError, ErrorCode, Mode, Settings, State, Status};
use crate::transport::{ReceiverMetrics, SenderMetrics};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MONITOR_INTERVAL: Duration = Duration::from_millis(10);

fn error(code: ErrorCode, message: impl Into<String>) -> ApiError {
    ApiError {
        code,
        message: message.into(),
    }
}

fn stopped() -> State {
    State {
        status: Status::Stopped,
        mode: None,
        input_peak: 0.0,
        output_peak: 0.0,
        packets_lost: 0,
        buffer_ms: 0,
        error: None,
    }
}

pub struct SessionController {
    state: Arc<Mutex<State>>,
    monitor: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    shutting_down: bool,
}

impl Default for SessionController {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(stopped())),
            monitor: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            shutting_down: false,
        }
    }
}

impl SessionController {
    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn start(&mut self, settings: &Settings) -> Result<State, ApiError> {
        self.start_with(settings, validate_device, build_pipeline)
    }

    // Closures keep lifecycle tests independent of audio hardware without a mock backend.
    fn start_with(
        &mut self,
        settings: &Settings,
        validate_device: impl FnOnce(&str, Mode) -> Result<(), ApiError>,
        build: impl FnOnce(&mut Pipeline, &str, Ipv4Addr, &Settings) -> Result<(), ApiError>,
    ) -> Result<State, ApiError> {
        self.require_stopped()?;
        let (peer, id) = validate_start(settings)?;
        validate_device(id, settings.mode)?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State {
            status: Status::Starting,
            mode: Some(settings.mode),
            buffer_ms: if settings.mode == Mode::Receive {
                settings.buffer_ms
            } else {
                0
            },
            ..stopped()
        };
        let mut pipeline = Pipeline::default();
        if let Err(mut failure) = build(&mut pipeline, id, peer, settings) {
            if let Err(cleanup) = pipeline.shutdown() {
                failure
                    .message
                    .push_str(&format!("; cleanup: {}", cleanup.message));
            }
            finish(&self.state, &pipeline, Some(failure.clone()));
            return Err(failure);
        }
        self.cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = self.cancelled.clone();
        let state = self.state.clone();
        // On spawn failure the captured pipeline's Drop also cancels and joins it.
        match thread::Builder::new()
            .name("audioportal-session".into())
            .spawn(move || {
                monitor(pipeline, state, cancelled);
            }) {
            Ok(monitor) => self.monitor = Some(monitor),
            Err(cause) => {
                let failure = error(
                    ErrorCode::WorkerFailed,
                    format!("Cannot start session monitor: {cause}"),
                );
                finish(&self.state, &Pipeline::default(), Some(failure.clone()));
                return Err(failure);
            }
        }
        Ok(self.snapshot())
    }

    pub fn stop(&mut self) -> Result<State, ApiError> {
        if let Some(monitor) = self.monitor.take() {
            // Failed is published only after cleanup; Stop acknowledges that old
            // failure, but must report a new failure encountered during this Stop.
            let already_failed = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let failed = state.status == Status::Failed;
                state.status = Status::Stopping;
                failed
            };
            self.cancelled.store(true, Ordering::Release);
            // Neither the state mutex nor any queue lock is held across join.
            let panicked = monitor.join().is_err();
            if panicked {
                finish(
                    &self.state,
                    &Pipeline::default(),
                    Some(error(ErrorCode::WorkerFailed, "Session monitor panicked")),
                );
            }
            if !already_failed || panicked {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(failure) = state.error.clone() {
                    state.status = Status::Failed;
                    return Err(failure);
                }
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = State {
            packets_lost: state.packets_lost,
            ..stopped()
        };
        Ok(state.clone())
    }

    pub(crate) fn require_stopped(&self) -> Result<(), ApiError> {
        if self.shutting_down {
            return Err(error(
                ErrorCode::LifecycleConflict,
                "Audioportal is shutting down",
            ));
        }
        if self.snapshot().status != Status::Stopped {
            return Err(error(
                ErrorCode::LifecycleConflict,
                "Stop the session before starting again or changing settings",
            ));
        }
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) -> Result<State, ApiError> {
        // Called under the same controller lock as HTTP Start/Save, including
        // blocking handlers that may outlive the HTTP drain deadline.
        self.shutting_down = true;
        self.stop()
    }

    #[cfg(test)]
    pub(crate) fn start_test_worker(&mut self, worker: WorkerHandle) {
        let settings = Settings {
            capture_device_id: Some("test".into()),
            peer_ip: "127.0.0.1".into(),
            ..Settings::default()
        };
        self.start_with(
            &settings,
            |_, _| Ok(()),
            |pipeline, _, _, _| {
                pipeline.workers.push(worker);
                Ok(())
            },
        )
        .unwrap();
    }

    #[cfg(test)]
    pub(crate) fn set_test_status(&mut self, status: Status) {
        self.state.lock().unwrap().status = status;
    }
}

impl Drop for SessionController {
    fn drop(&mut self) {
        // Explicit application shutdown calls stop on a blocking thread and reports errors.
        let _ = self.stop();
    }
}

fn validate_start(settings: &Settings) -> Result<(Ipv4Addr, &str), ApiError> {
    crate::settings::validate(settings)?;
    let peer: Ipv4Addr = settings.peer_ip.parse().map_err(|_| {
        error(
            ErrorCode::InvalidSettings,
            "Enter the peer's unicast IPv4 address",
        )
    })?;
    if peer.is_unspecified() || peer.is_multicast() || peer.octets()[0] >= 240 {
        return Err(error(
            ErrorCode::InvalidSettings,
            "Peer IP must be a unicast IPv4 address",
        ));
    }
    let id = match settings.mode {
        Mode::Send => &settings.capture_device_id,
        Mode::Receive => &settings.playback_device_id,
    }
    .as_deref()
    .ok_or_else(|| {
        error(
            ErrorCode::InvalidSettings,
            "Choose a device for the selected mode",
        )
    })?;
    Ok((peer, id))
}

#[cfg(not(windows))]
fn validate_device(_id: &str, _mode: Mode) -> Result<(), ApiError> {
    Err(error(
        ErrorCode::BackendUnavailable,
        "Audio sessions require Windows",
    ))
}

#[cfg(windows)]
fn validate_device(id: &str, mode: Mode) -> Result<(), ApiError> {
    wasapi::initialize_mta().ok().map_err(|cause| {
        error(
            ErrorCode::BackendUnavailable,
            format!("Cannot initialize Windows audio: {cause}"),
        )
    })?;
    let direction = match mode {
        Mode::Send => wasapi::Direction::Capture,
        Mode::Receive => wasapi::Direction::Render,
    };
    // Drop all COM objects before uninitializing this caller's apartment.
    let result = crate::audio::windows::selected_device(id, direction)
        .map(|_| ())
        .map_err(|cause| error(ErrorCode::DeviceUnavailable, cause.to_string()));
    wasapi::deinitialize();
    result
}

#[cfg(not(windows))]
fn build_pipeline(_: &mut Pipeline, _: &str, _: Ipv4Addr, _: &Settings) -> Result<(), ApiError> {
    Err(error(
        ErrorCode::BackendUnavailable,
        "Audio sessions require Windows",
    ))
}

#[cfg(windows)]
fn build_pipeline(
    pipeline: &mut Pipeline,
    id: &str,
    peer: Ipv4Addr,
    settings: &Settings,
) -> Result<(), ApiError> {
    use crate::{audio::windows, transport};
    match settings.mode {
        Mode::Send => {
            let capture = windows::start_capture(id)?;
            pipeline.input_peak = Some(capture.input_peak);
            pipeline.workers.push(capture.worker);
            let sender = transport::start_sender(peer, settings.udp_port, capture.blocks)?;
            pipeline.sender = Some(sender.metrics);
            pipeline.workers.push(sender.worker);
        }
        Mode::Receive => {
            let playback = windows::start_playback(id)?;
            pipeline.output_peak = Some(playback.output_peak);
            pipeline.workers.push(playback.worker);
            let receiver = transport::start_receiver(
                peer,
                settings.udp_port,
                settings.buffer_ms,
                playback.blocks,
                playback.ratio,
                playback.metrics,
            )?;
            pipeline.receiver = Some(receiver.metrics);
            pipeline.workers.push(receiver.worker);
        }
    }
    Ok(())
}

#[derive(Default)]
struct Pipeline {
    workers: Vec<WorkerHandle>,
    input_peak: Option<Arc<AtomicU32>>,
    output_peak: Option<Arc<AtomicU32>>,
    sender: Option<Arc<SenderMetrics>>,
    receiver: Option<Arc<ReceiverMetrics>>,
}

impl Pipeline {
    fn sample(&self, state: &mut State) {
        fn peak(value: &Option<Arc<AtomicU32>>) -> f64 {
            let value = value
                .as_ref()
                .map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)));
            if value.is_finite() {
                value.clamp(0.0, 1.0) as f64
            } else {
                0.0
            }
        }
        state.input_peak = peak(&self.input_peak);
        state.output_peak = peak(&self.output_peak);
        if let Some(receiver) = &self.receiver {
            state.status = receiver.status();
            state.packets_lost = receiver.packets_lost();
        } else if self.sender.as_ref().is_some_and(|s| s.packets_sent() > 0) {
            state.status = Status::Sending;
        }
    }

    fn shutdown(&mut self) -> Result<(), ApiError> {
        for worker in &self.workers {
            worker.cancel();
        }
        let mut failure: Option<ApiError> = None;
        for worker in self.workers.drain(..) {
            if let Err(err) = worker.join() {
                if let Some(first) = &mut failure {
                    first.message.push_str(&format!("; {}", err.message));
                } else {
                    failure = Some(err);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn finish(state: &Mutex<State>, pipeline: &Pipeline, failure: Option<ApiError>) {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    pipeline.sample(&mut state);
    state.input_peak = 0.0;
    state.output_peak = 0.0;
    state.buffer_ms = 0;
    state.status = if failure.is_some() {
        Status::Failed
    } else {
        Status::Stopped
    };
    if failure.is_none() {
        state.mode = None;
    }
    state.error = failure;
}

fn monitor(mut pipeline: Pipeline, state: Arc<Mutex<State>>, cancelled: Arc<AtomicBool>) {
    let mut failure = None;
    while !cancelled.load(Ordering::Acquire) {
        for worker in &pipeline.workers {
            if let Ok(err) = worker.errors.try_recv() {
                failure = Some(err);
                break;
            }
            if worker.thread.is_finished() {
                failure = Some(error(
                    ErrorCode::WorkerFailed,
                    "Session worker exited unexpectedly",
                ));
                break;
            }
        }
        if failure.is_some() {
            break;
        }
        {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            if state.status != Status::Stopping {
                pipeline.sample(&mut state);
            }
        }
        thread::sleep(MONITOR_INTERVAL);
    }
    // All workers see cancellation before any join. They never acquire state.
    if let Err(join_error) = pipeline.shutdown() {
        failure = Some(join_error);
    }
    finish(&state, &pipeline, failure);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        AudioBlock, BlockQueue, ConsumptionRatio, PLAYBACK_QUEUE_BLOCKS, SAMPLES_PER_BLOCK,
    };
    use crate::transport;
    use std::net::UdpSocket;
    use std::sync::mpsc;
    use std::time::Instant;

    fn settings() -> Settings {
        Settings {
            capture_device_id: Some("capture".into()),
            playback_device_id: Some("playback".into()),
            peer_ip: "127.0.0.1".into(),
            ..Settings::default()
        }
    }

    fn wait(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for session");
            thread::sleep(Duration::from_millis(2));
        }
    }

    // A real cancellable thread, with a single injected error/panic trigger.
    fn worker(panic: bool) -> (WorkerHandle, mpsc::Sender<()>, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let done = exited.clone();
        let (trigger, fail) = mpsc::channel();
        let (errors_tx, errors) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            while !cancel.load(Ordering::Acquire) {
                if fail.try_recv().is_ok() {
                    done.store(true, Ordering::Release);
                    assert!(!panic, "injected worker panic");
                    let failure = error(ErrorCode::WorkerFailed, "injected device loss");
                    let _ = errors_tx.try_send(failure.clone());
                    return Err(failure);
                }
                thread::sleep(Duration::from_millis(1));
            }
            done.store(true, Ordering::Release);
            Ok(())
        });
        (
            WorkerHandle {
                cancelled,
                errors,
                thread,
            },
            trigger,
            exited,
        )
    }

    #[test]
    fn lifecycle_cleanup_failure_stop_and_restart() {
        let mut session = SessionController::default();
        let config = settings();
        let (first, _trigger, exited) = worker(false);
        let failure = session
            .start_with(
                &config,
                |_, _| Ok(()),
                |pipeline, _, _, _| {
                    pipeline.workers.push(first);
                    Err(error(
                        ErrorCode::WorkerFailed,
                        "injected second worker startup failure",
                    ))
                },
            )
            .unwrap_err();
        assert!(failure.message.contains("startup failure"));
        assert!(exited.load(Ordering::Acquire));
        assert_eq!(session.snapshot().status, Status::Failed);
        assert_eq!(session.snapshot().mode, Some(Mode::Send));
        assert_eq!(
            session.start(&config).unwrap_err().code,
            ErrorCode::LifecycleConflict
        );
        assert_eq!(session.stop().unwrap(), stopped());
        assert_eq!(session.stop().unwrap(), stopped());

        for panic in [false, true] {
            let (first, trigger, exited) = worker(panic);
            let (second, _, peer_exited) = worker(false);
            session
                .start_with(
                    &config,
                    |_, _| Ok(()),
                    |pipeline, _, _, _| {
                        pipeline.workers.extend([first, second]);
                        pipeline.input_peak = Some(Arc::new(AtomicU32::new(0.5f32.to_bits())));
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(
                session.start(&config).unwrap_err().code,
                ErrorCode::LifecycleConflict
            );
            assert_eq!(
                session.require_stopped().unwrap_err().code,
                ErrorCode::LifecycleConflict
            );
            trigger.send(()).unwrap();
            // Failure cleanup must happen without any snapshot/API calls.
            wait(|| peer_exited.load(Ordering::Acquire));
            wait(|| session.snapshot().status == Status::Failed);
            assert!(exited.load(Ordering::Acquire));
            let failed = session.snapshot();
            assert_eq!(failed.input_peak, 0.0);
            assert_eq!(failed.buffer_ms, 0);
            assert!(failed.error.unwrap().message.contains(if panic {
                "panicked"
            } else {
                "device loss"
            }));
            assert_eq!(session.stop().unwrap(), stopped());
        }

        let (first, _, exited) = worker(false);
        session
            .start_with(
                &config,
                |_, _| Ok(()),
                |pipeline, _, _, _| {
                    pipeline.workers.push(first);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(session.snapshot().packets_lost, 0);
        assert!(session.snapshot().error.is_none());
        assert_eq!(session.stop().unwrap(), stopped());
        assert!(exited.load(Ordering::Acquire));
        assert_eq!(session.stop().unwrap(), stopped());
    }

    #[test]
    fn validation_stays_stopped_and_checks_only_selected_endpoint() {
        let mut session = SessionController::default();
        for ip in [
            "",
            "bad",
            "::1",
            "0.0.0.0",
            "224.0.0.1",
            "240.1.2.3",
            "255.255.255.255",
        ] {
            let config = Settings {
                peer_ip: ip.into(),
                ..settings()
            };
            assert_eq!(
                session.start(&config).unwrap_err().code,
                ErrorCode::InvalidSettings
            );
            assert_eq!(session.snapshot(), stopped());
        }
        let missing = Settings {
            capture_device_id: None,
            ..settings()
        };
        assert_eq!(
            session.start(&missing).unwrap_err().code,
            ErrorCode::InvalidSettings
        );
        let config = Settings {
            playback_device_id: Some("missing unused endpoint".into()),
            ..settings()
        };
        session
            .start_with(
                &config,
                |id, mode| {
                    assert_eq!(id, "capture");
                    assert_eq!(mode, Mode::Send);
                    Err(error(ErrorCode::DeviceUnavailable, "missing capture"))
                },
                |_, _, _, _| panic!("must not start workers"),
            )
            .unwrap_err();
        assert_eq!(session.snapshot(), stopped());
        #[cfg(not(windows))]
        assert_eq!(
            session.start(&config).unwrap_err().code,
            ErrorCode::BackendUnavailable
        );
    }

    #[test]
    fn stop_reports_join_failure_but_still_joins_peers() {
        let mut session = SessionController::default();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (_errors_tx, errors) = mpsc::sync_channel(1);
        let failing = WorkerHandle {
            cancelled,
            errors,
            thread: thread::spawn(move || {
                while !cancel.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error(ErrorCode::WorkerFailed, "injected stop failure"))
            }),
        };
        let (peer, _, peer_exited) = worker(false);
        session
            .start_with(
                &settings(),
                |_, _| Ok(()),
                |pipeline, _, _, _| {
                    pipeline.workers.extend([failing, peer]);
                    Ok(())
                },
            )
            .unwrap();
        assert!(session.stop().unwrap_err().message.contains("stop failure"));
        assert!(peer_exited.load(Ordering::Acquire));
        assert_eq!(session.snapshot().status, Status::Failed);
        assert_eq!(session.snapshot().mode, Some(Mode::Send));
        assert_eq!(session.stop().unwrap(), stopped());
    }

    #[test]
    fn receiver_metrics_timeout_port_conflict_and_fresh_restart() {
        let mut session = SessionController::default();
        let config = Settings {
            mode: Mode::Receive,
            buffer_ms: 10,
            ..settings()
        };
        let occupied = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        let (playback, _, exited) = worker(false);
        session
            .start_with(
                &config,
                |_, _| Ok(()),
                |pipeline, _, peer, _| {
                    pipeline.workers.push(playback);
                    let receiver = transport::start_receiver(
                        peer,
                        port,
                        10,
                        BlockQueue::new(4),
                        ConsumptionRatio::default(),
                        Arc::new(crate::audio::PlaybackMetrics::default()),
                    )?;
                    pipeline.workers.push(receiver.worker);
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(exited.load(Ordering::Acquire));
        assert_eq!(session.snapshot().status, Status::Failed);
        session.stop().unwrap();
        drop(occupied);

        for _ in 0..2 {
            let queue = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
            let mut local_port = 0;
            session
                .start_with(
                    &config,
                    |_, _| Ok(()),
                    |pipeline, _, peer, _| {
                        let receiver = transport::start_receiver(
                            peer,
                            port,
                            10,
                            queue.clone(),
                            ConsumptionRatio::default(),
                            Arc::new(crate::audio::PlaybackMetrics::default()),
                        )?;
                        local_port = receiver.local_addr.port();
                        pipeline.receiver = Some(receiver.metrics);
                        pipeline.output_peak = Some(Arc::new(AtomicU32::new(f32::NAN.to_bits())));
                        pipeline.workers.push(receiver.worker);
                        Ok(())
                    },
                )
                .unwrap();
            wait(|| session.snapshot().status == Status::Listening);
            assert_eq!(session.snapshot().packets_lost, 0);
            assert!(queue.is_empty());
            let capture = BlockQueue::new(20);
            let sender =
                transport::start_sender(Ipv4Addr::LOCALHOST, local_port, capture.clone()).unwrap();
            capture.push(AudioBlock {
                frame_index: 0,
                samples: [7; SAMPLES_PER_BLOCK],
            });
            wait(|| session.snapshot().status == Status::Receiving);
            assert_eq!(session.snapshot().buffer_ms, 10);
            assert_eq!(session.snapshot().output_peak, 0.0);
            wait(|| session.snapshot().packets_lost > 0);
            sender.worker.cancel();
            sender.worker.join().unwrap();
            wait(|| session.snapshot().status == Status::Listening);
            assert!(queue.is_empty());
            let lost = session.snapshot().packets_lost;
            assert!(lost > 0);
            let state = session.stop().unwrap();
            assert_eq!(state.packets_lost, lost);
            assert_eq!(state.buffer_ms, 0);
            assert_eq!(session.stop().unwrap(), state);
        }
    }

    #[test]
    fn sender_reports_emission_and_drop_joins_workers() {
        let mut session = SessionController::default();
        let queue = BlockQueue::new(20);
        let (capture, _, exited) = worker(false);
        session
            .start_with(
                &settings(),
                |_, _| Ok(()),
                |pipeline, _, peer, _| {
                    pipeline.workers.push(capture);
                    let sender = transport::start_sender(peer, 9, queue.clone())?;
                    pipeline.sender = Some(sender.metrics);
                    pipeline.workers.push(sender.worker);
                    pipeline.input_peak = Some(Arc::new(AtomicU32::new(0.75f32.to_bits())));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(session.snapshot().status, Status::Starting);
        queue.push(AudioBlock {
            frame_index: 0,
            samples: [1; SAMPLES_PER_BLOCK],
        });
        wait(|| session.snapshot().status == Status::Sending);
        assert_eq!(session.snapshot().input_peak, 0.75);
        assert_eq!(session.snapshot().packets_lost, 0);
        assert_eq!(session.snapshot().buffer_ms, 0);
        drop(session);
        assert!(exited.load(Ordering::Acquire));
    }
}
