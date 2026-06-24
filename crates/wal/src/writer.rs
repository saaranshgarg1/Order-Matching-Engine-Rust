use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use exchange_core::{Seq, Ts};
use crate::record::{RecordType, WalRecord};
use crate::segment::{segment_path, SEGMENT_SIZE};
use crate::WalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// fsync after every record (safest, slowest).
    PerRecord,
    /// fsync every N records.
    Every(usize),
    /// No fsync (benchmarks / tests only).
    Off,
}

pub struct WalWriter {
    dir:         PathBuf,
    seg_idx:     u64,
    seg_bytes:   u64,
    file:        BufWriter<File>,
    fsync:       FsyncPolicy,
    write_count: usize,
}

impl WalWriter {
    pub fn open(dir: &Path, fsync: FsyncPolicy) -> Result<Self, WalError> {
        fs::create_dir_all(dir)?;

        // Find the highest existing segment, or start at 1.
        let segs = crate::segment::list_segments(dir)?;
        let seg_idx = segs.last()
            .and_then(|p| crate::segment::segment_index(p))
            .unwrap_or(0) + 1;

        let path = segment_path(dir, seg_idx);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let seg_bytes = file.metadata()?.len();

        Ok(WalWriter {
            dir: dir.to_path_buf(),
            seg_idx,
            seg_bytes,
            file: BufWriter::new(file),
            fsync,
            write_count: 0,
        })
    }

    /// Append one WAL record. Returns the seq written.
    pub fn append(
        &mut self,
        seq: Seq,
        ts: Ts,
        rec_type: RecordType,
        payload: Vec<u8>,
    ) -> Result<(), WalError> {
        let rec = WalRecord::new(seq, ts, rec_type, payload);
        let bytes = rec.encode();

        self.file.write_all(&bytes)?;
        self.seg_bytes += bytes.len() as u64;
        self.write_count += 1;

        match self.fsync {
            FsyncPolicy::PerRecord => self.do_fsync()?,
            FsyncPolicy::Every(n) if self.write_count % n == 0 => self.do_fsync()?,
            _ => {}
        }

        if self.seg_bytes >= SEGMENT_SIZE {
            self.roll_segment()?;
        }

        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        Ok(())
    }

    fn do_fsync(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.get_ref().sync_data()?;
        Ok(())
    }

    fn roll_segment(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.seg_idx += 1;
        self.seg_bytes = 0;
        let path = segment_path(&self.dir, self.seg_idx);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.file = BufWriter::new(file);
        Ok(())
    }
}
