//! Knowledge graph data types and the dev/test fixture container.
//!
//! `KnowledgeGraph` is no longer the build model: production writes and reads
//! [`crate::store::KgStore`] directly, and queries run against it through
//! [`crate::reader::GraphReader`]. What remains here is the shared data shape
//! (`Entity`, `Relation`, `SectionInfo`, `Concept`, `TopicInfo`), the corpus
//! text helpers, and a small mutable container that dev tools and tests use
//! as a fixture source for [`crate::store::KgStore::write_full`]. BTreeMaps
//! keep iteration deterministic everywhere (DD-9).

use crate::text::tokenize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub etype: String,
    pub desc: String,
    /// Supporting section titles (provenance — DD-3).
    pub sections: Vec<String>,
    pub source_doc: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub source: String,
    pub target: String,
    pub rel: String,
    /// Anchor section this triple was extracted from.
    pub section: String,
    pub source_doc: Option<String>,
}

/// One concise glossary concept for a section (one per section, generated
/// during indexing). `name` is the routing label, `summary` the classifier
/// rubric, `terms` the lexical aliases (including lay synonyms) that let the
/// BM25 concept prefilter catch vague questions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Concept {
    pub name: String,
    pub summary: String,
    pub terms: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionInfo {
    pub topic: Option<String>,
    pub entity_ids: Vec<String>,
    /// Section concept for query routing; absent on sections whose extraction
    /// was skipped or that predate concept recording.
    pub concept: Option<Concept>,
    pub text: String,
    pub source_doc: String,
    /// 1-based line of the section's first content line in the source file.
    pub start_line: usize,
    /// 1-based line of the section's last content line (inclusive).
    pub end_line: usize,
    /// Content hash for incremental sync (DD-16).
    pub content_hash: String,
    /// True when extraction produced no usable payload. The section keeps its
    /// text and provenance but must re-extract on the next build; this flag
    /// keeps the sync diff from treating it as a legacy hash-less section.
    pub retry_pending: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicInfo {
    pub sections: Vec<String>,
}

/// The whole graph model. Field names mirror kg_demo's `graph.json` shape so
/// the ported pipeline keeps its terminology.
#[derive(Clone, Debug, Default)]
pub struct KnowledgeGraph {
    pub entities: BTreeMap<String, Entity>,
    pub relations: Vec<Relation>,
    pub sections: BTreeMap<String, SectionInfo>,
    pub topics: BTreeMap<String, TopicInfo>,
    pub noise_floor: Option<f64>,
    /// SHA-256 of each source document's raw bytes at build time, keyed by
    /// graph-root-relative rel path (`content.md` for a subfolder graph).
    /// Empty on graphs built before this was recorded — the staleness check
    /// then falls back to file existence.
    pub source_hashes: BTreeMap<String, String>,
}

/// Text fed to the entity BM25 corpus (`name type desc`). Shared by the
/// in-memory fit and the SQLite writer so both index the same corpus.
pub fn entity_text(e: &Entity) -> String {
    format!("{} {} {}", e.name, e.etype, e.desc)
}

/// Text fed to the concept BM25 corpus (`name summary terms`, falling back to
/// the section title when the concept text is empty).
pub fn concept_text(title: &str, info: &SectionInfo) -> String {
    let mut text = String::new();
    if let Some(concept) = &info.concept {
        text.push_str(&concept.name);
        text.push(' ');
        text.push_str(&concept.summary);
        text.push(' ');
        text.push_str(&concept.terms.join(" "));
    }
    if text.trim().is_empty() {
        text = title.to_string();
    }
    text
}

impl KnowledgeGraph {
    // -- mutation (dev/test fixtures only) -------------------------------

    /// Insert or merge an entity. Longer descriptions win; supporting
    /// sections and source documents accumulate without duplicates.
    pub fn add_entity(&mut self, id: &str, name: &str, etype: &str, desc: &str,
                      section_title: &str, source_doc: Option<&str>) {
        if let Some(existing) = self.entities.get_mut(id) {
            if desc.len() > existing.desc.len() {
                existing.desc = desc.to_string();
            }
            if let Some(doc) = source_doc {
                if !existing.source_doc.iter().any(|d| d == doc) {
                    existing.source_doc.push(doc.to_string());
                }
            }
            if !existing.sections.iter().any(|s| s == section_title) {
                existing.sections.push(section_title.to_string());
            }
            return;
        }
        self.entities.insert(id.to_string(), Entity {
            id: id.to_string(),
            name: name.to_string(),
            etype: etype.to_string(),
            desc: desc.to_string(),
            sections: vec![section_title.to_string()],
            source_doc: source_doc.map(|d| vec![d.to_string()]).unwrap_or_default(),
        });
    }

    /// Append a relation unless its exact triple already exists.
    pub fn add_relation(&mut self, source: &str, target: &str, rel: &str,
                        section_title: &str, source_doc: Option<&str>) {
        let exists = self.relations.iter()
            .any(|r| r.source == source && r.target == target && r.rel == rel);
        if exists {
            return;
        }
        self.relations.push(Relation {
            source: source.to_string(),
            target: target.to_string(),
            rel: rel.to_string(),
            section: section_title.to_string(),
            source_doc: source_doc.map(str::to_string),
        });
    }

    // -- corpus text + adjacency -----------------------------------------

    /// Tokenizer used by tests and the dev fixtures.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        tokenize(text)
    }

    /// Text fed to the section corpus (`title names text`). Shared by the
    /// `write_full` fixture writer and the native writer's `section_text`.
    pub fn section_text(&self, title: &str, info: &SectionInfo) -> String {
        let names: String = info.entity_ids.iter()
            .filter_map(|id| self.entities.get(id))
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        format!("{title} {names} {}", info.text)
    }

    /// Adjacent entity ids sorted lexicographically for determinism.
    pub fn neighbors(&self, entity_id: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for r in &self.relations {
            if r.source == entity_id {
                out.push(r.target.clone());
            } else if r.target == entity_id {
                out.push(r.source.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

