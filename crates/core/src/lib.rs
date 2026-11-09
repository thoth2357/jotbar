//! Shared note storage, hashing and the sync protocol for jotbar.

pub mod proto;
pub mod remote;
pub mod store;
pub mod sync;

pub use proto::*;
pub use store::{NoteMeta, SearchHit, Store};

use sha2::{Digest, Sha256};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Content hash used to detect changes. A deleted note always hashes the same,
/// so two devices that both deleted a note agree without a conflict.
pub fn content_hash(content: &str, deleted: bool) -> String {
    let mut h = Sha256::new();
    if deleted {
        h.update(b"\0deleted");
    } else {
        h.update(content.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Note ids become file names, so only allow a safe character set.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Write a file atomically (temp file + rename) so a crash never leaves half a note.
pub fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}
