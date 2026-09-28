//! Native build-path primitives (KG scale plan v2, phase 1).
//!
//! The v1 build path hydrated a whole `KnowledgeGraph`, refit BM25 every wave
//! and serialized everything at the end. These primitives let the build write
//! extraction results straight into the store instead: entity merges are
//! `ON CONFLICT`-style read/merge, relation and join dedup is index-enforced,
//! FTS stays live through triggers, and [`KgStore::glossary`] answers
//! extraction calls from the live indexes.
//!
//! Every primitive uses `self.connection()` directly, so callers can wrap a
//! whole section's writes in one `unchecked_transaction`.

use super::{KgStore, StoreError, StoreResult};
use crate::config::{GLOSSARY_MAX_DF, GLOSSARY_QUERY_TERMS};
use crate::extract::GlossaryEntry;
use crate::graph::Concept;
use crate::text::tokenize;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Base row for one extracted section. Ordered entity links, relations and
/// the concept are written by their own primitives.
#[derive(Clone, Copy, Debug, Default)]
pub struct SectionInput<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub source_doc: &'a str,
    pub start_line: usize,
    pub end_line: usize,
    pub content_hash: &'a str,
    pub retry_pending: bool,
    pub topic: Option<&'a str>,
}

/// Result of [`KgStore::upsert_entity`]: the surviving node and its public
/// `entity_key` (the first-seen id for the name's canonical slug).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpsertedEntity {
    pub node: i64,
    pub entity_key: String,
}

/// One stored section row used by the incremental sync diff.
#[derive(Clone, Debug)]
pub struct StoredSection {
    pub title: String,
    pub source_doc: String,
    pub content_hash: String,
    pub retry_pending: bool,
}

/// Row counts for the build's final stats.
#[derive(Clone, Copy, Debug, Default)]
pub struct StoreCounts {
    pub entities: usize,
    pub relations: usize,
    pub sections: usize,
    pub topics: usize,
}

impl KgStore {
    /// Insert or replace the base section row and return its row id. Tokens
    /// start as `title + text`; call [`KgStore::refresh_section_tokens`] after
    /// linking entities to fold their canonical names in.
    pub fn upsert_section(&self, input: &SectionInput<'_>) -> StoreResult<i64> {
        let conn = self.connection();
        let tokens = tokenize(&format!("{} {}", input.title, input.text));
        conn.execute(
            "INSERT INTO sections(title, topic, text, source_doc, start_line, end_line,
                                  content_hash, retry_pending, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(title) DO UPDATE SET
                 topic = excluded.topic, text = excluded.text,
                 source_doc = excluded.source_doc, start_line = excluded.start_line,
                 end_line = excluded.end_line, content_hash = excluded.content_hash,
                 retry_pending = excluded.retry_pending, tokens = excluded.tokens",
            params![
                input.title,
                input.topic,
                input.text,
                input.source_doc,
                input.start_line as i64,
                input.end_line as i64,
                input.content_hash,
                input.retry_pending as i64,
                tokens.join(" ")
            ],
        )?;
        let row: i64 = conn.query_row(
            "SELECT row FROM sections WHERE title = ?1",
            [input.title],
            |row| row.get(0),
        )?;
        // The locator corpus is the raw section text (DD-21); the
        // locator_df triggers keep the document frequency current.
        set_section_locator_terms(conn, row, input.text)?;
        Ok(row)
    }

