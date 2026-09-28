//! SQLite schema for the knowledge-graph store (v3, native indexing plan).
//!
//! One `.sqlite` file per graph. Row ids are dense and assigned in the same
//! order the in-memory model iterates (entities by `entity_key`, sections by
//! title), so a round trip through the store is order-preserving.
//!
//! v3 enforces referential integrity instead of preserving dangling
//! provenance: provenance joins store `section_row` ids with `ON DELETE
//! CASCADE`, relation triples are unique, and FTS5 tables are kept in sync by
//! triggers. Sections/topics keep text keys only where the model allows
//! dangling values (`section_entities.entity_key`, `topics.section`).
//!
//! v4 adds `entity_name_terms`: the native glossary's exact-name inverted
//! index, written by the native writer (SQLite cannot tokenize) and cascaded
//! with its entity.
//!
//! v5 adds `entities.entity_slug` (the canonical slug the native writer
//! dedups on while keeping the first-seen `entity_key` as the public id) and
//! an index on `sections.source_doc` for incremental sync.
//!
//! v6 drops the v1 exact scorer's `bm25_stats`/`term_df`/`doc_len` columns and
//! the `locator_fts` index: queries rank with native FTS5 `bm25()`, the
//! locator uses `section_locator_terms` + `locator_df`, and the glossary's
//! token rarity comes from `entity_df`.
//!
//! v8 adds `entity_name_terms.name_len` so the glossary's exact-name pass can
//! stream postings in Rust and compare counts directly, instead of a SQL
//! `GROUP BY` + correlated subquery per section (which cost ~1.3 s/section on
//! a 1M-entity store).
//!
//! `entity_df` is a glossary/noise-floor-only table: fts5vocab's `doc` column
//! walks the term's doclist (~1 ms for common terms), which is too slow per
//! extraction call, so the native writer maintains the entity corpus's
//! document frequencies directly.

use super::{StoreError, StoreResult};
use rusqlite::OptionalExtension;

/// Bump whenever the DDL changes; opening a database with another version is
/// refused instead of guessing (no legacy import — rebuild the graph).
pub const SCHEMA_VERSION: u32 = 8;

/// All base tables, virtual tables, indexes and triggers.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- Dense row ids follow title (BTreeMap) order.
CREATE TABLE IF NOT EXISTS sections(
  row          INTEGER PRIMARY KEY,
  title        TEXT NOT NULL UNIQUE,
  topic        TEXT,
  text         TEXT NOT NULL DEFAULT '',
  source_doc   TEXT NOT NULL DEFAULT '',
  start_line   INTEGER NOT NULL DEFAULT 0,
  end_line     INTEGER NOT NULL DEFAULT 0,
  content_hash TEXT NOT NULL DEFAULT '',
  retry_pending INTEGER NOT NULL DEFAULT 0,
  tokens       TEXT NOT NULL DEFAULT ''
);

-- Dense node ids follow entity_key (BTreeMap) order. `entity_slug` is the
-- canonical slug the native writer dedups on; `entity_key` stays the
-- first-seen id (`{TYPE}_{slug}`) so public ids never change.
CREATE TABLE IF NOT EXISTS entities(
  node       INTEGER PRIMARY KEY,
  entity_key TEXT NOT NULL UNIQUE,
  entity_slug TEXT NOT NULL UNIQUE,
  name       TEXT NOT NULL,
  type       TEXT NOT NULL DEFAULT '',
  desc       TEXT NOT NULL DEFAULT '',
  tokens     TEXT NOT NULL DEFAULT ''
);

