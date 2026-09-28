//! DD-25 relation-driven query expansion — ported from kg_demo
//! `expansion.py` (no LLM, no embeddings, no stats).
//!
//! Uses the two provenance structures the graph always had but retrieval
//! never touched: sections carry entity_ids (what each shelf talks about),
//! relations connect entities with typed edges. BM25-match ANCHOR entities
//! from the query, take their 1-hop neighbors as certified semantic
//! expansions, then boost every queued section that CONTAINS an anchor or
//! neighbor entity. Pure set algebra, millisecond cost, deterministic (DD-9).
//!
//! Neighbors come from the reader's indexed adjacency, so this no longer
//! builds a full-graph map per query.

use crate::reader::GraphReader;
use crate::store::StoreError;
use std::collections::{HashMap, HashSet};

pub struct RelationIndex {
    anchors: HashSet<String>,
    related: HashSet<String>,
}

impl RelationIndex {
    /// Match anchors with BM25 and collect their 1-hop neighbors.
    pub fn build(
        kg: &dyn GraphReader,
        query: &str,
        anchor_k: usize,
    ) -> Result<RelationIndex, StoreError> {
        let anchors: HashSet<String> = kg
            .bm25_entity_search(query, anchor_k)?
            .into_iter()
            .map(|(entity, _)| entity)
            .collect();
        let mut related: HashSet<String> = HashSet::new();
        if !anchors.is_empty() {
            let ids: Vec<String> = anchors.iter().cloned().collect();
            for neighbors in kg.neighbors(&ids)?.values() {
                related.extend(neighbors.iter().cloned());
            }
        }
        related.retain(|entity| !anchors.contains(entity));
        Ok(RelationIndex { anchors, related })
    }

    /// Normalized boost per queued section from entity overlap.
    pub fn section_boosts(
        &self,
        kg: &dyn GraphReader,
        queue: &[String],
        w_anchor: f64,
        w_related: f64,
    ) -> Result<HashMap<String, f64>, StoreError> {
        if self.anchors.is_empty() && self.related.is_empty() {
            return Ok(HashMap::new());
        }
        let mut raw: HashMap<String, f64> = HashMap::new();
        for sid in queue {
            let Some(info) = kg.section(sid)? else { continue };
            let direct = info
                .entity_ids
                .iter()
                .filter(|entity| self.anchors.contains(*entity))
                .count();
            let neighbors = info
                .entity_ids
                .iter()
                .filter(|entity| self.related.contains(*entity))
                .count();
            if direct > 0 || neighbors > 0 {
                raw.insert(
                    sid.clone(),
                    w_anchor * direct as f64 + w_related * neighbors as f64,
                );
            }
        }
        if raw.is_empty() {
            return Ok(HashMap::new());
        }
        let peak = raw.values().cloned().fold(f64::NEG_INFINITY, f64::max);
        if peak <= 0.0 {
            return Ok(HashMap::new());
        }
        Ok(raw
            .into_iter()
            .map(|(section, value)| (section, ((value / peak) * 1000.0).round() / 1000.0))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{KnowledgeGraph, SectionInfo};
    use crate::test_support::StoreFixture;

    fn fixture() -> StoreFixture {
        let mut kg = KnowledgeGraph::default();
        kg.add_entity("CONCEPT_cache", "cache", "CONCEPT", "fast memory", "S1", None);
        kg.add_entity("DEVICE_cpu", "cpu", "DEVICE", "processor", "S1", None);
        kg.add_entity("CONCEPT_ram", "ram", "CONCEPT", "main memory", "S2", None);
        kg.add_relation("DEVICE_cpu", "CONCEPT_cache", "USES", "S1", None);
        kg.add_relation("CONCEPT_cache", "CONCEPT_ram", "ENABLES", "S2", None);
        let s1 = SectionInfo { topic: Some("T".into()), entity_ids: vec![
            "CONCEPT_cache".into(), "DEVICE_cpu".into()], ..Default::default() };
        let s2 = SectionInfo { topic: Some("T".into()), entity_ids: vec![
            "CONCEPT_ram".into(), "CONCEPT_cache".into()], ..Default::default() };
        kg.sections.insert("S1".into(), s1);
        kg.sections.insert("S2".into(), s2);
        StoreFixture::new("expand", &kg)
    }

    #[test]
    fn expansion_set_finds_anchors_and_one_hop_neighbors() {
        let fx = fixture();
        let idx = RelationIndex::build(&fx.store, "ram", 1).unwrap();
        assert!(idx.anchors.contains("CONCEPT_ram"));
        assert!(idx.related.contains("CONCEPT_cache"), "1-hop neighbor via ENABLES");
        assert!(!idx.related.contains("CONCEPT_ram"), "anchors are removed from related");
    }

    #[test]
    fn section_boosts_normalize_to_peak() {
        let fx = fixture();
        let idx = RelationIndex::build(&fx.store, "ram", 1).unwrap();
        let boosts = idx
            .section_boosts(&fx.store, &["S1".into(), "S2".into()], 2.0, 1.0)
            .unwrap();
        // S2 holds ram (anchor, ×2) + cache (related, ×1) → raw 3 → peak 1.0;
        // S1 holds only cache (related) → raw 1 → 0.333 after rounding
        assert_eq!(boosts["S2"], 1.0);
        assert!((boosts["S1"] - 0.333).abs() < 0.001);
    }
}
