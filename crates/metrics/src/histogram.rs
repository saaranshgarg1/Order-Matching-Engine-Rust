use hdrhistogram::Histogram;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    IngressToMatch,
    MatchToAck,
    EndToEnd,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::IngressToMatch => "ingress_to_match",
            Stage::MatchToAck     => "match_to_ack",
            Stage::EndToEnd       => "end_to_end",
        }
    }
}

/// Thread-safe latency recorder backed by HdrHistogram.
/// Record values in nanoseconds; report in microseconds.
#[derive(Clone)]
pub struct LatencyRecorder {
    inner: Arc<Mutex<Histogram<u64>>>,
    stage: Stage,
}

impl LatencyRecorder {
    /// `sigfig` significant figures (3 is typical: p99.9 accurate).
    pub fn new(stage: Stage) -> Self {
        let hist = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3)
            .expect("valid histogram bounds");
        LatencyRecorder { inner: Arc::new(Mutex::new(hist)), stage }
    }

    /// Record one latency sample in nanoseconds.
    #[inline]
    pub fn record_ns(&self, ns: u64) {
        if let Ok(mut h) = self.inner.lock() {
            let _ = h.record(ns);
        }
    }

    pub fn stage(&self) -> Stage { self.stage }

    /// Returns (p50, p99, p999, max) in nanoseconds.
    pub fn percentiles(&self) -> (u64, u64, u64, u64) {
        let h = self.inner.lock().unwrap();
        (
            h.value_at_percentile(50.0),
            h.value_at_percentile(99.0),
            h.value_at_percentile(99.9),
            h.max(),
        )
    }

    /// Reset histogram (e.g. per reporting period).
    pub fn reset(&self) {
        if let Ok(mut h) = self.inner.lock() {
            h.reset();
        }
    }

    pub fn print_report(&self) {
        let (p50, p99, p999, max) = self.percentiles();
        println!(
            "[latency:{}] p50={:.1}µs  p99={:.1}µs  p999={:.1}µs  max={:.1}µs",
            self.stage.name(),
            p50  as f64 / 1_000.0,
            p99  as f64 / 1_000.0,
            p999 as f64 / 1_000.0,
            max  as f64 / 1_000.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reports() {
        let r = LatencyRecorder::new(Stage::EndToEnd);
        for i in 1_000..=10_000u64 {
            r.record_ns(i);
        }
        let (p50, p99, _, max) = r.percentiles();
        assert!(p50 > 0);
        assert!(p99 >= p50);
        assert!(max >= p99);
    }
}