    /// Insert or merge one entity and return the surviving node + public id.
    /// Dedup is by canonical name slug (`entity_slug`): the first-seen id
    /// wins, then first name/type wins and the longer description wins —
    /// exactly `KnowledgeGraph::add_entity` + the wave `slug_ids` remap —
    /// while `entity_name_terms` stays current so the glossary sees this
    /// entity immediately.
    pub fn upsert_entity(
        &self,
        entity_key: &str,
        name: &str,
        etype: &str,
        desc: &str,
    ) -> StoreResult<UpsertedEntity> {
        let conn = self.connection();
        let slug = crate::text::canonical_slug(name);
        let tokens = tokenize(&format!("{name} {etype} {desc}"));
        let joined = tokens.join(" ");

        let mut existing: Option<(i64, String, String, String)> = conn
            .query_row(
                "SELECT node, entity_key, desc, tokens FROM entities WHERE entity_key = ?1",
                [entity_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if existing.is_none() {
            existing = conn
                .query_row(
                    "SELECT node, entity_key, desc, tokens FROM entities WHERE entity_slug = ?1",
                    [slug.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
        }

        match existing {
            None => {
                conn.execute(
                    "INSERT INTO entities(entity_key, entity_slug, name, type, desc, tokens)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![entity_key, slug, name, etype, desc, joined],
                )?;
                let node: i64 = conn.query_row(
                    "SELECT node FROM entities WHERE entity_key = ?1",
                    [entity_key],
                    |row| row.get(0),
                )?;
                adjust_entity_df(conn, "", &tokens)?;
                insert_name_terms(conn, node, name)?;
                Ok(UpsertedEntity {
                    node,
                    entity_key: entity_key.to_string(),
                })
            }
            Some((node, stored_key, old_desc, old_tokens)) => {
                if desc.len() > old_desc.len() {
                    conn.execute(
                        "UPDATE entities SET desc = ?1, tokens = ?2 WHERE node = ?3",
                        params![desc, joined, node],
                    )?;
                    adjust_entity_df(conn, &old_tokens, &tokens)?;
                }
                Ok(UpsertedEntity {
                    node,
                    entity_key: stored_key,
                })
            }
        }
    }

    /// Append an ordered, deduplicated entity -> section link. Existing links
    /// keep their position.
    pub fn link_entity_section(&self, node: i64, section_row: i64) -> StoreResult<()> {
        self.connection().execute(
            "INSERT OR IGNORE INTO entity_sections(node, section_row, ord)
             VALUES (?1, ?2,
                     COALESCE((SELECT MAX(ord) + 1 FROM entity_sections WHERE node = ?1), 0))",
            params![node, section_row],
        )?;
        Ok(())
    }

    /// Append an ordered, deduplicated entity -> source-document link.
    pub fn link_entity_doc(&self, node: i64, doc: &str) -> StoreResult<()> {
        self.connection().execute(
            "INSERT OR IGNORE INTO entity_docs(node, doc, ord)
             VALUES (?1, ?2,
                     COALESCE((SELECT MAX(ord) + 1 FROM entity_docs WHERE node = ?1), 0))",
            params![node, doc],
        )?;
        Ok(())
    }

    /// Replace the section's ordered entity id list.
    pub fn set_section_entities(
        &self,
        section_row: i64,
        entity_keys: &[String],
    ) -> StoreResult<()> {
        let conn = self.connection();
        conn.execute(
            "DELETE FROM section_entities WHERE section_row = ?1",
            [section_row],
        )?;
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO section_entities(section_row, entity_key, ord)
             VALUES (?1, ?2, ?3)",
        )?;
        for (ord, key) in entity_keys.iter().enumerate() {
            stmt.execute(params![section_row, key, ord as i64])?;
        }
        Ok(())
    }

    /// Recompute the section corpus (`title + linked entity names + text`)
    /// and update `tokens`; the FTS trigger follows.
    pub fn refresh_section_tokens(&self, section_row: i64) -> StoreResult<()> {
        let conn = self.connection();
        let (title, text, _old_tokens): (String, String, String) = conn.query_row(
            "SELECT title, text, tokens FROM sections WHERE row = ?1",
            [section_row],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let names = self.section_entity_names(section_row)?;
        let tokens = tokenize(&format!("{title} {names} {text}"));
        conn.execute(
            "UPDATE sections SET tokens = ?1 WHERE row = ?2",
            params![tokens.join(" "), section_row],
        )?;
        Ok(())
    }

    /// Upsert the section's concept row and its BM25 corpus (`name summary
    /// terms`, title fallback). `None` keeps the row with `present = 0` so the
    /// concept corpus still covers every section.
    pub fn upsert_concept(
        &self,
        section_row: i64,
        concept: Option<&Concept>,
    ) -> StoreResult<()> {
        let conn = self.connection();
        let title: String = conn.query_row(
            "SELECT title FROM sections WHERE row = ?1",
            [section_row],
            |row| row.get(0),
        )?;
        let mut text = String::new();
        if let Some(concept) = concept {
            text = format!(
                "{} {} {}",
                concept.name,
                concept.summary,
                concept.terms.join(" ")
            );
        }
        if text.trim().is_empty() {
            text = title;
        }
        let tokens = tokenize(&text);
        let (present, name, summary, terms) = match concept {
            Some(concept) => (
                1i64,
                concept.name.as_str(),
                concept.summary.as_str(),
                serde_json::to_string(&concept.terms)
                    .map_err(|e| StoreError::Data(e.to_string()))?,
            ),
            None => (0i64, "", "", "[]".to_string()),
        };
        conn.execute(
            "INSERT INTO concepts(section_row, present, name, summary, terms, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(section_row) DO UPDATE SET
                 present = excluded.present, name = excluded.name,
                 summary = excluded.summary, terms = excluded.terms,
                 tokens = excluded.tokens",
            params![section_row, present, name, summary, terms, tokens.join(" ")],
        )?;
        Ok(())
    }

    /// Insert a relation triple unless it already exists (the unique index
    /// dedups globally, like `KnowledgeGraph::add_relation`). Returns whether
    /// a row was inserted.
    pub fn insert_relation(
        &self,
        source: &str,
        target: &str,
        rel: &str,
        section_row: i64,
        source_doc: Option<&str>,
    ) -> StoreResult<bool> {
        let conn = self.connection();
        let src = entity_node(conn, source)?;
        let dst = entity_node(conn, target)?;
        let changed = conn.execute(
            "INSERT OR IGNORE INTO relations(src, dst, rel, section_row, source_doc)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![src, dst, rel, section_row, source_doc],
        )?;
        Ok(changed == 1)
    }

    /// Known-entity glossary for one extraction call: exact name matches first
    /// (complete, `entity_key` order), then native FTS bm25 hits seeded by the
    /// section's rarest tokens, capped at `k`.
    pub fn glossary(&self, text: &str, k: usize) -> StoreResult<Vec<GlossaryEntry>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let unique: BTreeSet<String> = tokens.into_iter().collect();

        let mut out: Vec<GlossaryEntry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Exact-name pass, streamed in Rust: count each section token's name
        // postings per node and keep the nodes whose count equals their
        // name's unique token count. Complete (every section token is
        // counted) and O(total postings) with plain hash ops — the SQL
        // GROUP BY + correlated subquery cost ~1.3 s/section at 1M entities.
        let mut counts: HashMap<i64, (u32, u32)> = HashMap::new();
        {
            let mut stmt = self
                .connection()
                .prepare("SELECT node, name_len FROM entity_name_terms WHERE term = ?1")?;
            for term in &unique {
                let mut rows = stmt.query([term.as_str()])?;
                while let Some(row) = rows.next()? {
                    let node: i64 = row.get(0)?;
                    let name_len: i64 = row.get(1)?;
                    counts
                        .entry(node)
                        .or_insert((0, name_len as u32))
                        .0 += 1;
                }
            }
        }
        let matches: Vec<i64> = counts
            .into_iter()
            .filter(|(_, (count, name_len))| count == name_len)
            .map(|(node, _)| node)
            .collect();
        if !matches.is_empty() {
            // Fetch matched names in chunks (SQLite parameter limit), then
            // take them in entity_key order like the v1 exact pass.
            let mut entries: Vec<(String, GlossaryEntry)> = Vec::new();
            for chunk in matches.chunks(500) {
                let placeholders = vec!["?"; chunk.len()].join(",");
                let sql = format!(
                    "SELECT entity_key, name, type FROM entities WHERE node IN ({placeholders})"
                );
                let mut stmt = self.connection().prepare(&sql)?;
                let mut rows = stmt.query(params_from_iter(chunk.iter()))?;
                while let Some(row) = rows.next()? {
                    let key: String = row.get(0)?;
                    entries.push((
                        key.clone(),
                        GlossaryEntry { id: key, name: row.get(1)?, etype: row.get(2)? },
                    ));
                }
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (id, entry) in entries {
                if out.len() >= k {
                    break;
                }
                if seen.insert(id) {
                    out.push(entry);
                }
            }
        }

        if out.len() < k {
            let seed = self.glossary_seed_terms(&unique)?;
            if !seed.is_empty() {
                let fts_query = seed.join(" OR ");
                let mut stmt = self.connection().prepare(
                    "SELECT e.entity_key, e.name, e.type
                     FROM entity_fts JOIN entities e ON e.node = entity_fts.rowid
                     WHERE entity_fts MATCH ?1
                     ORDER BY rank, e.node
                     LIMIT ?2",
                )?;
                let mut rows = stmt.query(params![fts_query, k as i64])?;
                while let Some(row) = rows.next()? {
                    if out.len() >= k {
                        break;
                    }
                    let entry = GlossaryEntry {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        etype: row.get(2)?,
                    };
                    if seen.insert(entry.id.clone()) {
                        out.push(entry);
                    }
                }
            }
        }
        Ok(out)
    }

    /// The rarest section tokens (df at most [`GLOSSARY_MAX_DF`]), used to
    /// seed the FTS candidate query so it never scores the whole corpus.
    /// Rarity comes from `entity_df`, maintained incrementally by the writer
    /// (fts5vocab's `doc` walks doclists and is too slow per section).
    fn glossary_seed_terms(&self, unique: &BTreeSet<String>) -> StoreResult<Vec<String>> {
        let mut df: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = self
                .connection()
                .prepare("SELECT df FROM entity_df WHERE term = ?1")?;
            for term in unique {
                if let Some(count) = stmt
                    .query_row([term.as_str()], |row| row.get::<_, i64>(0))
                    .optional()?
                {
                    df.insert(term.clone(), count);
                }
            }
        }
        let mut candidates: Vec<(&String, i64)> = unique
            .iter()
            .filter_map(|term| df.get(term).map(|count| (term, *count)))
            .filter(|(_, count)| *count <= GLOSSARY_MAX_DF as i64)
            .collect();
        candidates.sort_unstable_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        Ok(candidates
            .into_iter()
            .take(GLOSSARY_QUERY_TERMS)
            .map(|(term, _)| term.clone())
            .collect())
    }

    /// Canonical names of the section's linked entities, in link order
    /// (dangling entity keys are skipped, matching the v1 `section_text`).
    fn section_entity_names(&self, section_row: i64) -> StoreResult<String> {
        let mut stmt = self.connection().prepare(
            "SELECT e.name FROM section_entities se
             JOIN entities e ON e.entity_key = se.entity_key
             WHERE se.section_row = ?1 ORDER BY se.ord",
        )?;
        let mut names: Vec<String> = Vec::new();
        let mut rows = stmt.query([section_row])?;
        while let Some(row) = rows.next()? {
            names.push(row.get(0)?);
        }
        Ok(names.join(" "))
    }
}

// ---------------------------------------------------------------------------
// Build orchestration data + finalize
// ---------------------------------------------------------------------------

impl KgStore {
    /// Begin a deferred transaction on the store's connection. Callers use
    /// this to wrap a whole section's primitive calls in one commit.
    pub fn transaction(&self) -> StoreResult<rusqlite::Transaction<'_>> {
        Ok(self.connection().unchecked_transaction()?)
    }

    /// Node id for `entity_key`, if the entity exists.
    pub fn entity_node(&self, entity_key: &str) -> StoreResult<Option<i64>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT node FROM entities WHERE entity_key = ?1",
                [entity_key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Whether `entity_key` exists (build callers drop relations to unknown
    /// endpoints, matching the v1 `kg.entities.contains_key` gate).
    pub fn entity_exists(&self, entity_key: &str) -> StoreResult<bool> {
        Ok(self.entity_node(entity_key)?.is_some())
    }

    /// `insert_relation` for build callers: silently skips triples whose
    /// endpoints are not entities.
    pub fn insert_relation_if_known(
        &self,
        source: &str,
        target: &str,
        rel: &str,
        section_row: i64,
        source_doc: Option<&str>,
    ) -> StoreResult<bool> {
        let Some(src) = self.entity_node(source)? else {
            return Ok(false);
        };
        let Some(dst) = self.entity_node(target)? else {
            return Ok(false);
        };
        let changed = self.connection().execute(
            "INSERT OR IGNORE INTO relations(src, dst, rel, section_row, source_doc)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![src, dst, rel, section_row, source_doc],
        )?;
        Ok(changed == 1)
    }

    /// Sections of `scope_docs`, in title order (incremental sync diff).
    pub fn sections_in_scope(
        &self,
        scope_docs: &BTreeSet<String>,
    ) -> StoreResult<Vec<StoredSection>> {
        if scope_docs.is_empty() {
            return Ok(Vec::new());
        }
        let json = json_array(scope_docs.iter())?;
        let mut stmt = self.connection().prepare(
            "SELECT title, source_doc, content_hash, retry_pending FROM sections
             WHERE source_doc IN (SELECT value FROM json_each(?1))
             ORDER BY title",
        )?;
        let mut rows = stmt.query([json])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(StoredSection {
                title: row.get(0)?,
                source_doc: row.get(1)?,
                content_hash: row.get(2)?,
                retry_pending: row.get::<_, i64>(3)? != 0,
            });
        }
        Ok(out)
    }

    /// Backfill a stored section's content hash without re-extracting it.
    pub fn update_section_hash(&self, title: &str, hash: &str) -> StoreResult<()> {
        self.connection().execute(
            "UPDATE sections SET content_hash = ?1 WHERE title = ?2",
            params![hash, title],
        )?;
        Ok(())
    }

    /// Distinct source documents that have stored sections.
    pub fn section_docs(&self) -> StoreResult<BTreeSet<String>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT DISTINCT source_doc FROM sections WHERE source_doc != ''")?;
        let mut rows = stmt.query([])?;
        let mut out = BTreeSet::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?);
        }
        Ok(out)
    }

