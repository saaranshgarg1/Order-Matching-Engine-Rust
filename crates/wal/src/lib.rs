pub mod record;
pub mod segment;
pub mod writer;
pub mod reader;
pub mod snapshot;

pub use record::{WalRecord, RecordType};
pub use writer::WalWriter;
pub use reader::WalReader;
pub use snapshot::{Snapshot, save_snapshot, load_snapshot};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("crc mismatch: expected {expected:#010x} got {actual:#010x}")]
    Crc { expected: u32, actual: u32 },
    #[error("truncated record at offset {0}")]
    Truncated(u64),
    #[error("serialise error: {0}")]
    Serialise(String),
}
