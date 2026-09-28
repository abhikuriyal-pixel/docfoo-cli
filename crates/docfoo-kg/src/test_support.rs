//! Test-only helpers: a temporary read-only `KgStore` built from a
//! `KnowledgeGraph` fixture via `write_full` and removed on drop.
//!
//! Query and expand tests read through the same [`crate::reader::GraphReader`]
//! implementation production uses, so the fixtures exercise the SQLite path.

use crate::graph::KnowledgeGraph;
use crate::store::{KgStore, OpenMode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

pub struct StoreFixture {
    pub store: KgStore,
    dir: PathBuf,
}

impl StoreFixture {
    pub fn new(tag: &str, kg: &KnowledgeGraph) -> StoreFixture {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "docfoo-kg-fixture-{tag}-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("graph.sqlite");
        KgStore::write_full(kg, &path).unwrap();
        let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
        StoreFixture { store, dir }
    }
}

impl Drop for StoreFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
