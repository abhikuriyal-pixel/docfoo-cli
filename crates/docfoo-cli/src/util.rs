//! Small filesystem helpers shared by config, KG and later stages.

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

/// Write `bytes` to `path` via a temporary sibling + rename. On Windows the
/// destination is removed first because `rename` refuses to replace.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Pretty-print `value` (with a trailing newline) and write it atomically.
pub fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}