    /// Titles of every section belonging to `docs`.
    pub fn section_titles_for_docs(&self, docs: &BTreeSet<String>) -> StoreResult<Vec<String>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let json = json_array(docs.iter())?;
        let mut stmt = self.connection().prepare(
            "SELECT title FROM sections
             WHERE source_doc IN (SELECT value FROM json_each(?1))
             ORDER BY title",
        )?;
        let mut rows = stmt.query([json])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row.get(0)?);
        }
        Ok(out)
    }

    /// Stored section titles + topics, in title order (topic derivation).
    pub fn section_topics(&self) -> StoreResult<Vec<(String, Option<String>)>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT title, topic FROM sections ORDER BY title")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    /// Assign every section its topic (`Other` when absent) and rebuild the
    /// `topics` buckets in title order — the native `build_hierarchy`.
    pub fn apply_topics(&self, topic_map: &HashMap<String, String>) -> StoreResult<()> {
        let conn = self.connection();
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _kg_topics(title TEXT PRIMARY KEY, topic TEXT);
             DELETE FROM _kg_topics;",
        )?;
        {
            let mut stmt =
                tx.prepare("INSERT OR REPLACE INTO _kg_topics(title, topic) VALUES (?1, ?2)")?;
            for (title, topic) in topic_map {
                stmt.execute(params![title, topic])?;
            }
        }
        tx.execute(
            "UPDATE sections SET topic = COALESCE(
                 (SELECT topic FROM _kg_topics WHERE title = sections.title), 'Other')",
            [],
        )?;
        tx.execute_batch(
            "DELETE FROM topics;
             INSERT INTO topics(topic, ord, section)
             SELECT topic, ROW_NUMBER() OVER (PARTITION BY topic ORDER BY title) - 1, title
             FROM sections WHERE topic IS NOT NULL;",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rename source documents (relocate): sections, relations, entity docs
    /// and fingerprints. `entity_docs` dedups against existing rows.
    pub fn rename_documents(&self, renames: &BTreeMap<String, String>) -> StoreResult<()> {
        if renames.is_empty() {
            return Ok(());
        }
        let tx = self.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _kg_doc_renames(old TEXT PRIMARY KEY, new TEXT);
             DELETE FROM _kg_doc_renames;",
        )?;
        {
            let mut stmt = tx
                .prepare("INSERT OR REPLACE INTO _kg_doc_renames(old, new) VALUES (?1, ?2)")?;
            for (old, new) in renames {
                stmt.execute(params![old, new])?;
            }
        }
        tx.execute(
            "UPDATE sections SET source_doc = (
                 SELECT new FROM _kg_doc_renames WHERE old = sections.source_doc)
             WHERE source_doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.execute(
            "UPDATE relations SET source_doc = (
                 SELECT new FROM _kg_doc_renames WHERE old = relations.source_doc)
             WHERE source_doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO entity_docs(node, doc, ord)
             SELECT node, (SELECT new FROM _kg_doc_renames WHERE old = entity_docs.doc), ord
             FROM entity_docs WHERE doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.execute(
            "DELETE FROM entity_docs WHERE doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO source_hashes(doc, hash)
             SELECT (SELECT new FROM _kg_doc_renames WHERE old = source_hashes.doc), hash
             FROM source_hashes WHERE doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.execute(
            "DELETE FROM source_hashes WHERE doc IN (SELECT old FROM _kg_doc_renames)",
            [],
        )?;
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }

    /// Rename a section title (relocate tag rewrites): title, section corpus
    /// tokens (the title feeds the corpus), concept fallback corpus and topic
    /// membership. Joins store row ids, so they need no update.
    pub fn rename_section(&self, old_title: &str, new_title: &str) -> StoreResult<()> {
        if old_title == new_title {
            return Ok(());
        }
        let tx = self.transaction()?;
        tx.execute(
            "UPDATE sections SET title = ?1 WHERE title = ?2",
            params![new_title, old_title],
        )?;
        let row: i64 = tx.query_row(
            "SELECT row FROM sections WHERE title = ?1",
            [new_title],
            |row| row.get(0),
        )?;
        let concept: Option<Concept> = tx
            .query_row(
                "SELECT present, name, summary, terms FROM concepts WHERE section_row = ?1",
                [row],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .and_then(|(present, name, summary, terms)| {
                (present != 0).then(|| Concept {
                    name,
                    summary,
                    terms: serde_json::from_str(&terms).unwrap_or_default(),
                })
            });
        self.refresh_section_tokens(row)?;
        self.upsert_concept(row, concept.as_ref())?;
        tx.execute(
            "UPDATE topics SET section = ?1 WHERE section = ?2",
            params![new_title, old_title],
        )?;
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }

    /// Drop fingerprints for `docs` (relocate eviction).
    pub fn delete_source_hashes(&self, docs: &BTreeSet<String>) -> StoreResult<()> {
        if docs.is_empty() {
            return Ok(());
        }
        let json = json_array(docs.iter())?;
        self.connection().execute(
            "DELETE FROM source_hashes WHERE doc IN (SELECT value FROM json_each(?1))",
            [json],
        )?;
        Ok(())
    }

    /// Every stored fingerprint.
    pub fn stored_hashes(&self) -> StoreResult<BTreeMap<String, String>> {
        let mut stmt = self.connection().prepare("SELECT doc, hash FROM source_hashes")?;
        let mut rows = stmt.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?, row.get(1)?);
        }
        Ok(out)
    }

    /// Stored section titles + source docs, in title order (relocate).
    pub fn section_title_docs(&self) -> StoreResult<Vec<(String, String)>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT title, source_doc FROM sections ORDER BY title")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    /// Record one document's raw-file fingerprint.
    pub fn upsert_source_hash(&self, doc: &str, hash: &str) -> StoreResult<()> {
        self.connection().execute(
            "INSERT INTO source_hashes(doc, hash) VALUES (?1, ?2)
             ON CONFLICT(doc) DO UPDATE SET hash = excluded.hash",
            params![doc, hash],
        )?;
        Ok(())
    }

    /// Drop fingerprints whose document has no stored sections.
    pub fn prune_source_hashes(&self) -> StoreResult<()> {
        self.connection().execute(
            "DELETE FROM source_hashes WHERE doc NOT IN (
                 SELECT DISTINCT source_doc FROM sections WHERE source_doc != '')",
            [],
        )?;
        Ok(())
    }

    /// Row counts for the build's final stats.
    pub fn counts(&self) -> StoreResult<StoreCounts> {
        let conn = self.connection();
        let count = |sql: &str| -> StoreResult<usize> {
            Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))? as usize)
        };
        Ok(StoreCounts {
            entities: count("SELECT count(*) FROM entities")?,
            relations: count("SELECT count(*) FROM relations")?,
            sections: count("SELECT count(*) FROM sections")?,
            topics: count(
                "SELECT count(DISTINCT topic) FROM sections WHERE topic IS NOT NULL",
            )?,
        })
    }

    /// Entities supported by more than one source document.
    pub fn cross_doc_merges(&self) -> StoreResult<usize> {
        Ok(self.connection().query_row(
            "SELECT count(*) FROM (
                 SELECT es.node FROM entity_sections es
                 JOIN sections s ON s.row = es.section_row
                 WHERE s.source_doc != ''
                 GROUP BY es.node HAVING count(DISTINCT s.source_doc) > 1)",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize)
    }

    /// Full entity vocabulary with document frequencies — the noise-floor
    /// sample pool.
    pub fn entity_df(&self) -> StoreResult<BTreeMap<String, usize>> {
        let mut stmt = self
            .connection()
            .prepare("SELECT term, df FROM entity_df")?;
        let mut rows = stmt.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?, row.get::<_, i64>(1)? as usize);
        }
        Ok(out)
    }

    /// Stamp a finished build: counts, build uid, noise floor and
    /// `derived_ready`.
    pub fn finalize_build(&self, noise_floor: Option<f64>) -> StoreResult<()> {
        let counts = self.counts()?;
        self.set_meta(super::META_ENTITY_COUNT, &counts.entities.to_string())?;
        self.set_meta(super::META_RELATION_COUNT, &counts.relations.to_string())?;
        self.set_meta(super::META_SECTION_COUNT, &counts.sections.to_string())?;
        self.set_meta(super::META_BUILD_UID, &super::new_build_uid())?;
        self.set_meta(
            super::META_NOISE_FLOOR,
            &serde_json::to_string(&noise_floor)
                .map_err(|e| StoreError::Data(e.to_string()))?,
        )?;
        self.set_meta(super::META_DERIVED_READY, "1")?;
        Ok(())
    }
}

