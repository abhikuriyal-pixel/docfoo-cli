//! Read side of `KgStore`: the [`GraphReader`] implementation, native FTS5
//! bm25 ranking, the rare-token locator and indexed adjacency lookups.
//!
//! Queries rank with FTS5's own `bm25()` (`ORDER BY rank`), so there is no
//! candidate rescore and no persisted statistics to keep in sync.

use super::{KgStore, META_SECTION_COUNT, StoreResult};
use crate::graph::{Concept, Entity, SectionInfo};
use crate::reader::{EntityPrimary, GraphReader};
use crate::text::tokenize;
use rusqlite::{params, params_from_iter, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy)]
enum Bm25Corpus {
    Entities,
    Sections,
    Concepts,
}

impl KgStore {
    /// One entity with its ordered supporting sections and source documents.
    pub fn entity(&self, id: &str) -> StoreResult<Option<Entity>> {
        let mut stmt = self.connection().prepare(
            "SELECT node, entity_key, name, type, desc FROM entities WHERE entity_key = ?1",
        )?;
        let row = stmt
            .query_row([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    Entity {
                        id: row.get(1)?,
                        name: row.get(2)?,
                        etype: row.get(3)?,
                        desc: row.get(4)?,
                        sections: Vec::new(),
                        source_doc: Vec::new(),
                    },
                ))
            })
            .optional()?;
        let Some((node, mut entity)) = row else { return Ok(None) };
        {
            let mut stmt = self.connection().prepare(
                "SELECT s.title FROM entity_sections es
                 JOIN sections s ON s.row = es.section_row
                 WHERE es.node = ?1 ORDER BY es.ord",
            )?;
            let mut rows = stmt.query([node])?;
            while let Some(row) = rows.next()? {
                entity.sections.push(row.get(0)?);
            }
        }
        {
            let mut stmt = self
                .connection()
                .prepare("SELECT doc FROM entity_docs WHERE node = ?1 ORDER BY ord")?;
            let mut rows = stmt.query([node])?;
            while let Some(row) = rows.next()? {
                entity.source_doc.push(row.get(0)?);
            }
        }
        Ok(Some(entity))
    }

    /// One section with ordered `entity_ids` and its concept. The graph-dir
    /// prefix is applied to `source_doc` here (the stored rows stay
    /// graph-root-relative), replacing the old query-time mutation.
    pub fn section(&self, title: &str) -> StoreResult<Option<SectionInfo>> {
        let mut stmt = self.connection().prepare(
            "SELECT row, topic, text, source_doc, start_line, end_line, content_hash, retry_pending
             FROM sections WHERE title = ?1",
        )?;
        let row = stmt
            .query_row([title], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    SectionInfo {
                        topic: row.get(1)?,
                        entity_ids: Vec::new(),
                        concept: None,
                        text: row.get(2)?,
                        source_doc: row.get(3)?,
                        start_line: row.get::<_, i64>(4)? as usize,
                        end_line: row.get::<_, i64>(5)? as usize,
                        content_hash: row.get(6)?,
                        retry_pending: row.get::<_, i64>(7)? != 0,
                    },
                ))
            })
            .optional()?;
        let Some((section_row, mut info)) = row else { return Ok(None) };
        {
            let mut stmt = self.connection().prepare(
                "SELECT entity_key FROM section_entities WHERE section_row = ?1 ORDER BY ord",
            )?;
            let mut rows = stmt.query([section_row])?;
            while let Some(row) = rows.next()? {
                info.entity_ids.push(row.get(0)?);
            }
        }
        {
            let mut stmt = self.connection().prepare(
                "SELECT present, name, summary, terms FROM concepts WHERE section_row = ?1",
            )?;
            let concept = stmt
                .query_row([section_row], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .optional()?;
            if let Some((present, name, summary, terms)) = concept {
                if present != 0 {
                    info.concept = Some(Concept {
                        name,
                        summary,
                        terms: serde_json::from_str(&terms).unwrap_or_default(),
                    });
                }
            }
        }
        info.source_doc = self.prefix_doc(&info.source_doc);
        Ok(Some(info))
    }

    pub fn bm25_entity_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        self.bm25_search(Bm25Corpus::Entities, query, k)
    }

    pub fn bm25_section_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        self.bm25_search(Bm25Corpus::Sections, query, k)
    }

    pub fn bm25_concept_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        self.bm25_search(Bm25Corpus::Concepts, query, k)
    }

    /// Jev candidates: BM25 hits first, then remaining sections in title
    /// order up to `k` (see [`GraphReader::concept_shortlist`]).
    pub fn concept_shortlist(&self, query: &str, k: usize) -> StoreResult<Vec<String>> {
        let mut hits: Vec<String> = self
            .bm25_concept_search(query, k)?
            .into_iter()
            .map(|(title, _)| title)
            .collect();
        if hits.len() < k {
            let mut seen: HashSet<String> = hits.iter().cloned().collect();
            let mut stmt = self
                .connection()
                .prepare("SELECT title FROM sections ORDER BY title")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if hits.len() >= k {
                    break;
                }
                let title: String = row.get(0)?;
                if seen.insert(title.clone()) {
                    hits.push(title);
                }
            }
        }
        Ok(hits)
    }

    /// Sorted, deduplicated neighbor ids per requested entity id.
    pub fn neighbors(&self, ids: &[String]) -> StoreResult<HashMap<String, Vec<String>>> {
        let mut out: HashMap<String, Vec<String>> =
            ids.iter().map(|id| (id.clone(), Vec::new())).collect();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        for source_side in [true, false] {
            let column = if source_side { "s.entity_key" } else { "t.entity_key" };
            let sql = format!(
                "SELECT s.entity_key, t.entity_key FROM relations r
                 JOIN entities s ON s.node = r.src
                 JOIN entities t ON t.node = r.dst
                 WHERE {column} IN ({placeholders})"
            );
            let mut stmt = self.connection().prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(ids.iter().map(String::as_str)))?;
            while let Some(row) = rows.next()? {
                let src: String = row.get(0)?;
                let dst: String = row.get(1)?;
                if source_side {
                    if let Some(list) = out.get_mut(&src) {
                        list.push(dst);
                    }
                } else if let Some(list) = out.get_mut(&dst) {
                    list.push(src);
                }
            }
        }
        for list in out.values_mut() {
            list.sort();
            list.dedup();
        }
        Ok(out)
    }

    /// `(source name, relation, target name)` triples among `ids`, in
    /// relation insertion order, capped at `limit`.
    pub fn relations_among(
        &self,
        ids: &HashSet<String>,
        limit: usize,
    ) -> StoreResult<Vec<(String, String, String)>> {
        if limit == 0 || ids.is_empty() {
            return Ok(Vec::new());
        }
        let values: Vec<&str> = ids.iter().map(String::as_str).collect();
        let placeholders = vec!["?"; values.len()].join(",");
        let sql = format!(
            "SELECT s.name, r.rel, t.name FROM relations r
             JOIN entities s ON s.node = r.src
             JOIN entities t ON t.node = r.dst
             WHERE s.entity_key IN ({placeholders}) AND t.entity_key IN ({placeholders})
             ORDER BY r.rid LIMIT {limit}"
        );
        let mut stmt = self.connection().prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(
            values.iter().copied().chain(values.iter().copied()),
        ))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    /// Primary section/topic per requested entity id.
    pub fn entity_primaries(
        &self,
        ids: &[String],
    ) -> StoreResult<HashMap<String, EntityPrimary>> {
        let mut out = HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT e.entity_key, s.title, s.topic FROM entities e
             JOIN entity_sections es ON es.node = e.node AND es.ord = 0
             JOIN sections s ON s.row = es.section_row
             WHERE e.entity_key IN ({placeholders})"
        );
        let mut stmt = self.connection().prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(ids.iter().map(String::as_str)))?;
        while let Some(row) = rows.next()? {
            out.insert(
                row.get(0)?,
                EntityPrimary {
                    section: row.get(1)?,
                    topic: row.get(2)?,
                },
            );
        }
        Ok(out)
    }

    /// Unique exact title match, else unique substring match (guide-pick
    /// fragment resolution), using Rust's Unicode lowercasing via `kg_lower`.
    pub fn resolve_section_fragment(&self, fragment: &str) -> StoreResult<Option<String>> {
        let lowered = fragment.trim().to_lowercase();
        if lowered.is_empty() {
            return Ok(None);
        }
        {
            let mut stmt = self.connection().prepare(
                "SELECT title FROM sections WHERE kg_lower(title) = ?1 LIMIT 2",
            )?;
            let mut rows = stmt.query([&lowered])?;
            let mut exact = Vec::new();
            while let Some(row) = rows.next()? {
                exact.push(row.get::<_, String>(0)?);
            }
            if exact.len() == 1 {
                return Ok(Some(exact.remove(0)));
            }
        }
        let escaped = lowered
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.connection().prepare(
            "SELECT title FROM sections WHERE kg_lower(title) LIKE ?1 ESCAPE '\\' LIMIT 2",
        )?;
        let mut rows = stmt.query([&pattern])?;
        let mut contains = Vec::new();
        while let Some(row) = rows.next()? {
            contains.push(row.get::<_, String>(0)?);
        }
        if contains.len() == 1 {
            return Ok(Some(contains.remove(0)));
        }
        Ok(None)
    }

    /// Rare-token locator: occurrences come from `locator_df`, candidate
    /// sections from `section_locator_terms` (both maintained by the writer's
    /// triggers), and the exact regex scoring runs only on those candidates.
    pub fn locate_direct_sections(
        &self,
        query: &str,
        max_sections: usize,
    ) -> StoreResult<Vec<String>> {
        if max_sections == 0 {
            return Ok(Vec::new());
        }
        let patterns = crate::query::deliver::locator_patterns(query);
        if patterns.is_empty() {
            return Ok(Vec::new());
        }
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        for (token, _) in &patterns {
            occurrences.insert(token.clone(), self.locator_df(token)?);
        }
        let active = crate::query::deliver::locator_active(&patterns, &occurrences);
        if active.is_empty() {
            return Ok(Vec::new());
        }
        let json =
            serde_json::to_string(&active.iter().map(|(token, _)| token).collect::<Vec<_>>())
                .map_err(|e| super::StoreError::Data(e.to_string()))?;
        let mut sections: Vec<(String, String)> = Vec::new();
        {
            let mut stmt = self.connection().prepare(
                "SELECT DISTINCT s.title, s.text FROM section_locator_terms t
                 JOIN sections s ON s.row = t.section_row
                 WHERE t.term IN (SELECT value FROM json_each(?1))",
            )?;
            let mut rows = stmt.query([json])?;
            while let Some(row) = rows.next()? {
                let title: String = row.get(0)?;
                let text: String = row.get(1)?;
                sections.push((title, text.to_lowercase()));
            }
        }
        let total_secs = self
            .meta(META_SECTION_COUNT)?
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        Ok(crate::query::deliver::locator_rank(
            total_secs,
            &active,
            &occurrences,
            &sections,
            max_sections,
        ))
    }

    /// Document frequency of a raw locator token (0 when absent).
    fn locator_df(&self, token: &str) -> StoreResult<usize> {
        Ok(self
            .connection()
            .query_row("SELECT df FROM locator_df WHERE term = ?1", [token], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0) as usize)
    }

    /// Every document the graph knows about, with its build-time hash when
    /// recorded. Paths stay graph-root-relative.
    pub fn stored_docs(&self) -> StoreResult<BTreeMap<String, Option<String>>> {
        let mut docs: BTreeMap<String, Option<String>> = BTreeMap::new();
        {
            let mut stmt = self.connection().prepare(
                "SELECT DISTINCT source_doc FROM sections WHERE source_doc != ''",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                docs.entry(row.get(0)?).or_insert(None);
            }
        }
        {
            let mut stmt = self.connection().prepare("SELECT doc, hash FROM source_hashes")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                docs.insert(row.get(0)?, Some(row.get(1)?));
            }
        }
        Ok(docs)
    }

    // -- internals ------------------------------------------------------

    /// Native FTS5 bm25 search: candidates and ranking come from the index in
    /// one query (`ORDER BY rank`), and the score is `-bm25()` so higher is
    /// better. No exact rescore, no statistics tables.
    fn bm25_search(
        &self,
        corpus: Bm25Corpus,
        query: &str,
        k: usize,
    ) -> StoreResult<Vec<(String, f64)>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = tokens.join(" OR ");
        let mut out: Vec<(String, f64)> = Vec::new();
        match corpus {
            Bm25Corpus::Entities => {
                let mut stmt = self.connection().prepare(
                    "SELECT e.entity_key, -bm25(entity_fts) FROM entity_fts
                     JOIN entities e ON e.node = entity_fts.rowid
                     WHERE entity_fts MATCH ?1
                     ORDER BY rank, e.node LIMIT ?2",
                )?;
                let mut rows = stmt.query(params![fts_query, k as i64])?;
                while let Some(row) = rows.next()? {
                    out.push((row.get(0)?, row.get(1)?));
                }
            }
            Bm25Corpus::Sections => {
                let mut stmt = self.connection().prepare(
                    "SELECT s.title, -bm25(section_fts) FROM section_fts
                     JOIN sections s ON s.row = section_fts.rowid
                     WHERE section_fts MATCH ?1
                     ORDER BY rank, s.row LIMIT ?2",
                )?;
                let mut rows = stmt.query(params![fts_query, k as i64])?;
                while let Some(row) = rows.next()? {
                    out.push((row.get(0)?, row.get(1)?));
                }
            }
            Bm25Corpus::Concepts => {
                let mut stmt = self.connection().prepare(
                    "SELECT s.title, -bm25(concept_fts) FROM concept_fts
                     JOIN concepts c ON c.section_row = concept_fts.rowid
                     JOIN sections s ON s.row = c.section_row
                     WHERE concept_fts MATCH ?1
                     ORDER BY rank, c.section_row LIMIT ?2",
                )?;
                let mut rows = stmt.query(params![fts_query, k as i64])?;
                while let Some(row) = rows.next()? {
                    out.push((row.get(0)?, row.get(1)?));
                }
            }
        }
        Ok(out)
    }
}

