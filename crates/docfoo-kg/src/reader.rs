//! The read interface shared by the query pipeline.
//!
//! The query pipeline (`query::*`, `expand`) only talks to [`GraphReader`],
//! whose production implementation is [`crate::store::KgStore`]. Tests build
//! small stores with the dev fixture writer and read them through the same
//! interface. Every method is fallible because the SQLite implementation can
//! fail.

use crate::graph::{Entity, SectionInfo};
use crate::store::StoreError;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The primary (first) section of an entity plus that section's topic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityPrimary {
    pub section: String,
    pub topic: Option<String>,
}

pub trait GraphReader {
    fn noise_floor(&self) -> Result<Option<f64>, StoreError>;

    fn entity(&self, id: &str) -> Result<Option<Entity>, StoreError>;

    fn section(&self, title: &str) -> Result<Option<SectionInfo>, StoreError>;

    fn bm25_entity_search(&self, query: &str, k: usize) -> Result<Vec<(String, f64)>, StoreError>;

    fn bm25_section_search(&self, query: &str, k: usize) -> Result<Vec<(String, f64)>, StoreError>;

    fn bm25_concept_search(&self, query: &str, k: usize) -> Result<Vec<(String, f64)>, StoreError>;

    /// Jev candidates: BM25 concept hits first, then remaining sections in
    /// title order up to `k`. Graphs with at most `k` sections therefore
    /// present every concept, exactly like the pre-SQLite pipeline.
    fn concept_shortlist(&self, query: &str, k: usize) -> Result<Vec<String>, StoreError>;

    /// Sorted, deduplicated neighbor ids per requested entity id.
    fn neighbors(&self, ids: &[String]) -> Result<HashMap<String, Vec<String>>, StoreError>;

    /// `(source name, relation, target name)` triples whose endpoints are all
    /// in `ids`, in relation insertion order, capped at `limit`.
    fn relations_among(
        &self,
        ids: &HashSet<String>,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, StoreError>;

    /// Primary section/topic per requested entity id (absent when the entity
    /// has no supporting section).
    fn entity_primaries(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, EntityPrimary>, StoreError>;

    /// Unique case-insensitive exact title match, else unique case-insensitive
    /// substring match (guide-pick fragment resolution).
    fn resolve_section_fragment(&self, fragment: &str) -> Result<Option<String>, StoreError>;

    /// Rare-token locator (DD-21).
    fn locate_direct_sections(
        &self,
        query: &str,
        max_sections: usize,
    ) -> Result<Vec<String>, StoreError>;

    /// Every document the graph knows about, mapped to its build-time hash
    /// (`None` for legacy sections without a recorded hash).
    fn stored_docs(&self) -> Result<BTreeMap<String, Option<String>>, StoreError>;
}
