//! SQLite store for the knowledge graph (KG scale plan).
//!
//! `KgStore` is the on-disk counterpart of [`KnowledgeGraph`]: one `.sqlite`
//! file per graph with entity/section rows, ordered provenance joins, the
//! tokenized corpora and FTS5 tables kept in sync by triggers. Schema v3
//! enforces referential integrity (`ON DELETE CASCADE`, unique relation
//! triples); v6 ranks queries with native FTS5 `bm25()` and carries no
//! persisted BM25 statistics.
//!
//! `write_full` builds a sibling `.tmp` database and renames it over the
//! target, so a crash can never leave a half-written graph. The temp database
//! is created with `journal_mode=OFF` and switched to a durable rollback
//! journal before the rename, leaving no `-wal`/`-shm` sidecars behind.

mod mutate;
mod native;
mod read;
pub mod scene;
mod schema;
mod search;
mod write;

/// Re-exported so shell code can use the same rusqlite version without
/// adding its own dependency (features unify with `bundled` here).
pub use rusqlite;

pub use schema::SCHEMA_VERSION;

pub use native::{SectionInput, StoreCounts, StoredSection, UpsertedEntity};

use crate::graph::KnowledgeGraph;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const META_SCHEMA_VERSION: &str = "schema_version";
pub(crate) const META_BUILD_UID: &str = "build_uid";
pub(crate) const META_DERIVED_READY: &str = "derived_ready";
pub(crate) const META_NOISE_FLOOR: &str = "noise_floor";
pub(crate) const META_ENTITY_COUNT: &str = "entity_count";
pub(crate) const META_RELATION_COUNT: &str = "relation_count";
pub(crate) const META_SECTION_COUNT: &str = "section_count";

/// Memory-mapped page window (256 MB) — the scale-plan default.
pub const MMAP_SIZE: i64 = 268_435_456;
/// Page cache in KiB (negative = KiB, so this is 64 MB).
pub const CACHE_SIZE_KIB: i64 = -65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// Query/read paths: FTS5 and joins only, no journal or sidecars.
    ReadOnly,
    /// Build/sync paths: WAL enabled, schema created if missing.
    ReadWrite,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store schema: {0}")]
    Schema(String),
    #[error("store data: {0}")]
    Data(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// An open knowledge-graph database. A connection can serve one thread; the
/// read paths (chunk 2) open one per command, WAL keeps concurrent readers
/// consistent with a running build.
pub struct KgStore {
    conn: Connection,
    path: PathBuf,
    mode: OpenMode,
    /// Graph-root-relative document prefix applied to `source_doc` reads for
    /// subfolder graphs (replaces the old query-time `qualify_source_docs`).
    source_prefix: String,
}

impl KgStore {
    /// Open `path`. Read-only opens validate the schema version; read-write
    /// opens create the schema and enable WAL.
    pub fn open(path: &Path, mode: OpenMode) -> StoreResult<KgStore> {
        let conn = match mode {
            OpenMode::ReadOnly => {
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
            }
            OpenMode::ReadWrite => Connection::open(path)?,
        };
        conn.execute_batch(&format!(
            "PRAGMA mmap_size={MMAP_SIZE};
             PRAGMA cache_size={CACHE_SIZE_KIB};
             PRAGMA busy_timeout=5000;
             PRAGMA foreign_keys=ON;"
        ))?;
        match mode {
            OpenMode::ReadWrite => {
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
                schema::create(&conn)?;
            }
            OpenMode::ReadOnly => {
                let has_meta: i64 = conn.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'",
                    [],
                    |row| row.get(0),
                )?;
                if has_meta == 0 {
                    return Err(StoreError::Schema(format!(
                        "{}: not a knowledge-graph database",
                        path.display()
                    )));
                }
                let found = read::meta(&conn, META_SCHEMA_VERSION)?;
                let Some(found) = found else {
                    return Err(StoreError::Schema(format!(
                        "{} has no meta.schema_version",
                        path.display()
                    )));
                };
                let found: u32 = found
                    .parse()
                    .map_err(|_| StoreError::Schema(format!("bad schema_version {found:?}")))?;
                if found != SCHEMA_VERSION {
                    return Err(StoreError::Schema(format!(
                        "{}: schema version {found}, expected {SCHEMA_VERSION}",
                        path.display()
                    )));
                }
            }
        }
        // Rust-side lowercasing for guide-fragment resolution: SQLite's
        // built-in lower() is ASCII-only, and titles can be Unicode.
        conn.create_scalar_function(
            "kg_lower",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let value = ctx.get::<Option<String>>(0)?;
                Ok(value.map(|text| text.to_lowercase()))
            },
        )?;
        Ok(KgStore {
            conn,
            path: path.to_path_buf(),
            mode,
            source_prefix: String::new(),
        })
    }

    /// Apply a graph-root-relative prefix to every `source_doc` read (the
    /// shell passes the active graph rel; "" for top-level graphs).
    pub fn with_source_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.source_prefix = prefix.into().trim_matches('/').to_string();
        self
    }

    /// `source_doc` as the workspace sees it: graph-root-relative stored rows
    /// plus the graph dir prefix, idempotently.
    pub(crate) fn prefix_doc(&self, doc: &str) -> String {
        if self.source_prefix.is_empty() || doc.is_empty() {
            return doc.to_string();
        }
        if doc == self.source_prefix
            || doc.starts_with(&format!("{}/", self.source_prefix))
        {
            return doc.to_string();
        }
        format!("{}/{doc}", self.source_prefix)
    }

    /// Losslessly write `kg` to `path` via a sibling temp file + rename.
    /// Relations whose endpoints are not entities are rejected: a triple that
    /// cannot be traversed is not worth preserving.
    pub fn write_full(kg: &KnowledgeGraph, path: &Path) -> StoreResult<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let tmp = sidecar_path(path, ".tmp");
        cleanup_db(&tmp);
        if let Err(error) = build_temp(&tmp, kg) {
            cleanup_db(&tmp);
            return Err(error);
        }
        rename_with_retry(&tmp, path)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mode(&self) -> OpenMode {
        self.mode
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Hydrate `kg` from the store (replaces every field).
    pub fn load_into(&self, kg: &mut KnowledgeGraph) -> StoreResult<()> {
        read::load_into(&self.conn, kg)
    }

    pub fn meta(&self, key: &str) -> StoreResult<Option<String>> {
        read::meta(&self.conn, key)
    }

    /// Upsert a meta row (read-write connections only).
    pub fn set_meta(&self, key: &str, value: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> StoreResult<u32> {
        let found = self.meta(META_SCHEMA_VERSION)?;
        found
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| StoreError::Schema("missing meta.schema_version".to_string()))
    }

    /// Identifies one successful build; the viewer projection cache keys off
    /// this instead of hashing file bytes.
    pub fn build_uid(&self) -> StoreResult<String> {
        Ok(self.meta(META_BUILD_UID)?.unwrap_or_default())
    }

    /// False while a cancelled build's checkpoint still needs its derived
    /// indexes rebuilt (chunk 3).
    pub fn derived_ready(&self) -> StoreResult<bool> {
        Ok(self.meta(META_DERIVED_READY)?.as_deref() == Some("1"))
    }

    pub fn noise_floor(&self) -> StoreResult<Option<f64>> {
        match self.meta(META_NOISE_FLOOR)? {
            Some(raw) => serde_json::from_str::<Option<f64>>(&raw)
                .map_err(|e| StoreError::Data(format!("noise_floor: {e}"))),
            None => Ok(None),
        }
    }

}

