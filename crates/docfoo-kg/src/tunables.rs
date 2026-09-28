//! User-tunable knobs — indexing and querying live in separate groups.
//!
//! `IndexSettings` shapes what the graph contains (vocabulary, prompt, entity
//! budget). `QueryTunables` shapes retrieval and writing (seeding, traversal,
//! evidence, escalation). Both persist in `db/kg-settings.json` as
//! `{ "index": { … }, "query": { … } }`; missing fields fall back to the
//! defaults here (`#[serde(default)]` at the struct level).
//!
//! Indexing runs on cloud models, so the model-specific guardrails that used
//! to live in this struct (local-endpoint concurrency, extraction ceilings,
//! hollow-section recovery) are gone. Query guardrails are untouched because
//! small local models remain a first-class target there.

use serde::{Deserialize, Serialize};

/// Graph-shaping settings for indexing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexSettings {
    /// Extra user instructions appended to the built-in extraction prompt.
    pub index_prompt: String,
    /// Closed entity-type vocabulary (schema-locked, DD-2).
    pub entity_types: Vec<String>,
    /// Closed relation vocabulary (schema-locked, DD-2).
    pub relations: Vec<String>,
    /// Max entities per section extraction call.
    pub max_entities_per_section: usize,
    /// Load-bearing items when a section is list-heavy.
    pub priority_entity_count: usize,
    /// Fingerprint of the vocabulary used for the last build; when it no
    /// longer matches the current vocab the next build is forced fresh.
    pub vocab_fingerprint: String,
}

impl Default for IndexSettings {
    fn default() -> Self {
        IndexSettings {
            index_prompt: String::new(),
            entity_types: crate::config::ENTITY_TYPES.iter().map(|s| s.to_string()).collect(),
            relations: crate::config::RELATIONS.iter().map(|s| s.to_string()).collect(),
            max_entities_per_section: crate::config::MAX_ENTITIES_PER_SECTION,
            priority_entity_count: crate::config::PRIORITY_ENTITY_COUNT,
            vocab_fingerprint: String::new(),
        }
    }
}

impl IndexSettings {
    /// Sanitize loaded values so a hand-edited settings file cannot break the
    /// extraction schema downstream.
    pub fn clamped(mut self) -> Self {
        self.max_entities_per_section = self.max_entities_per_section.clamp(1, 100);
        self.priority_entity_count =
            self.priority_entity_count.clamp(1, self.max_entities_per_section);
        self.entity_types = normalize_vocab(self.entity_types, &crate::config::ENTITY_TYPES);
        self.relations = normalize_vocab(self.relations, &crate::config::RELATIONS);
        self
    }
}

