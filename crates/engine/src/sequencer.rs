use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Global monotonic sequence number backed by a shared epoch Instant.
/// Both the sequencer and the shard threads use the same Arc<Instant> so
/// cmd.ts and match_ts are directly comparable nanosecond offsets.
pub struct Sequencer {
    next:  AtomicU64,
    epoch: Arc<Instant>,
}

impl Sequencer {
    pub fn new() -> Self {
        Self::with_epoch(Arc::new(Instant::now()))
    }

    pub fn with_epoch(epoch: Arc<Instant>) -> Self {
        Sequencer { next: AtomicU64::new(1), epoch }
    }

    /// Returns (seq, ts_nanos_from_shared_epoch).
    #[inline]
    pub fn next(&self) -> (u64, u64) {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let ts  = self.epoch.elapsed().as_nanos() as u64;
        (seq, ts)
    }

    pub fn current(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }
}

impl Default for Sequencer {
    fn default() -> Self { Self::new() }
}