impl GraphReader for KgStore {
    fn noise_floor(&self) -> StoreResult<Option<f64>> {
        KgStore::noise_floor(self)
    }

    fn entity(&self, id: &str) -> StoreResult<Option<Entity>> {
        KgStore::entity(self, id)
    }

    fn section(&self, title: &str) -> StoreResult<Option<SectionInfo>> {
        KgStore::section(self, title)
    }

    fn bm25_entity_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        KgStore::bm25_entity_search(self, query, k)
    }

    fn bm25_section_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        KgStore::bm25_section_search(self, query, k)
    }

    fn bm25_concept_search(&self, query: &str, k: usize) -> StoreResult<Vec<(String, f64)>> {
        KgStore::bm25_concept_search(self, query, k)
    }

    fn concept_shortlist(&self, query: &str, k: usize) -> StoreResult<Vec<String>> {
        KgStore::concept_shortlist(self, query, k)
    }

    fn neighbors(&self, ids: &[String]) -> StoreResult<HashMap<String, Vec<String>>> {
        KgStore::neighbors(self, ids)
    }

    fn relations_among(
        &self,
        ids: &HashSet<String>,
        limit: usize,
    ) -> StoreResult<Vec<(String, String, String)>> {
        KgStore::relations_among(self, ids, limit)
    }

    fn entity_primaries(&self, ids: &[String]) -> StoreResult<HashMap<String, EntityPrimary>> {
        KgStore::entity_primaries(self, ids)
    }

    fn resolve_section_fragment(&self, fragment: &str) -> StoreResult<Option<String>> {
        KgStore::resolve_section_fragment(self, fragment)
    }

    fn locate_direct_sections(
        &self,
        query: &str,
        max_sections: usize,
    ) -> StoreResult<Vec<String>> {
        KgStore::locate_direct_sections(self, query, max_sections)
    }

    fn stored_docs(&self) -> StoreResult<BTreeMap<String, Option<String>>> {
        KgStore::stored_docs(self)
    }
}
