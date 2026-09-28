//! Build-path mutations: section removals and `VACUUM INTO` backups.
//!
//! Schema v3 does the bookkeeping: joins store `section_row` ids and cascade
//! on delete, and the `entity_sections_dead` trigger GCs entities that lose
//! their last section. `remove_sections` is therefore one `DELETE`; the
//! native build writer lives in `store/native.rs`.

use super::{KgStore, StoreResult};
use std::path::Path;

impl KgStore {
    /// Delete `titles` and GC exactly like `build::apply_removals`: the
    /// section delete cascades joins/relations/concepts, the
    /// `entity_sections_dead` trigger kills entities left without sections
    /// (cascading their relations and docs), and surviving touched entities
    /// get their source-document lists recomputed.
    pub fn remove_sections(&self, titles: &[String]) -> StoreResult<()> {
        if titles.is_empty() {
            return Ok(());
        }
        let tx = self.connection().unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _kg_titles(title TEXT PRIMARY KEY);
             DELETE FROM _kg_titles;
             CREATE TEMP TABLE IF NOT EXISTS _kg_touched(node INTEGER PRIMARY KEY);
             DELETE FROM _kg_touched;",
        )?;
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO _kg_titles(title) VALUES (?1)")?;
            for title in titles {
                insert.execute([title])?;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO _kg_touched(node)
             SELECT es.node FROM entity_sections es
             JOIN sections s ON s.row = es.section_row
             WHERE s.title IN (SELECT title FROM _kg_titles)",
            [],
        )?;
        // One statement: FK cascades remove joins/relations/concepts, and the
        // dead-entity trigger removes orphaned entities (cascading again).
        tx.execute(
            "DELETE FROM sections WHERE title IN (SELECT title FROM _kg_titles)",
            [],
        )?;
        // Surviving touched entities: rebuild source docs from their remaining
        // sections (sorted, deduplicated — the in-memory BTreeSet order).
        tx.execute(
            "DELETE FROM entity_docs WHERE node IN (SELECT node FROM _kg_touched)",
            [],
        )?;
        tx.execute(
            "INSERT INTO entity_docs(node, ord, doc)
             SELECT node, ROW_NUMBER() OVER (PARTITION BY node ORDER BY doc) - 1, doc
             FROM (
                 SELECT DISTINCT es.node AS node, s.source_doc AS doc
                 FROM entity_sections es JOIN sections s ON s.row = es.section_row
                 WHERE es.node IN (SELECT node FROM _kg_touched) AND s.source_doc != ''
             )",
            [],
        )?;
        // Derived indexes now describe a graph that no longer exists.
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('derived_ready', '0')
             ON CONFLICT(key) DO UPDATE SET value = '0'",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Compact backup via `VACUUM INTO` (works even with a pending WAL).
    pub fn backup_to(&self, dest: &Path) -> StoreResult<()> {
        let _ = std::fs::remove_file(dest);
        self.connection().execute(
            "VACUUM INTO ?1",
            [dest.to_string_lossy().to_string()],
        )?;
        Ok(())
    }
}
