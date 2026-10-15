//! Bounded UDP sender and receiver workers.
use crate::audio::drift::{frames_duration, occupancy_frames, DriftController};
use crate::audio::{BlockQueue, ConsumptionRatio, PlaybackMetrics, WorkerHandle, FRAMES_PER_BLOCK};
use crate::contracts::{ApiError, ErrorCode, Status};
use crate::jitter::JitterBuffer;
use crate::protocol::{decode, encode, Packet, PACKET_LEN};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

pub const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(1);

const POLL_INTERVAL: Duration = Duration::from_millis(1);
const DATAGRAMS_PER_TICK: usize = 32;

fn failed(message: impl Into<String>) -> ApiError {
    ApiError {
        code: ErrorCode::WorkerFailed,
        message: message.into(),
    }
}

pub struct SenderMetrics {
    packets_sent: AtomicU64,
}

impl SenderMetrics {
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }
}

pub struct ReceiverMetrics {
    receiving: AtomicBool,
    packets_lost: AtomicU64,
    occupancy: AtomicUsize,
    local_skipped_blocks: AtomicU64,
    queue_evicted_blocks: AtomicU64,
    total_frames: AtomicU64, // f64 bits; 48 kHz frames, including playback/device buffers
}

impl ReceiverMetrics {
    pub fn status(&self) -> Status {
        if self.receiving.load(Ordering::Relaxed) {
            Status::Receiving
        } else {
            Status::Listening
        }
    }

    pub fn packets_lost(&self) -> u64 {
        self.packets_lost.load(Ordering::Relaxed)
    }

    pub fn occupancy(&self) -> usize {
        self.occupancy.load(Ordering::Relaxed)
    }

    pub fn local_skipped_blocks(&self) -> u64 {
        self.local_skipped_blocks.load(Ordering::Relaxed)
    }

    pub fn queue_evicted_blocks(&self) -> u64 {
        self.queue_evicted_blocks.load(Ordering::Relaxed)
    }

    pub fn total_frames(&self) -> f64 {
        f64::from_bits(self.total_frames.load(Ordering::Relaxed))
    }
}

pub struct SenderWorker {
    pub worker: WorkerHandle,
    pub metrics: Arc<SenderMetrics>,
    pub session_id: u64,
    pub local_addr: SocketAddrV4,
}

pub struct ReceiverWorker {
    pub worker: WorkerHandle,
    pub metrics: Arc<ReceiverMetrics>,
    pub local_addr: SocketAddrV4,
}

/// Starts a worker that drains captured blocks and emits one UDP datagram per block.
pub fn start_sender(
    peer_ip: Ipv4Addr,
    port: u16,
    blocks: BlockQueue,
) -> Result<SenderWorker, ApiError> {
    let destination = SocketAddrV4::new(peer_ip, port);
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).map_err(|err| {
        failed(format!(
            "cannot bind UDP sender to an ephemeral IPv4 port: {err}"
        ))
    })?;
    socket
        .set_nonblocking(true)
        .map_err(|err| failed(format!("cannot configure UDP sender socket: {err}")))?;
    let local_addr = ipv4_addr(
        socket
            .local_addr()
            .map_err(|err| failed(format!("cannot read UDP sender socket address: {err}")))?,
    )?;
    let session_id = random_session_id()?;
    let metrics = Arc::new(SenderMetrics {
        packets_sent: AtomicU64::new(0),
    });
    let thread_metrics = metrics.clone();
    let worker = spawn_worker("audioportal-udp-sender", move |cancelled| {
        run_sender(
            socket,
            destination,
            session_id,
            blocks,
            &thread_metrics,
            &cancelled,
        )
    })?;
    Ok(SenderWorker {
        worker,
        metrics,
        session_id,
        local_addr,
    })
}

