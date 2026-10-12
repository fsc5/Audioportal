# 07 — LAN sender and receiver workers

Dependencies: 06.

## Goal

Exchange bounded audio streams between configured peers, without owning audio devices or the web API.

## Context and files

Read `specs/contracts.md`, protocol/jitter public APIs, and shared worker lifecycle types.
Own `src/transport.rs` and a small runnable two-process or localhost exercise.

## Work

- Sender accepts captured blocks, adds random session ID/sequence/frame index, and sends to the configured IPv4/port. Bind an ephemeral source port.
- Receiver binds the configured UDP port, filters by peer IP, rejects invalid datagrams, and inserts accepted data into the jitter buffer.
- Implement active-session learning, idle timeout/reset, and session replacement rules exactly as frozen. Receiver must report listening separately from receiving.
- Expose receive/playout operations needed by playback without coupling packet arrival timing to device callbacks.
- Bound all queues; network waits and socket operations stay off audio threads. Preserve timeline gaps on overflow.
- Cancellation closes/unblocks socket workers promptly. Port conflicts and network errors produce actionable results.
- Do not add retransmission, discovery, encryption, or a claim that UDP emission confirms remote connection.

## Acceptance

- Two local processes exchange known PCM and stop cleanly; use ephemeral test ports to avoid environment conflicts.
- Focused checks cover wrong peer, malformed input, active-session rejection, timeout, and new-session recovery.
- Artificial loss/reordering does not stall playout or grow memory.
- Receiver restart and unavailable destination do not cause unbounded sender backlog.

## Handoff

Provide startup/stop signatures, stream state/counters, and timeout behavior for task 11.
