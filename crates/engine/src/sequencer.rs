use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Global monotonic sequence number. One per engine.
pub struct Sequencer {
    next: AtomicU64,
    start: Instant,
}

impl Sequencer {
    pub fn new() -> Self {
        Sequencer { next: AtomicU64::new(1), start: Instant::now() }
    }

    /// Atomically fetch-and-increment. Returns (seq, ts_nanos).
    #[inline]
    pub fn next(&self) -> (u64, u64) {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let ts  = self.start.elapsed().as_nanos() as u64;
        (seq, ts)
    }

    pub fn current(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }
}

impl Default for Sequencer {
    fn default() -> Self { Self::new() }
}
