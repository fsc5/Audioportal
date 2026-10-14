//! Fixed-size UDP audio packet encoding and validation.
use crate::audio::{AudioBlock, SAMPLES_PER_BLOCK};

pub const HEADER_LEN: usize = 28;
pub const PAYLOAD_LEN: usize = SAMPLES_PER_BLOCK * 2;
pub const PACKET_LEN: usize = HEADER_LEN + PAYLOAD_LEN;

const MAGIC: &[u8; 4] = b"APRT";
const VERSION: u8 = 1;

/// A decoded packet whose wire-format invariants have been validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub session_id: u64,
    pub sequence: u32,
    pub block: AudioBlock,
}

/// Why a datagram could not be decoded as an audio packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketError {
    InvalidLength { actual: usize },
    InvalidMagic,
    UnsupportedVersion(u8),
    NonzeroReserved,
    InvalidHeaderLength(u16),
    ZeroSessionId,
    MisalignedFrameIndex,
    SequenceTimelineMismatch,
}

/// Encodes a packet into the fixed 988-byte wire representation.
///
/// The packet's session and timeline fields are written as supplied; decode
/// validates those fields before accepting a datagram.
pub fn encode(packet: &Packet) -> [u8; PACKET_LEN] {
    let mut bytes = [0; PACKET_LEN];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = VERSION;
    bytes[6..8].copy_from_slice(&(HEADER_LEN as u16).to_be_bytes());
    bytes[8..16].copy_from_slice(&packet.session_id.to_be_bytes());
    bytes[16..20].copy_from_slice(&packet.sequence.to_be_bytes());
    bytes[20..28].copy_from_slice(&packet.block.frame_index.to_be_bytes());

    for (index, sample) in packet.block.samples.iter().enumerate() {
        let offset = HEADER_LEN + index * 2;
        bytes[offset..offset + 2].copy_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Decodes and validates one exact-size wire datagram without allocating.
pub fn decode(bytes: &[u8]) -> Result<Packet, PacketError> {
    if bytes.len() != PACKET_LEN {
        return Err(PacketError::InvalidLength {
            actual: bytes.len(),
        });
    }
    if &bytes[..4] != MAGIC {
        return Err(PacketError::InvalidMagic);
    }
    if bytes[4] != VERSION {
        return Err(PacketError::UnsupportedVersion(bytes[4]));
    }
    if bytes[5] != 0 {
        return Err(PacketError::NonzeroReserved);
    }

    let header_len = u16::from_be_bytes([bytes[6], bytes[7]]);
    if header_len != HEADER_LEN as u16 {
        return Err(PacketError::InvalidHeaderLength(header_len));
    }

    let session_id = u64::from_be_bytes(bytes[8..16].try_into().expect("fixed packet length"));
    if session_id == 0 {
        return Err(PacketError::ZeroSessionId);
    }
    let sequence = u32::from_be_bytes(bytes[16..20].try_into().expect("fixed packet length"));
    let frame_index = u64::from_be_bytes(bytes[20..28].try_into().expect("fixed packet length"));
    if frame_index % 240 != 0 {
        return Err(PacketError::MisalignedFrameIndex);
    }
    if (frame_index / 240) as u32 != sequence {
        return Err(PacketError::SequenceTimelineMismatch);
    }

    let mut samples = [0; SAMPLES_PER_BLOCK];
    for (index, sample) in samples.iter_mut().enumerate() {
        let offset = HEADER_LEN + index * 2;
        *sample = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    }

    Ok(Packet {
        session_id,
        sequence,
        block: AudioBlock {
            frame_index,
            samples,
        },
    })
}

/// Returns whether `a` is newer than `b` under wrapping 32-bit ordering.
pub const fn sequence_is_newer(a: u32, b: u32) -> bool {
    let delta = a.wrapping_sub(b);
    delta != 0 && delta < (1 << 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> Packet {
        let mut samples = [0; SAMPLES_PER_BLOCK];
        samples[0] = i16::MIN;
        samples[1] = i16::MAX;
        samples[SAMPLES_PER_BLOCK - 1] = -1;
        Packet {
            session_id: 0x0102_0304_0506_0708,
            sequence: 1,
            block: AudioBlock {
                frame_index: 240,
                samples,
            },
        }
    }

    #[test]
    fn matches_known_wire_vector_and_decodes_it() {
        let packet = packet();
        let encoded = encode(&packet);

        let mut known = [0; PACKET_LEN];
        known[..28].copy_from_slice(&[
            b'A', b'P', b'R', b'T', 1, 0, 0, 28, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0,
            0, 0, 240,
        ]);
        known[28..32].copy_from_slice(&[0, 128, 255, 127]);
        known[PACKET_LEN - 2..].copy_from_slice(&[255, 255]);

        assert_eq!(encoded, known);
        assert_eq!(decode(&known), Ok(packet));
    }

    #[test]
    fn rejects_invalid_datagrams() {
        let valid = encode(&packet());
        assert_eq!(
            decode(&valid[..PACKET_LEN - 1]),
            Err(PacketError::InvalidLength {
                actual: PACKET_LEN - 1
            })
        );
        let mut oversized = valid.to_vec();
        oversized.push(0);
        assert_eq!(
            decode(&oversized),
            Err(PacketError::InvalidLength {
                actual: PACKET_LEN + 1
            })
        );

        for (index, value, error) in [
            (0, b'X', PacketError::InvalidMagic),
            (4, 2, PacketError::UnsupportedVersion(2)),
            (5, 1, PacketError::NonzeroReserved),
        ] {
            let mut invalid = valid;
            invalid[index] = value;
            assert_eq!(decode(&invalid), Err(error));
        }

        let mut invalid = valid;
        invalid[6..8].copy_from_slice(&27u16.to_be_bytes());
        assert_eq!(decode(&invalid), Err(PacketError::InvalidHeaderLength(27)));
        let mut invalid = valid;
        invalid[8..16].fill(0);
        assert_eq!(decode(&invalid), Err(PacketError::ZeroSessionId));
        let mut invalid = valid;
        invalid[27] = 241;
        assert_eq!(decode(&invalid), Err(PacketError::MisalignedFrameIndex));
        let mut invalid = valid;
        invalid[19] = 2;
        assert_eq!(decode(&invalid), Err(PacketError::SequenceTimelineMismatch));
    }

    #[test]
    fn sequence_ordering_handles_wrap_and_ambiguity() {
        assert!(sequence_is_newer(0, u32::MAX));
        assert!(sequence_is_newer(11, 10));
        assert!(!sequence_is_newer(10, 10));
        assert!(!sequence_is_newer(10, 11));
        assert!(!sequence_is_newer(0x8000_0000, 0));
    }
}