/// Starts a worker that validates UDP packets and drives monotonic playout into
/// the playback worker's bounded input queue.
pub fn start_receiver(
    peer_ip: Ipv4Addr,
    port: u16,
    buffer_ms: u16,
    blocks: BlockQueue,
    ratio: ConsumptionRatio,
    playback: Arc<PlaybackMetrics>,
) -> Result<ReceiverWorker, ApiError> {
    let jitter = JitterBuffer::new(buffer_ms)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
        .map_err(|err| failed(format!("cannot bind UDP receiver port {port}: {err}")))?;
    socket
        .set_nonblocking(true)
        .map_err(|err| failed(format!("cannot configure UDP receiver socket: {err}")))?;
    let local_addr = ipv4_addr(
        socket
            .local_addr()
            .map_err(|err| failed(format!("cannot read UDP receiver socket address: {err}")))?,
    )?;
    let metrics = Arc::new(ReceiverMetrics {
        receiving: AtomicBool::new(false),
        packets_lost: AtomicU64::new(0),
        occupancy: AtomicUsize::new(0),
        local_skipped_blocks: AtomicU64::new(0),
        queue_evicted_blocks: AtomicU64::new(0),
        total_frames: AtomicU64::new(0f64.to_bits()),
    });
    let thread_metrics = metrics.clone();
    let worker = spawn_worker("audioportal-udp-receiver", move |cancelled| {
        let state = ReceiveState::new(peer_ip, jitter, buffer_ms);
        let result = run_receiver(
            socket,
            blocks,
            ratio,
            playback,
            state,
            &thread_metrics,
            &cancelled,
        );
        thread_metrics.receiving.store(false, Ordering::Relaxed);
        result
    })?;
    Ok(ReceiverWorker {
        worker,
        metrics,
        local_addr,
    })
}