/// Retrieval + writing settings for KG queries.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QueryTunables {
    /// BM25 entity seeds kept (`SEED_POOL_K`).
    pub seed_pool_k: usize,
    /// Top-scored seeds that anchor graph traversal (`ANCHOR_K`).
    pub anchor_k: usize,
    /// BFS radius around anchors for deep queries (`HOP_DEPTH`; simple
    /// queries clamp to 1).
    pub hop_depth: usize,
    /// Section-vote weight of a node at hop h (`HOP_DECAY`).
    pub hop_decay: f64,
    /// Max new nodes expanded per node per hop (`NEIGHBOR_CAP`).
    pub neighbor_cap: usize,
    /// Fraction of ancestor votes the guide set must cover (`GUIDE_COVERAGE`).
    pub guide_coverage: f64,
    /// Guide cap for deep queries (`MAX_GUIDES`).
    pub max_guides: usize,
    /// Guide cap for simple queries (`MAX_GUIDES_SIMPLE`).
    pub max_guides_simple: usize,
    /// Entities picked per guide topic (`DESCENT_PICK_MAX`).
    pub descent_pick_max: usize,
    /// Full-section char budget when unlimited mode is off
    /// (`EVIDENCE_CHAR_BUDGET`).
    pub evidence_char_budget: usize,
    /// Ship EVERY queued section whole, ignoring the char budget
    /// (`EVIDENCE_BUDGET_UNLIMITED`, DD-24).
    pub evidence_budget_unlimited: bool,
    /// Relation-driven ordering of the delivery queue
    /// (`ENABLE_QUERY_EXPANSION`, DD-25).
    pub enable_query_expansion: bool,
    /// Boost scale relative to the BM25 probe score (`EXPANSION_WEIGHT`).
    pub expansion_weight: f64,
    /// Hard cap on the rare-token locator tier (`LOCATOR_MAX_SECTIONS`,
    /// DD-21).
    pub locator_max_sections: usize,
    /// Stable-sort the queue best-BM25-match-first before budget fill
    /// (`EVIDENCE_RELEVANCE_SORT`, DD-23).
    pub evidence_relevance_sort: bool,
    /// Width of the BM25 section probe used for ordering
    /// (`RELEVANCE_SEARCH_K`).
    pub relevance_search_k: usize,
    /// Technical terms per legacy LLM bridge call — the fallback path used
    /// when concept routing is disabled, unconfigured, or produces no picks.
    pub expansion_max_terms: usize,
    /// Jev concept routing when lexical seeding looks weak
    /// (`ENABLE_CONCEPT_ROUTING`) — replaces the generative bridge.
    pub enable_concept_routing: bool,
    /// Max concepts sent to Jev per request (one Noul each); the candidate
    /// shortlist is split into sequential requests.
    pub concept_batch_max: usize,
    /// Concept candidates considered per query. BM25 concept hits lead, then
    /// remaining sections fill the budget in title order, so graphs with at
    /// most this many sections present every concept (pre-SQLite behaviour).
    /// This is a cost/quality budget, not a graph-size switch.
    pub concept_candidate_k: usize,
    /// Minimum probability for a concept to seed the query.
    pub concept_min_prob: f64,
    /// Hard cap on routed concepts per query.
    pub concept_max_picks: usize,
    /// System One provider profile: `opencode` (OpenCode Zen), `openrouter`,
    /// or `typesafe`. Invalid values fall back to OpenCode Zen.
    pub concept_provider: String,
    /// Model id override; empty uses the provider profile's default
    /// (OpenCode Zen `jev-1.13-free`, OpenRouter `typesafe/jev-1.13`,
    /// TypeSafe `jev-1.13.0`).
    pub concept_model: String,
    /// API key override; empty uses the provider's environment variable
    /// (`OPENCODE_API_KEY` / `OPENROUTER_API_KEY` / `TYPESAFE_API_KEY`).
    pub concept_api_key: String,
    /// Fire the gate below this ratio of the measured noise floor
    /// (`ESCALATE_FLOOR_RATIO`, DD-17).
    pub escalate_floor_ratio: f64,
    /// ...or when fewer than this many positive hits
    /// (`SEED_ESCALATE_MIN_COUNT`).
    pub seed_escalate_min_count: usize,
    /// Allow chain-of-thought in the query-time writer/escalation calls.
    /// OFF (default) sends the pi thinking level "off"; ON lets the model's
    /// own default apply. Controlled ONLY by the Settings toggle, never by
    /// Buddy's ⚡ toggle.
    pub reasoning: bool,
}

impl Default for QueryTunables {
    fn default() -> Self {
        QueryTunables {
            seed_pool_k: 12,
            anchor_k: 5,
            hop_depth: 2,
            hop_decay: 0.5,
            neighbor_cap: 15,
            guide_coverage: 0.8,
            max_guides: 3,
            max_guides_simple: 2,
            descent_pick_max: 8,
            evidence_char_budget: 40_000,
            evidence_budget_unlimited: true,
            enable_query_expansion: true,
            expansion_weight: 0.35,
            locator_max_sections: 6,
            evidence_relevance_sort: true,
            relevance_search_k: 25,
            enable_concept_routing: true,
            expansion_max_terms: 8,
            concept_batch_max: 200,
            concept_candidate_k: 200,
            concept_min_prob: 0.35,
            concept_max_picks: 5,
            concept_provider: "opencode".to_string(),
            concept_model: String::new(),
            concept_api_key: String::new(),
            escalate_floor_ratio: 1.1,
            seed_escalate_min_count: 3,
            reasoning: false,
        }
    }
}

