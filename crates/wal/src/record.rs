use crc32fast::Hasher;
use exchange_core::{Seq, Ts};
use crate::WalError;

/// Record type tag written in the WAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordType {
    Command = 1,
    Event   = 2,
    Snap    = 3,
}

impl RecordType {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(RecordType::Command),
            2 => Some(RecordType::Event),
            3 => Some(RecordType::Snap),
            _ => None,
        }
    }
}

/// One WAL record: framed with len + crc32.
///
/// On-disk layout (all little-endian):
///   4  bytes  payload_len  u32
///   4  bytes  crc32        u32   (crc of everything after this field)
///   8  bytes  seq          u64
///   8  bytes  ts           u64
///   1  byte   rec_type     u8
///   N  bytes  payload
#[derive(Debug, Clone)]
pub struct WalRecord {
    pub seq:      Seq,
    pub ts:       Ts,
    pub rec_type: RecordType,
    pub payload:  Vec<u8>,
}

impl WalRecord {
    pub fn new(seq: Seq, ts: Ts, rec_type: RecordType, payload: Vec<u8>) -> Self {
        WalRecord { seq, ts, rec_type, payload }
    }

    /// Encode to bytes ready for appending to a segment file.
    pub fn encode(&self) -> Vec<u8> {
        // body = seq(8) + ts(8) + rec_type(1) + payload(N)
        let body_len = 8 + 8 + 1 + self.payload.len();
        let mut body = Vec::with_capacity(body_len);
        body.extend_from_slice(&self.seq.to_le_bytes());
        body.extend_from_slice(&self.ts.to_le_bytes());
        body.push(self.rec_type as u8);
        body.extend_from_slice(&self.payload);

        let crc = crc32_of(&body);
        let payload_len = body_len as u32;

        let mut out = Vec::with_capacity(4 + 4 + body_len);
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Decode one record from a byte slice.  Returns (record, bytes_consumed).
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), WalError> {
        if buf.len() < 8 {
            return Err(WalError::Truncated(0));
        }
        let payload_len = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let stored_crc  = u32::from_le_bytes(buf[4..8].try_into().unwrap());

        let total = 8 + payload_len;
        if buf.len() < total {
            return Err(WalError::Truncated(8));
        }

        let body = &buf[8..total];
        let actual_crc = crc32_of(body);
        if actual_crc != stored_crc {
            return Err(WalError::Crc { expected: stored_crc, actual: actual_crc });
        }

        if body.len() < 17 {
            return Err(WalError::Truncated(8));
        }
        let seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let ts  = u64::from_le_bytes(body[8..16].try_into().unwrap());
        let rec_type = RecordType::from_u8(body[16])
            .ok_or_else(|| WalError::Serialise(format!("unknown rec_type {}", body[16])))?;
        let payload = body[17..].to_vec();

        Ok((WalRecord { seq, ts, rec_type, payload }, total))
    }
}

fn crc32_of(data: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let rec = WalRecord::new(42, 1_000_000, RecordType::Command, b"hello".to_vec());
        let bytes = rec.encode();
        let (dec, consumed) = WalRecord::decode(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(dec.seq, 42);
        assert_eq!(dec.ts, 1_000_000);
        assert_eq!(dec.rec_type, RecordType::Command);
        assert_eq!(dec.payload, b"hello");
    }

    #[test]
    fn crc_mismatch_detected() {
        let rec = WalRecord::new(1, 0, RecordType::Event, vec![1, 2, 3]);
        let mut bytes = rec.encode();
        bytes[5] ^= 0xFF; // corrupt crc
        assert!(matches!(WalRecord::decode(&bytes), Err(WalError::Crc { .. })));
    }

    #[test]
    fn truncated_detected() {
        let rec = WalRecord::new(1, 0, RecordType::Event, vec![0; 100]);
        let bytes = rec.encode();
        assert!(matches!(WalRecord::decode(&bytes[..10]), Err(WalError::Truncated(_))));
    }
}