fn run_sender(
    socket: UdpSocket,
    destination: SocketAddrV4,
    session_id: u64,
    blocks: BlockQueue,
    metrics: &SenderMetrics,
    cancelled: &AtomicBool,
) -> Result<(), ApiError> {
    let mut last_frame = None;
    while !cancelled.load(Ordering::Acquire) {
        let Some(block) = blocks.pop() else {
            thread::sleep(POLL_INTERVAL);
            continue;
        };
        if !block.frame_index.is_multiple_of(FRAMES_PER_BLOCK as u64)
            || last_frame.is_some_and(|last| block.frame_index <= last)
        {
            return Err(failed(format!(
                "capture produced an invalid UDP timeline at frame {}",
                block.frame_index
            )));
        }
        last_frame = Some(block.frame_index);
        let bytes = encode(&Packet {
            session_id,
            sequence: (block.frame_index / FRAMES_PER_BLOCK as u64) as u32,
            block,
        });
        match socket.send_to(&bytes, destination) {
            Ok(PACKET_LEN) => {
                metrics.packets_sent.fetch_add(1, Ordering::Relaxed);
            }
            Ok(sent) => {
                return Err(failed(format!(
                    "UDP sender wrote {sent} of {PACKET_LEN} packet bytes to {destination}"
                )));
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => {
                return Err(failed(format!(
                    "cannot send UDP audio to {destination}: {err}"
                )));
            }
        }
    }
    Ok(())
}

struct ReceiveState {
    peer_ip: Ipv4Addr,
    active_session: Option<u64>,
    last_accepted: Option<Instant>,
    jitter: JitterBuffer,
    target_ms: u16,
    first_packet: Option<Instant>,
    media_clock: Option<(Instant, Instant)>, // real sample time, virtual playout time
    playout_control: DriftController,
    playback_control: DriftController,
    queue_evicted_blocks: u64,
}

impl ReceiveState {
    fn new(peer_ip: Ipv4Addr, jitter: JitterBuffer, target_ms: u16) -> Self {
        Self {
            peer_ip,
            active_session: None,
            last_accepted: None,
            jitter,
            target_ms,
            first_packet: None,
            media_clock: None,
            // The release phase oscillates around the target by half a block.
            playout_control: DriftController::new(target_ms as usize * 48 - FRAMES_PER_BLOCK / 2),
            // Leave two blocks after the jitter stage for callback phase variation.
            playback_control: DriftController::new(target_ms as usize * 48 + 2 * FRAMES_PER_BLOCK),
            queue_evicted_blocks: 0,
        }
    }

    fn media_now(&mut self, now: Instant) -> Instant {
        let (previous, media) = self.media_clock.unwrap_or((now, now));
        let seconds = now.saturating_duration_since(previous).as_secs_f64();
        let advanced = Duration::from_secs_f64(seconds * self.playout_control.ratio() as f64);
        let media = media.checked_add(advanced).unwrap_or(now);
        self.media_clock = Some((now, media));
        media
    }

    fn expire(&mut self, now: Instant, blocks: &BlockQueue, ratio: &ConsumptionRatio) -> bool {
        if !self
            .last_accepted
            .is_some_and(|last| now.saturating_duration_since(last) >= SESSION_IDLE_TIMEOUT)
        {
            return false;
        }
        self.active_session = None;
        self.last_accepted = None;
        self.jitter.reset();
        self.first_packet = None;
        self.media_clock = None;
        self.playout_control.reset();
        self.playback_control.reset();
        while blocks.pop().is_some() {}
        ratio.set(1.0).expect("1.0 is a valid consumption ratio");
        true
    }

    fn accept(&mut self, source: SocketAddr, bytes: &[u8], now: Instant) -> bool {
        if source.ip() != IpAddr::V4(self.peer_ip) {
            return false;
        }
        let Ok(packet) = decode(bytes) else {
            return false;
        };
        if self
            .active_session
            .is_some_and(|session| session != packet.session_id)
        {
            return false;
        }
        let media = self.media_now(now);
        if !self.jitter.insert(packet.block, media) {
            return false;
        }
        if self.active_session.is_none() {
            self.first_packet = Some(now);
        }
        self.active_session = Some(packet.session_id);
        self.last_accepted = Some(now);
        true
    }

    fn playout(&mut self, now: Instant, blocks: &BlockQueue) -> u64 {
        let mut lost = 0u64;
        let media = self.media_now(now);
        while let Some((block, silence)) = self.jitter.poll(media) {
            self.queue_evicted_blocks = self
                .queue_evicted_blocks
                .saturating_add(u64::from(blocks.push(block)));
            lost += u64::from(silence);
        }
        lost
    }

    fn correct(
        &mut self,
        now: Instant,
        blocks: &BlockQueue,
        ratio: &ConsumptionRatio,
        playback: &PlaybackMetrics,
    ) -> Option<f64> {
        if !self.first_packet.is_some_and(|first| {
            now.saturating_duration_since(first)
                >= Duration::from_millis(u64::from(self.target_ms) + 200)
        }) {
            return None;
        }
        let lead = self.jitter.occupancy();
        self.playout_control
            .update((lead * FRAMES_PER_BLOCK) as f64, now);
        let total = occupancy_frames(lead, blocks.len(), playback)?;
        // Filter and device padding are in the measured total, but constitute
        // fixed downstream latency; feedback must not try to drain the device.
        let rate = playback.output_rate.load(Ordering::Relaxed) as f64;
        let fixed = (playback.padding_frames.load(Ordering::Relaxed) as f64
            + playback.resampler_delay_frames.load(Ordering::Relaxed) as f64)
            * 48_000.
            / rate;
        ratio
            .set(self.playback_control.update(total - fixed, now))
            .expect("controller ratio is within the consumption ratio bounds");
        Some(total)
    }
}

fn run_receiver(
    socket: UdpSocket,
    blocks: BlockQueue,
    ratio: ConsumptionRatio,
    playback: Arc<PlaybackMetrics>,
    mut state: ReceiveState,
    metrics: &ReceiverMetrics,
    cancelled: &AtomicBool,
) -> Result<(), ApiError> {
    // A full UDP payload buffer prevents oversized datagrams from becoming
    // WSAEMSGSIZE worker failures on Windows; decode rejects non-exact lengths.
    let mut bytes = vec![0; u16::MAX as usize];
    let mut last_report = Instant::now();
    let mut skipped_before_reset = 0u64;
    while !cancelled.load(Ordering::Acquire) {
        let mut now = Instant::now();
        let skipped = state.jitter.local_skipped_blocks();
        if state.expire(now, &blocks, &ratio) {
            skipped_before_reset = skipped_before_reset.saturating_add(skipped);
            metrics.receiving.store(false, Ordering::Relaxed);
            metrics
                .total_frames
                .store(0f64.to_bits(), Ordering::Relaxed);
        }

        for _ in 0..DATAGRAMS_PER_TICK {
            match socket.recv_from(&mut bytes) {
                Ok((length, source)) => {
                    now = Instant::now();
                    let skipped = state.jitter.local_skipped_blocks();
                    if state.expire(now, &blocks, &ratio) {
                        skipped_before_reset = skipped_before_reset.saturating_add(skipped);
                        metrics.receiving.store(false, Ordering::Relaxed);
                        metrics
                            .total_frames
                            .store(0f64.to_bits(), Ordering::Relaxed);
                    }
                    if state.accept(source, &bytes[..length], now) {
                        metrics.receiving.store(true, Ordering::Relaxed);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    return Err(failed(format!("cannot receive UDP audio: {err}")));
                }
            }
        }

        now = Instant::now();
        let skipped = state.jitter.local_skipped_blocks();
        if state.expire(now, &blocks, &ratio) {
            skipped_before_reset = skipped_before_reset.saturating_add(skipped);
            metrics.receiving.store(false, Ordering::Relaxed);
            metrics
                .total_frames
                .store(0f64.to_bits(), Ordering::Relaxed);
        } else {
            let lost = state.playout(now, &blocks);
            metrics.packets_lost.fetch_add(lost, Ordering::Relaxed);
            if let Some(total) = state.correct(now, &blocks, &ratio, &playback) {
                metrics
                    .total_frames
                    .store(total.to_bits(), Ordering::Relaxed);
            }
        }
        metrics
            .occupancy
            .store(state.jitter.occupancy(), Ordering::Relaxed);
        metrics.local_skipped_blocks.store(
            skipped_before_reset.saturating_add(state.jitter.local_skipped_blocks()),
            Ordering::Relaxed,
        );
        metrics
            .queue_evicted_blocks
            .store(state.queue_evicted_blocks, Ordering::Relaxed);
        if now.saturating_duration_since(last_report) >= Duration::from_secs(10) {
            last_report = now;
            if state.active_session.is_some() {
                let total = metrics.total_frames();
                eprintln!(
                    "receive: jitter={} blocks total={:.1} ms ratio={:.6} playout={:.6} missing_input={} lost={} skipped={} queue_evicted={}",
                    metrics.occupancy(),
                    frames_duration(total).as_secs_f64() * 1_000.,
                    ratio.get(),
                    state.playout_control.ratio(),
                    playback.missing_input_frames.load(Ordering::Relaxed),
                    metrics.packets_lost(),
                    metrics.local_skipped_blocks(),
                    metrics.queue_evicted_blocks(),
                );
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn spawn_worker<F>(name: &str, run: F) -> Result<WorkerHandle, ApiError>
where
    F: FnOnce(Arc<AtomicBool>) -> Result<(), ApiError> + Send + 'static,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = cancelled.clone();
    let (errors_tx, errors) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let result = run(thread_cancelled);
            if let Err(err) = &result {
                let _ = errors_tx.try_send(err.clone());
            }
            result
        })
        .map_err(|err| failed(format!("cannot spawn {name}: {err}")))?;
    Ok(WorkerHandle {
        cancelled,
        errors,
        thread,
    })
}

fn random_session_id() -> Result<u64, ApiError> {
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes)
            .map_err(|err| failed(format!("cannot generate UDP session ID: {err}")))?;
        let id = u64::from_ne_bytes(bytes);
        if id != 0 {
            return Ok(id);
        }
    }
}

fn ipv4_addr(address: SocketAddr) -> Result<SocketAddrV4, ApiError> {
    match address {
        SocketAddr::V4(address) => Ok(address),
        SocketAddr::V6(_) => Err(failed("UDP worker unexpectedly opened an IPv6 socket")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        AudioBlock, CAPTURE_QUEUE_BLOCKS, PLAYBACK_QUEUE_BLOCKS, SAMPLES_PER_BLOCK,
    };
    use std::sync::atomic::AtomicU32;

    fn packet(session_id: u64, slot: u64) -> [u8; PACKET_LEN] {
        encode(&Packet {
            session_id,
            sequence: slot as u32,
            block: AudioBlock {
                frame_index: slot * FRAMES_PER_BLOCK as u64,
                samples: [slot as i16; SAMPLES_PER_BLOCK],
            },
        })
    }

    fn state(peer_ip: Ipv4Addr, buffer_ms: u16) -> ReceiveState {
        ReceiveState::new(peer_ip, JitterBuffer::new(buffer_ms).unwrap(), buffer_ms)
    }

    fn source(ip: Ipv4Addr) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(ip, 12345))
    }

    fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(POLL_INTERVAL);
        }
        condition()
    }

    #[test]
    fn filters_peer_and_packet_then_replaces_session_only_after_timeout() {
        let peer = Ipv4Addr::LOCALHOST;
        let start = Instant::now();
        let blocks = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let ratio = ConsumptionRatio::default();
        let mut receive = state(peer, 10);

        assert!(!receive.accept(source(Ipv4Addr::new(127, 0, 0, 2)), &packet(1, 0), start));
        assert!(!receive.accept(source(peer), b"malformed", start));
        assert_eq!(receive.active_session, None);
        assert!(receive.accept(source(peer), &packet(1, 0), start));
        assert!(!receive.accept(source(peer), &packet(2, 1), start));
        assert!(!receive.accept(source(peer), &packet(1, 0), start));

        blocks.push(AudioBlock {
            frame_index: 99,
            samples: [0; SAMPLES_PER_BLOCK],
        });
        ratio.set(1.001).unwrap();
        receive
            .playout_control
            .update(2_000., start + Duration::from_millis(10));
        receive
            .playout_control
            .update(2_000., start + Duration::from_millis(20));
        receive
            .playback_control
            .update(2_000., start + Duration::from_millis(10));
        receive
            .playback_control
            .update(2_000., start + Duration::from_millis(20));
        assert!(receive.playout_control.ratio() > 1.);
        assert!(receive.playback_control.ratio() > 1.);
        assert!(receive.expire(start + SESSION_IDLE_TIMEOUT, &blocks, &ratio));
        assert_eq!(receive.active_session, None);
        assert!(blocks.is_empty());
        assert_eq!(ratio.get(), 1.0);
        assert_eq!(receive.playout_control.ratio(), 1.0);
        assert_eq!(receive.playback_control.ratio(), 1.0);
        assert!(receive.accept(source(peer), &packet(2, 100), start + SESSION_IDLE_TIMEOUT));
    }

    #[test]
    fn reordered_playout_stays_bounded_and_counts_only_missing_audio() {
        let peer = Ipv4Addr::LOCALHOST;
        let start = Instant::now();
        let blocks = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let mut receive = state(peer, 10);
        for slot in [0, 2, 1, 4, 3, 7, 6] {
            assert!(receive.accept(source(peer), &packet(1, slot), start));
        }
        assert!(!receive.accept(source(peer), &packet(1, 40), start));
        assert!(receive.jitter.occupancy() <= 40);

        assert_eq!(
            receive.playout(start + Duration::from_millis(30), &blocks),
            0
        );
        assert_eq!(blocks.len(), PLAYBACK_QUEUE_BLOCKS);
        assert_eq!(receive.jitter.local_skipped_blocks(), 0);
        assert_eq!(receive.queue_evicted_blocks, 1);
        assert_eq!(
            receive.playout(start + Duration::from_millis(35), &blocks),
            1
        );
        assert_eq!(blocks.len(), PLAYBACK_QUEUE_BLOCKS);
        assert_eq!(receive.queue_evicted_blocks, 2);
        for _ in 1..PLAYBACK_QUEUE_BLOCKS {
            blocks.pop().unwrap();
        }
        let silence = blocks.pop().unwrap();
        assert_eq!(silence.frame_index, 5 * FRAMES_PER_BLOCK as u64);
        assert_eq!(silence.samples, [0; SAMPLES_PER_BLOCK]);
    }

    #[test]
    fn localhost_workers_exchange_pcm_and_stop_cleanly() {
        let playback = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let receiver = start_receiver(
            Ipv4Addr::LOCALHOST,
            0,
            10,
            playback.clone(),
            ConsumptionRatio::default(),
            Arc::new(PlaybackMetrics::default()),
        )
        .unwrap();
        let port = receiver.local_addr.port();
        let capture = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let sender = start_sender(Ipv4Addr::LOCALHOST, port, capture.clone()).unwrap();
        let expected: Vec<_> = (0..3)
            .map(|slot| AudioBlock {
                frame_index: slot * FRAMES_PER_BLOCK as u64,
                samples: [slot as i16; SAMPLES_PER_BLOCK],
            })
            .collect();
        for block in &expected {
            capture.push(block.clone());
        }

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut received = Vec::new();
        while received.len() < 3 && Instant::now() < deadline {
            if let Some(block) = playback.pop() {
                received.push(block);
            } else {
                thread::sleep(POLL_INTERVAL);
            }
        }
        assert_eq!(received, expected);
        assert!(sender.metrics.packets_sent() >= 3);
        assert_eq!(receiver.metrics.status(), Status::Receiving);

        receiver.worker.cancel();
        receiver.worker.join().unwrap();

        let restarted_playback = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let restarted = start_receiver(
            Ipv4Addr::LOCALHOST,
            port,
            10,
            restarted_playback.clone(),
            ConsumptionRatio::default(),
            Arc::new(PlaybackMetrics::default()),
        )
        .unwrap();
        let after_restart = AudioBlock {
            frame_index: 8 * FRAMES_PER_BLOCK as u64,
            samples: [123; SAMPLES_PER_BLOCK],
        };
        capture.push(after_restart.clone());
        assert!(wait_until(Duration::from_secs(1), || {
            restarted.metrics.status() == Status::Receiving && !restarted_playback.is_empty()
        }));
        assert_eq!(restarted_playback.pop(), Some(after_restart));

        sender.worker.cancel();
        restarted.worker.cancel();
        sender.worker.join().unwrap();
        restarted.worker.join().unwrap();
    }

    #[test]
    fn worker_ignores_oversized_input_and_reports_idle_session_recovery() {
        let playback = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let receiver = start_receiver(
            Ipv4Addr::LOCALHOST,
            0,
            10,
            playback,
            ConsumptionRatio::default(),
            Arc::new(PlaybackMetrics::default()),
        )
        .unwrap();
        let source = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let destination = SocketAddrV4::new(Ipv4Addr::LOCALHOST, receiver.local_addr.port());
        source.send_to(&vec![0; 2_000], destination).unwrap();
        for slot in 0..7 {
            source.send_to(&packet(1, slot), destination).unwrap();
        }
        assert!(wait_until(Duration::from_secs(1), || {
            receiver.metrics.status() == Status::Receiving
        }));
        assert!(wait_until(Duration::from_secs(1), || {
            receiver.metrics.queue_evicted_blocks() >= 3
        }));
        assert!(wait_until(Duration::from_secs(2), || {
            receiver.metrics.status() == Status::Listening
        }));
        let evicted = receiver.metrics.queue_evicted_blocks();
        assert!(evicted >= 3);

        source.send_to(&packet(2, 100), destination).unwrap();
        assert!(wait_until(Duration::from_secs(1), || {
            receiver.metrics.status() == Status::Receiving
        }));
        assert_eq!(receiver.metrics.queue_evicted_blocks(), evicted);
        assert!(receiver.worker.errors.try_recv().is_err());
        receiver.worker.cancel();
        receiver.worker.join().unwrap();
    }

    #[test]
    fn unavailable_destination_keeps_backlog_bounded_and_cancels_promptly() {
        let capture = BlockQueue::new(CAPTURE_QUEUE_BLOCKS);
        let sender = start_sender(Ipv4Addr::LOCALHOST, 9, capture.clone()).unwrap();
        for slot in 0..100 {
            capture.push(AudioBlock {
                frame_index: slot * FRAMES_PER_BLOCK as u64,
                samples: [0; SAMPLES_PER_BLOCK],
            });
        }
        assert!(capture.len() <= CAPTURE_QUEUE_BLOCKS);
        assert!(wait_until(Duration::from_secs(1), || {
            sender.metrics.packets_sent() > 0
        }));
        let stopped = Instant::now();
        sender.worker.cancel();
        sender.worker.join().unwrap();
        assert!(stopped.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn receiver_reports_port_conflict() {
        // Match the receiver's wildcard bind; Windows permits some distinct
        // specific-address/wildcard bindings to coexist on the same port.
        let occupied = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let error = start_receiver(
            Ipv4Addr::LOCALHOST,
            port,
            10,
            BlockQueue::new(PLAYBACK_QUEUE_BLOCKS),
            ConsumptionRatio::default(),
            Arc::new(PlaybackMetrics::default()),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::WorkerFailed);
        assert!(error.message.contains("cannot bind UDP receiver port"));
    }

    // 5 ms virtual ticks, 0/2/7 ms deterministic delivery jitter, one dropped
    // packet per 997, with a separate +/-100 ppm simulated playback clock.
    // All receiver operations below are the production accept/playout/control
    // path; only sockets, WASAPI and the PCM conversion are modeled.
    fn simulate(sender_ppm: f64, reverse: bool) {
        let start = Instant::now();
        let blocks = BlockQueue::new(PLAYBACK_QUEUE_BLOCKS);
        let ratio = ConsumptionRatio::default();
        let playback = PlaybackMetrics {
            output_rate: AtomicU32::new(48_000),
            padding_frames: AtomicU32::new(240),
            resampler_delay_frames: AtomicU32::new(64),
            ..PlaybackMetrics::default()
        };
        let mut receiver = state(Ipv4Addr::LOCALHOST, 30);
        let mut packets = Vec::new();
        let mut next_sender = 0.; // nanoseconds on the wall clock
        let mut next_playback = 0.;
        let mut slot = 0u64;
        let mut pending_frames = 0f64;
        let mut missing = 0f64;
        let mut loss = 0u64;
        let mut maximum = 0f64;
        let mut midpoint_ratio = 1.;
        let mut final_ratio = 1.;
        let mut midpoint_playout = 1.;
        let mut final_playout = 1.;
        let mut early_total = 0.;
        let mut late_total = 0.;
        let mut late_missing_start = 0.;
        let mut late_evictions_start = 0;
        for tick in 0..720_000u64 {
            let now_ns = tick * 5_000_000;
            let now = start + Duration::from_nanos(now_ns);
            let rate = if reverse && tick >= 360_000 {
                -sender_ppm
            } else {
                sender_ppm
            };
            while next_sender <= now_ns as f64 {
                if !slot.is_multiple_of(997) || slot == 0 {
                    packets.push((
                        next_sender as u64 + [0, 2, 7, 1][slot as usize % 4] * 1_000_000,
                        slot,
                    ));
                }
                slot += 1;
                next_sender += 5_000_000. / (1. + rate / 1_000_000.);
            }
            let mut index = 0;
            while index < packets.len() {
                if packets[index].0 <= now_ns {
                    let (_, slot) = packets.swap_remove(index);
                    receiver.accept(source(Ipv4Addr::LOCALHOST), &packet(1, slot), now);
                } else {
                    index += 1;
                }
            }
            assert!(
                !receiver.expire(now, &blocks, &ratio),
                "stream timed out at {tick}"
            );
            loss += receiver.playout(now, &blocks);
            while next_playback <= now_ns as f64 {
                let required = 240. * ratio.get() as f64;
                while pending_frames < required {
                    if blocks.pop().is_some() {
                        pending_frames += 240.;
                    } else {
                        break;
                    }
                }
                if pending_frames < required {
                    missing += required - pending_frames;
                    pending_frames = 0.;
                } else {
                    pending_frames -= required;
                }
                next_playback += 5_000_000. / (1. - 100. / 1_000_000.);
            }
            playback
                .pending_input_frames
                .store(pending_frames as u32, Ordering::Relaxed);
            let total = receiver.correct(now, &blocks, &ratio, &playback);
            if tick > 12_000 {
                maximum = maximum.max(total.unwrap());
            }
            if (60_000..120_000).contains(&tick) {
                early_total += total.unwrap();
            }
            if tick >= 660_000 {
                late_total += total.unwrap();
                if tick == 660_000 {
                    late_missing_start = missing;
                    late_evictions_start = receiver.queue_evicted_blocks;
                }
            }
            if tick == 340_000 {
                midpoint_ratio = ratio.get();
                midpoint_playout = receiver.playout_control.ratio();
            }
            if tick == 700_000 {
                final_ratio = ratio.get();
                final_playout = receiver.playout_control.ratio();
            }
            assert!(receiver.jitter.occupancy() <= 40);
            assert!(blocks.len() <= PLAYBACK_QUEUE_BLOCKS);
        }
        // At a -100 ppm playback clock the expected correction is sender ppm
        // minus playback ppm. Reversal must cross nominal in the other direction.
        let expected_first = sender_ppm + 100.;
        let expected_last = if reverse {
            -sender_ppm + 100.
        } else {
            expected_first
        };
        assert!(
            (midpoint_ratio as f64 - (1. + expected_first / 1e6)).abs() < 0.0005,
            "midpoint ratio={midpoint_ratio} expected ppm={expected_first}"
        );
        assert!(
            (final_ratio as f64 - (1. + expected_last / 1e6)).abs() < 0.0005,
            "final ratio={final_ratio} expected ppm={expected_last}"
        );
        assert_eq!(midpoint_ratio > 1., expected_first > 0.);
        assert_eq!(final_ratio > 1., expected_last > 0.);
        assert_eq!(midpoint_playout > 1., sender_ppm > 0.);
        assert_eq!(
            final_playout > 1.,
            (if reverse { -sender_ppm } else { sender_ppm }) > 0.
        );
        assert!(maximum < 4_000., "total occupancy grew to {maximum} frames");
        let early_average = early_total / 60_000.;
        let late_average = late_total / 60_000.;
        let target = 30. * 48. + 2. * 240. + 240. + 64.;
        for average in [early_average, late_average] {
            assert!(
                (average - target).abs() < 480.,
                "occupancy {average} frames outside target {target} +/- 480"
            );
        }
        assert!(
            (late_average - early_average).abs() < 480.,
            "occupancy trend: {early_average} to {late_average} frames"
        );
        assert!(loss > 100, "injected packet losses must be visible");
        assert!(missing < 25_000., "playback underrun {missing} frames");
        // Do not hide recurring underruns behind the startup underflow count.
        assert!(
            missing - late_missing_start < 2_400.,
            "underruns continued after settling: {missing} total"
        );
        assert!(receiver.jitter.local_skipped_blocks() < 10);
        assert!(
            receiver.queue_evicted_blocks - late_evictions_start < 10,
            "playback queue kept evicting: {} total",
            receiver.queue_evicted_blocks
        );
        // The real receiver checks idle timeout before catch-up. Old audio and
        // controller history must be discarded, while eviction counts survive.
        let stalled = start + Duration::from_secs(3_700);
        let evictions_before_reset = receiver.queue_evicted_blocks;
        assert!(receiver.expire(stalled, &blocks, &ratio));
        assert!(blocks.is_empty());
        assert_eq!(receiver.jitter.occupancy(), 0);
        assert_eq!(receiver.media_clock, None);
        assert_eq!(receiver.queue_evicted_blocks, evictions_before_reset);
        assert_eq!(ratio.get(), 1.);
        assert_eq!(receiver.playout_control.ratio(), 1.);
        assert_eq!(receiver.playback_control.ratio(), 1.);
        assert_eq!(receiver.playout(stalled, &blocks), 0);
        assert!(receiver.accept(source(Ipv4Addr::LOCALHOST), &packet(2, 800_000), stalled));
        assert_eq!(receiver.correct(stalled, &blocks, &ratio, &playback), None);
        assert_eq!(
            receiver.playout(stalled + Duration::from_millis(29), &blocks),
            0
        );
        assert_eq!(receiver.jitter.occupancy(), 1);
        assert_eq!(ratio.get(), 1.);
    }

    #[test]
    fn drift_hour_faster_sender() {
        simulate(200., false);
    }

    #[test]
    fn drift_hour_slower_sender() {
        simulate(-200., false);
    }

    #[test]
    fn drift_direction_reversal() {
        simulate(200., true);
    }
}
