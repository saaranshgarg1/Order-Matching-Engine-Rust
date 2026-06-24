use std::fs;
use std::path::Path;

use exchange_core::Seq;
use crate::record::WalRecord;
use crate::segment::list_segments;
use crate::WalError;

pub struct WalReader {
    dir: std::path::PathBuf,
}

impl WalReader {
    pub fn new(dir: &Path) -> Self {
        WalReader { dir: dir.to_path_buf() }
    }

    /// Replay all records with seq > after_seq, calling `f` for each.
    /// Stops on a truncated final record (torn write); errors on CRC mismatch.
    pub fn replay<F>(&self, after_seq: Seq, mut f: F) -> Result<u64, WalError>
    where
        F: FnMut(WalRecord) -> Result<(), WalError>,
    {
        let mut last_seq = after_seq;
        let segments = list_segments(&self.dir)?;

        for seg_path in &segments {
            let data = fs::read(seg_path)?;
            let mut offset = 0usize;

            loop {
                if offset >= data.len() { break; }

                match WalRecord::decode(&data[offset..]) {
                    Ok((rec, consumed)) => {
                        offset += consumed;
                        if rec.seq > after_seq {
                            last_seq = rec.seq;
                            f(rec)?;
                        }
                    }
                    Err(WalError::Truncated(_)) => {
                        // Torn write at end of segment — safe to stop.
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(last_seq)
    }

    /// Count total records (for diagnostics).
    pub fn count(&self) -> Result<usize, WalError> {
        let mut n = 0;
        self.replay(0, |_| { n += 1; Ok(()) })?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{FsyncPolicy, WalWriter};
    use crate::record::RecordType;
    use tempfile::tempdir;

    #[test]
    fn write_then_replay() {
        let dir = tempdir().unwrap();
        let mut w = WalWriter::open(dir.path(), FsyncPolicy::Off).unwrap();
        for i in 1u64..=10 {
            w.append(i, i * 100, RecordType::Command, format!("cmd{}", i).into_bytes()).unwrap();
        }
        w.flush().unwrap();

        let r = WalReader::new(dir.path());
        let mut replayed = vec![];
        r.replay(0, |rec| { replayed.push(rec.seq); Ok(()) }).unwrap();
        assert_eq!(replayed, (1u64..=10).collect::<Vec<_>>());
    }

    #[test]
    fn replay_after_seq_skips_earlier() {
        let dir = tempdir().unwrap();
        let mut w = WalWriter::open(dir.path(), FsyncPolicy::Off).unwrap();
        for i in 1u64..=10 {
            w.append(i, 0, RecordType::Command, vec![]).unwrap();
        }
        w.flush().unwrap();

        let r = WalReader::new(dir.path());
        let mut replayed = vec![];
        r.replay(5, |rec| { replayed.push(rec.seq); Ok(()) }).unwrap();
        assert_eq!(replayed, vec![6u64, 7, 8, 9, 10]);
    }
}
