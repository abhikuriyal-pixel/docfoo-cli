//! Small filesystem helpers shared by config, KG, backup and collections.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    let tmp = PathBuf::from(tmp);
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

/// Unix milliseconds, or 0 before the epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Non-fatal failure report (the CLI has no UI to surface these).
pub fn report(context: &str, error: impl std::fmt::Display) {
    eprintln!("warning: {context}: {error}");
}

/// True when `rel` is a safe relative path: non-empty, no `..`/`.` segments,
/// no absolute/prefix components, no NUL or drive colon.
pub fn is_safe_rel(rel: &str) -> bool {
    if rel.is_empty() || rel.len() > 1024 || rel.contains('\0') || rel.contains(':') {
        return false;
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return false;
    }
    if rel
        .split(|c| c == '/' || c == '\\')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return false;
    }
    let path = Path::new(rel);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Sanitize a file/folder name for a collection install.
pub fn sanitize_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim().to_string();
    if sanitized.is_empty() {
        "shared".to_string()
    } else {
        sanitized
    }
}

// ---------------------------------------------------------------------------
// Archive extraction quotas (zip-bomb defense)
// ---------------------------------------------------------------------------

/// Limits and running counters for archive extraction.
pub struct Quota {
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
    /// Declared-size / compressed-size ceiling, applied only at or above
    /// `ratio_min_bytes` so tiny highly-compressible files stay allowed.
    pub max_ratio: u64,
    pub ratio_min_bytes: u64,
}

#[derive(Default)]
pub struct QuotaState {
    pub entries: usize,
    pub bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QuotaError {
    TooManyEntries,
    EntryTooLarge,
    TotalTooLarge,
    CompressionRatio,
}

impl Quota {
    /// Account for one archive entry. `declared` is the uncompressed size from
    /// the archive directory, `compressed` its stored size.
    pub fn check(
        &self,
        state: &mut QuotaState,
        declared: u64,
        compressed: u64,
    ) -> std::result::Result<(), QuotaError> {
        state.entries += 1;
        if state.entries > self.max_entries {
            return Err(QuotaError::TooManyEntries);
        }
        if declared > self.max_entry_bytes {
            return Err(QuotaError::EntryTooLarge);
        }
        if declared >= self.ratio_min_bytes && compressed > 0 && declared / compressed > self.max_ratio
        {
            return Err(QuotaError::CompressionRatio);
        }
        state.bytes = state.bytes.saturating_add(declared);
        if state.bytes > self.max_total_bytes {
            return Err(QuotaError::TotalTooLarge);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_rejects_escapes() {
        assert!(is_safe_rel("Book/content.md"));
        assert!(is_safe_rel("a/b/c.png"));
        assert!(!is_safe_rel(""));
        assert!(!is_safe_rel("../etc"));
        assert!(!is_safe_rel("a/../b"));
        assert!(!is_safe_rel("/abs"));
        assert!(!is_safe_rel("C:\\evil"));
        assert!(!is_safe_rel("a//b"));
    }

    #[test]
    fn quota_rejects_oversized_entries() {
        let quota = Quota {
            max_entries: 3,
            max_entry_bytes: 10,
            max_total_bytes: 15,
            max_ratio: 1000,
            ratio_min_bytes: 8,
        };
        let mut state = QuotaState::default();
        assert!(quota.check(&mut state, 5, 5).is_ok());
        assert_eq!(quota.check(&mut state, 11, 5), Err(QuotaError::EntryTooLarge));
        assert_eq!(quota.check(&mut state, 5, 5), Ok(()));
        assert_eq!(quota.check(&mut state, 5, 5), Err(QuotaError::TooManyEntries));
    }
}