/// `path` with `suffix` appended (not replaced), e.g. `graph.sqlite` +
/// `-wal` -> `graph.sqlite-wal`.
pub(crate) fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn cleanup_db(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(sidecar_path(path, suffix));
    }
}

/// Replace `to` with `from`, retrying while a short-lived reader still holds
/// the destination open (Windows refuses the rename until the handle closes).
/// Readers are normally milliseconds; a bounded retry beats a global lock
/// that would freeze the viewer for the duration of a build.
fn rename_with_retry(from: &Path, to: &Path) -> StoreResult<()> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..40u64 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                let step = 50 * (attempt.min(4) + 1);
                std::thread::sleep(std::time::Duration::from_millis(step));
            }
        }
    }
    Err(last.expect("at least one rename attempt").into())
}

fn build_temp(tmp: &Path, kg: &KnowledgeGraph) -> StoreResult<()> {
    let conn = Connection::open(tmp)?;
    // 8 KiB pages hold more of the many small rows per page; must be set
    // before the first table exists. Foreign keys drive the v3 cascade GC.
    conn.execute_batch(
        "PRAGMA page_size=8192; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
         PRAGMA foreign_keys=ON;",
    )?;
    schema::create(&conn)?;
    let uid = new_build_uid();
    {
        let tx = conn.unchecked_transaction()?;
        write::write_all(&tx, kg, &uid)?;
        tx.commit()?;
    }
    // Persist a durable rollback-journal mode before the file becomes the
    // real graph, then close so no sidecar survives the rename.
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
    drop(conn);
    let _ = std::fs::remove_file(sidecar_path(tmp, "-wal"));
    let _ = std::fs::remove_file(sidecar_path(tmp, "-shm"));
    Ok(())
}

fn new_build_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}-{:x}-{n:x}", std::process::id())
}

#[cfg(test)]
mod tests;
