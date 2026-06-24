pub mod histogram;
pub mod registry;

pub use histogram::{LatencyRecorder, Stage};
pub use registry::{
    init_prometheus,
    inc_orders, inc_trades, inc_rejects,
    set_book_depth, set_spread, set_ring_occupancy,
};
