//! Bounded UDP sender and receiver workers.
use crate::audio::{BlockQueue, ConsumptionRatio, WorkerHandle, FRAMES_PER_BLOCK};
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
    });
    let thread_metrics = metrics.clone();
    let worker = spawn_worker("audioportal-udp-receiver", move |cancelled| {
        let result = run_receiver(
            socket,
            peer_ip,
            blocks,
            ratio,
            jitter,
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
}

impl ReceiveState {
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
        if !self.jitter.insert(packet.block, now) {
            return false;
        }
        self.active_session = Some(packet.session_id);
        self.last_accepted = Some(now);
        true
    }

    fn playout(&mut self, now: Instant, blocks: &BlockQueue) -> u64 {
        let mut lost = 0u64;
        while let Some((block, silence)) = self.jitter.poll(now) {
            blocks.push(block);
            lost += u64::from(silence);
        }
        lost
    }
}

fn run_receiver(
    socket: UdpSocket,
    peer_ip: Ipv4Addr,
    blocks: BlockQueue,
    ratio: ConsumptionRatio,
    jitter: JitterBuffer,
    metrics: &ReceiverMetrics,
    cancelled: &AtomicBool,
) -> Result<(), ApiError> {
    let mut state = ReceiveState {
        peer_ip,
        active_session: None,
        last_accepted: None,
        jitter,
    };
    // A full UDP payload buffer prevents oversized datagrams from becoming
    // WSAEMSGSIZE worker failures on Windows; decode rejects non-exact lengths.
    let mut bytes = vec![0; u16::MAX as usize];
    while !cancelled.load(Ordering::Acquire) {
        let mut now = Instant::now();
        if state.expire(now, &blocks, &ratio) {
            metrics.receiving.store(false, Ordering::Relaxed);
        }

        for _ in 0..DATAGRAMS_PER_TICK {
            match socket.recv_from(&mut bytes) {
                Ok((length, source)) => {
                    now = Instant::now();
                    if state.expire(now, &blocks, &ratio) {
                        metrics.receiving.store(false, Ordering::Relaxed);
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
        if state.expire(now, &blocks, &ratio) {
            metrics.receiving.store(false, Ordering::Relaxed);
        } else {
            let lost = state.playout(now, &blocks);
            metrics.packets_lost.fetch_add(lost, Ordering::Relaxed);
        }
        metrics
            .occupancy
            .store(state.jitter.occupancy(), Ordering::Relaxed);
        metrics
            .local_skipped_blocks
            .store(state.jitter.local_skipped_blocks(), Ordering::Relaxed);
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
        ReceiveState {
            peer_ip,
            active_session: None,
            last_accepted: None,
            jitter: JitterBuffer::new(buffer_ms).unwrap(),
        }
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
        assert!(receive.expire(start + SESSION_IDLE_TIMEOUT, &blocks, &ratio));
        assert_eq!(receive.active_session, None);
        assert!(blocks.is_empty());
        assert_eq!(ratio.get(), 1.0);
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
        assert_eq!(
            receive.playout(start + Duration::from_millis(35), &blocks),
            1
        );
        assert_eq!(blocks.len(), PLAYBACK_QUEUE_BLOCKS);
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
        )
        .unwrap();
        let source = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let destination = SocketAddrV4::new(Ipv4Addr::LOCALHOST, receiver.local_addr.port());
        source.send_to(&vec![0; 2_000], destination).unwrap();
        source.send_to(&packet(1, 0), destination).unwrap();
        assert!(wait_until(Duration::from_secs(1), || {
            receiver.metrics.status() == Status::Receiving
        }));
        assert!(wait_until(Duration::from_secs(2), || {
            receiver.metrics.status() == Status::Listening
        }));

        source.send_to(&packet(2, 100), destination).unwrap();
        assert!(wait_until(Duration::from_secs(1), || {
            receiver.metrics.status() == Status::Receiving
        }));
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
        let occupied = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let error = start_receiver(
            Ipv4Addr::LOCALHOST,
            port,
            10,
            BlockQueue::new(PLAYBACK_QUEUE_BLOCKS),
            ConsumptionRatio::default(),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::WorkerFailed);
        assert!(error.message.contains("cannot bind UDP receiver port"));
    }
}
