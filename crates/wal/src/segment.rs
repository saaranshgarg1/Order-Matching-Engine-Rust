use std::path::{Path, PathBuf};
use std::fs;

pub const SEGMENT_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB per segment

/// Parse segment index from filename "0000000001.wal" → 1.
pub fn segment_index(path: &Path) -> Option<u64> {
    path.file_stem()?
        .to_str()?
        .parse::<u64>()
        .ok()
}

/// Filename for a given segment index.
pub fn segment_path(dir: &Path, idx: u64) -> PathBuf {
    dir.join(format!("{:010}.wal", idx))
}

/// Return all existing segment paths in ascending order.
pub fn list_segments(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut segs: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wal"))
        .collect();
    segs.sort();
    Ok(segs)
}