-- Entity.sections is ordered; the row id cascades with its section.
CREATE TABLE IF NOT EXISTS entity_sections(
  node        INTEGER NOT NULL REFERENCES entities(node) ON DELETE CASCADE,
  section_row INTEGER NOT NULL REFERENCES sections(row) ON DELETE CASCADE,
  ord         INTEGER NOT NULL,
  PRIMARY KEY(node, section_row)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS entity_sections_section ON entity_sections(section_row);

CREATE TABLE IF NOT EXISTS entity_docs(
  node INTEGER NOT NULL REFERENCES entities(node) ON DELETE CASCADE,
  doc  TEXT NOT NULL,
  ord  INTEGER NOT NULL,
  PRIMARY KEY(node, doc)
) WITHOUT ROWID;

-- Exact-name inverted index for the native glossary: one row per unique token
-- of an entity's name, carrying the name's unique token count so the writer
-- can stream postings and match counts without a SQL GROUP BY. Names are
-- first-wins, so rows are immutable; they cascade with their entity.
CREATE TABLE IF NOT EXISTS entity_name_terms(
  term     TEXT NOT NULL,
  node     INTEGER NOT NULL REFERENCES entities(node) ON DELETE CASCADE,
  name_len INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(term, node)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS entity_name_terms_node ON entity_name_terms(node);

CREATE TABLE IF NOT EXISTS relations(
  rid         INTEGER PRIMARY KEY,
  src         INTEGER NOT NULL REFERENCES entities(node) ON DELETE CASCADE,
  dst         INTEGER NOT NULL REFERENCES entities(node) ON DELETE CASCADE,
  rel         TEXT NOT NULL DEFAULT '',
  section_row INTEGER NOT NULL REFERENCES sections(row) ON DELETE CASCADE,
  source_doc  TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS relations_triple ON relations(src, dst, rel);
CREATE INDEX IF NOT EXISTS relations_src ON relations(src);
CREATE INDEX IF NOT EXISTS relations_dst ON relations(dst);
CREATE INDEX IF NOT EXISTS relations_section ON relations(section_row);

-- SectionInfo.entity_ids is ordered; the key text keeps dangling refs intact.
CREATE TABLE IF NOT EXISTS section_entities(
  section_row INTEGER NOT NULL REFERENCES sections(row) ON DELETE CASCADE,
  entity_key  TEXT NOT NULL,
  ord         INTEGER NOT NULL,
  PRIMARY KEY(section_row, entity_key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS section_entities_entity ON section_entities(entity_key);

-- One row per section (present=0 marks the title-fallback corpus).
CREATE TABLE IF NOT EXISTS concepts(
  section_row INTEGER PRIMARY KEY REFERENCES sections(row) ON DELETE CASCADE,
  present     INTEGER NOT NULL DEFAULT 0,
  name        TEXT NOT NULL DEFAULT '',
  summary     TEXT NOT NULL DEFAULT '',
  terms       TEXT NOT NULL DEFAULT '',
  tokens      TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS source_hashes(
  doc  TEXT PRIMARY KEY,
  hash TEXT NOT NULL
);

-- Incremental sync filters stored sections by source document.
CREATE INDEX IF NOT EXISTS sections_source_doc ON sections(source_doc);

-- KnowledgeGraph.topics is stored (not rebuilt on hydrate) so stale or empty
-- buckets round-trip exactly; stale titles are deliberately not enforced.
CREATE TABLE IF NOT EXISTS topics(
  topic   TEXT NOT NULL,
  ord     INTEGER NOT NULL,
  section TEXT NOT NULL,
  PRIMARY KEY(topic, ord)
) WITHOUT ROWID;

-- Raw locator vocabulary (DD-21): one row per unique raw token per section.
-- The triggers keep `locator_df` (document frequency) current on every
-- write/delete, so the locator never needs a full-text index or a scan.
CREATE TABLE IF NOT EXISTS section_locator_terms(
  term        TEXT NOT NULL,
  section_row INTEGER NOT NULL REFERENCES sections(row) ON DELETE CASCADE,
  PRIMARY KEY(term, section_row)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS locator_df(
  term TEXT PRIMARY KEY,
  df   INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS section_locator_terms_ai AFTER INSERT ON section_locator_terms BEGIN
  INSERT INTO locator_df(term, df) VALUES (new.term, 1)
  ON CONFLICT(term) DO UPDATE SET df = df + 1;
END;
CREATE TRIGGER IF NOT EXISTS section_locator_terms_ad AFTER DELETE ON section_locator_terms BEGIN
  UPDATE locator_df SET df = df - 1 WHERE term = old.term;
  DELETE FROM locator_df WHERE term = old.term AND df <= 0;
END;

-- Pre-stemmed corpora. detail=col keeps term frequencies without positions:
-- enough for native bm25 ranking, cheaper than full.
CREATE VIRTUAL TABLE IF NOT EXISTS entity_fts USING fts5(
  tokens, content='entities', content_rowid='node', tokenize='unicode61', detail=col
);
CREATE VIRTUAL TABLE IF NOT EXISTS section_fts USING fts5(
  tokens, content='sections', content_rowid='row', tokenize='unicode61', detail=col
);
CREATE VIRTUAL TABLE IF NOT EXISTS concept_fts USING fts5(
  tokens, content='concepts', content_rowid='section_row', tokenize='unicode61', detail=col
);
-- Entity-corpus document frequencies for glossary token-rarity selection
-- and the noise-floor sample pool, maintained incrementally by the writer.
CREATE TABLE IF NOT EXISTS entity_df(
  term TEXT PRIMARY KEY,
  df   INTEGER NOT NULL
) WITHOUT ROWID;

-- Triggers keep every FTS index in sync with its base table, so no build step
-- ever rebuilds them. External-content FTS5 needs the old column values for
-- the 'delete' command, which is why update/delete triggers pass them through.
CREATE TRIGGER IF NOT EXISTS entities_ai AFTER INSERT ON entities BEGIN
  INSERT INTO entity_fts(rowid, tokens) VALUES (new.node, new.tokens);
END;
CREATE TRIGGER IF NOT EXISTS entities_ad AFTER DELETE ON entities BEGIN
  INSERT INTO entity_fts(entity_fts, rowid, tokens)
  VALUES ('delete', old.node, old.tokens);
END;
CREATE TRIGGER IF NOT EXISTS entities_au AFTER UPDATE OF tokens ON entities BEGIN
  INSERT INTO entity_fts(entity_fts, rowid, tokens)
  VALUES ('delete', old.node, old.tokens);
  INSERT INTO entity_fts(rowid, tokens) VALUES (new.node, new.tokens);
END;

CREATE TRIGGER IF NOT EXISTS sections_ai AFTER INSERT ON sections BEGIN
  INSERT INTO section_fts(rowid, tokens) VALUES (new.row, new.tokens);
END;
CREATE TRIGGER IF NOT EXISTS sections_ad AFTER DELETE ON sections BEGIN
  INSERT INTO section_fts(section_fts, rowid, tokens)
  VALUES ('delete', old.row, old.tokens);
END;
CREATE TRIGGER IF NOT EXISTS sections_au_tokens AFTER UPDATE OF tokens ON sections BEGIN
  INSERT INTO section_fts(section_fts, rowid, tokens)
  VALUES ('delete', old.row, old.tokens);
  INSERT INTO section_fts(rowid, tokens) VALUES (new.row, new.tokens);
END;

CREATE TRIGGER IF NOT EXISTS concepts_ai AFTER INSERT ON concepts BEGIN
  INSERT INTO concept_fts(rowid, tokens) VALUES (new.section_row, new.tokens);
END;
CREATE TRIGGER IF NOT EXISTS concepts_ad AFTER DELETE ON concepts BEGIN
  INSERT INTO concept_fts(concept_fts, rowid, tokens)
  VALUES ('delete', old.section_row, old.tokens);
END;
CREATE TRIGGER IF NOT EXISTS concepts_au AFTER UPDATE OF tokens ON concepts BEGIN
  INSERT INTO concept_fts(concept_fts, rowid, tokens)
  VALUES ('delete', old.section_row, old.tokens);
  INSERT INTO concept_fts(rowid, tokens) VALUES (new.section_row, new.tokens);
END;

-- GC: an entity whose last supporting section goes away dies with it, which
-- cascades its relations and source documents. This makes "delete a section"
-- a single statement (plus the FTS/join cascades above).
CREATE TRIGGER IF NOT EXISTS entity_sections_dead AFTER DELETE ON entity_sections
WHEN NOT EXISTS (SELECT 1 FROM entity_sections WHERE node = old.node)
BEGIN
  DELETE FROM entities WHERE node = old.node;
END;
"#;

/// Create every object (idempotent) and seed the schema version. Refuses to
/// touch a database whose stored version differs — the hard-cutover rule
/// applies to read-write opens too, not just queries.
pub fn create(conn: &rusqlite::Connection) -> StoreResult<()> {
    if let Some(found) = read_version(conn)? {
        if found != SCHEMA_VERSION {
            return Err(StoreError::Schema(format!(
                "schema version {found}, expected {SCHEMA_VERSION} (no legacy import; rebuild the graph)"
            )));
        }
    }
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

fn read_version(conn: &rusqlite::Connection) -> StoreResult<Option<u32>> {
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'",
        [],
        |row| row.get(0),
    )?;
    if has_meta == 0 {
        return Ok(None);
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match raw {
        None => Ok(None),
        Some(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| StoreError::Schema(format!("bad schema_version {value:?}"))),
    }
}