fn json_array<'a>(values: impl Iterator<Item = &'a String>) -> StoreResult<String> {
    serde_json::to_string(&values.collect::<Vec<_>>())
        .map_err(|e| StoreError::Data(e.to_string()))
}

fn entity_node(conn: &Connection, entity_key: &str) -> StoreResult<i64> {
    conn.query_row(
        "SELECT node FROM entities WHERE entity_key = ?1",
        [entity_key],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| {
        StoreError::Data(format!(
            "relation references unknown entity {entity_key:?}"
        ))
    })
}

fn insert_name_terms(conn: &Connection, node: i64, name: &str) -> StoreResult<()> {
    let terms: BTreeSet<String> = tokenize(name).into_iter().collect();
    let name_len = terms.len() as i64;
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO entity_name_terms(term, node, name_len) VALUES (?1, ?2, ?3)",
    )?;
    for term in terms {
        stmt.execute(params![term, node, name_len])?;
    }
    Ok(())
}

/// Move `entity_df` from the old token string to the new token list,
/// counting each term once per entity (document frequency).
fn adjust_entity_df(conn: &Connection, old_tokens: &str, new_tokens: &[String]) -> StoreResult<()> {
    let old_set: BTreeSet<&str> = old_tokens.split(' ').filter(|t| !t.is_empty()).collect();
    let new_set: BTreeSet<&str> = new_tokens.iter().map(String::as_str).collect();
    if old_set == new_set {
        return Ok(());
    }
    let mut dec = conn.prepare("UPDATE entity_df SET df = df - 1 WHERE term = ?1")?;
    let mut del = conn.prepare("DELETE FROM entity_df WHERE term = ?1 AND df <= 0")?;
    let mut inc = conn.prepare(
        "INSERT INTO entity_df(term, df) VALUES (?1, 1)
         ON CONFLICT(term) DO UPDATE SET df = df + 1",
    )?;
    for term in old_set.difference(&new_set) {
        dec.execute([*term])?;
        del.execute([*term])?;
    }
    for term in new_set.difference(&old_set) {
        inc.execute([*term])?;
    }
    Ok(())
}

/// Replace a section's raw locator vocabulary (DD-21); the
/// `section_locator_terms_ai/ad` triggers keep `locator_df` current.
fn set_section_locator_terms(conn: &Connection, section_row: i64, text: &str) -> StoreResult<()> {
    conn.execute(
        "DELETE FROM section_locator_terms WHERE section_row = ?1",
        [section_row],
    )?;
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO section_locator_terms(term, section_row) VALUES (?1, ?2)",
    )?;
    for term in crate::query::deliver::locator_tokens(text) {
        stmt.execute(params![term, section_row])?;
    }
    Ok(())
}
