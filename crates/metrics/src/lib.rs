pub mod histogram;
pub mod registry;

pub use histogram::{LatencyRecorder, Stage};
pub use registry::init_prometheus;