impl QueryTunables {
    /// Sanitize loaded values so a hand-edited settings file cannot produce
    /// division by zero, empty pools, or absurd budgets downstream.
    pub fn clamped(mut self) -> Self {
        let c = |v: &mut usize, lo, hi| *v = (*v).clamp(lo, hi);
        c(&mut self.seed_pool_k, 1, 100);
        c(&mut self.anchor_k, 1, self.seed_pool_k);
        self.hop_depth = self.hop_depth.clamp(0, 6);
        if !(0.0..=1.0).contains(&self.hop_decay) { self.hop_decay = 0.5; }
        c(&mut self.neighbor_cap, 1, 200);
        if !(0.0..=1.0).contains(&self.guide_coverage) { self.guide_coverage = 0.8; }
        c(&mut self.max_guides, 1, 20);
        c(&mut self.max_guides_simple, 1, self.max_guides);
        c(&mut self.descent_pick_max, 1, 100);
        c(&mut self.evidence_char_budget, 1_000, 10_000_000);
        let cf = |v: &mut f64, lo, hi| *v = v.clamp(lo, hi);
        cf(&mut self.expansion_weight, 0.0, 10.0);
        c(&mut self.locator_max_sections, 0, 50);
        c(&mut self.relevance_search_k, 1, 500);
        c(&mut self.concept_batch_max, 1, 500);
        c(&mut self.concept_candidate_k, 20, 2_000);
        cf(&mut self.concept_min_prob, 0.0, 1.0);
        c(&mut self.concept_max_picks, 1, 20);
        self.concept_provider = match self.concept_provider.trim().to_lowercase().as_str() {
            "openrouter" => "openrouter".to_string(),
            "typesafe" => "typesafe".to_string(),
            _ => "opencode".to_string(),
        };
        self.concept_model = self.concept_model.trim().to_string();
        self.concept_api_key = self.concept_api_key.trim().to_string();
        c(&mut self.expansion_max_terms, 1, 50);
        if self.escalate_floor_ratio < 0.0 { self.escalate_floor_ratio = 1.1; }
        c(&mut self.seed_escalate_min_count, 1, 100);
        self
    }
}

/// Both groups as persisted in `db/kg-settings.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KgSettings {
    pub index: IndexSettings,
    pub query: QueryTunables,
}

impl KgSettings {
    pub fn clamped(self) -> Self {
        KgSettings {
            index: self.index.clamped(),
            query: self.query.clamped(),
        }
    }
}

/// Trim/uppercase/dedup a vocabulary list; fall back to the built-in set
/// when nothing survives.
fn normalize_vocab(mut v: Vec<String>, fallback: &[&str]) -> Vec<String> {
    v = v.into_iter()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    v.sort();
    v.dedup();
    if v.is_empty() {
        v = fallback.iter().map(|s| s.to_string()).collect();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_gets_defaults() {
        let t: QueryTunables = serde_json::from_str(r#"{"hopDepth":3}"#).unwrap();
        assert_eq!(t.hop_depth, 3);
        assert_eq!(t.seed_pool_k, 12);
        assert!(!t.reasoning);
        assert!(t.evidence_budget_unlimited);
    }

    #[test]
    fn clamp_sanity() {
        let t = QueryTunables { seed_pool_k: 0, anchor_k: 99, hop_decay: 7.0, ..Default::default() };
        let t = t.clamped();
        assert_eq!(t.seed_pool_k, 1);
        assert_eq!(t.anchor_k, 1); // anchor <= pool
        assert_eq!(t.hop_decay, 0.5);

        let i = IndexSettings { max_entities_per_section: 0, priority_entity_count: 99, ..Default::default() };
        let i = i.clamped();
        assert_eq!(i.max_entities_per_section, 1);
        assert_eq!(i.priority_entity_count, 1);
    }

    #[test]
    fn camel_case_roundtrip() {
        let t = QueryTunables::default();
        let v = serde_json::to_value(&t).unwrap();
        assert!(v.get("seedPoolK").is_some());
        assert!(v.get("evidenceBudgetUnlimited").is_some());
        let back: QueryTunables = serde_json::from_value(v).unwrap();
        assert_eq!(back.seed_pool_k, t.seed_pool_k);
    }

    #[test]
    fn nested_shape_roundtrip() {
        let s = KgSettings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("index").is_some());
        assert!(v.get("query").is_some());
        assert_eq!(v["index"]["maxEntitiesPerSection"], 12);
        assert_eq!(v["query"]["seedPoolK"], 12);
        let back: KgSettings = serde_json::from_value(v).unwrap();
        assert_eq!(back.index.max_entities_per_section, 12);
        assert_eq!(back.query.seed_pool_k, 12);
    }
}
