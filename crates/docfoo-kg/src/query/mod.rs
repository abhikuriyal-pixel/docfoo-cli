//! Retrieval + synthesis — ported from kg_demo `pipeline.py`.
//!
//! One pass (`retrieve_pass`): depth classification → BM25 seeding →
//! seed-escalation gate (DD-17) with Jev concept routing when lexical seeds
//! are weak (see [`seeds`]) → ancestor voting for guide topics → guided
//! descent picks (0 LLM calls) → BFS traversal from anchors → rare-token
//! locator + full-text evidence delivery (DD-21/23/24/25, see [`deliver`]) →
//! streaming writer call ([`synthesis`]). There is no retry pass: a refusal
//! is reported as-is, and recovery happens at seeding time (concept routing),
//! never by regenerating an answer.

pub(crate) mod deliver;
mod heuristics;
mod seeds;
pub mod stage;
mod synthesis;

pub use heuristics::classify_depth;
pub use stage::StageEvent;
use stage::StageEvent as Ev;

use crate::decision::DecisionClient;
use crate::expand::RelationIndex;
use crate::llm::{ChatClient, LlmError};
use crate::reader::GraphReader;
use crate::store::StoreError;
use crate::tunables::QueryTunables;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// kg_demo GUIDE_BAN_TOPICS — topics that may never lead the guides.
const GUIDE_BAN_TOPICS: [&str; 1] = ["Other"];

/// Max KNOWN RELATIONS triples shipped to the writer.
const TRIPLES_CAP: usize = 40;

// ---------------------------------------------------------------------------
// Trace types (serialized into the UI's collapsible sources row)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct SeedInfo {
    pub id: String,
    pub name: String,
    pub score: f64,
}

#[derive(Clone, Serialize)]
pub struct AnchorInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Serialize)]
pub struct TopicVote {
    pub topic: String,
    pub score: f64,
}

#[derive(Clone, Serialize)]
pub struct GuidePick {
    pub topic: String,
    pub sections: Vec<String>,
    pub entities: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct EvidenceBlock {
    pub section: String,
    pub chars: usize,
    /// Source document rel path (empty on legacy graphs without provenance).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
}

#[derive(Clone, Serialize)]
pub struct GateInfo {
    pub s1: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    pub n: usize,
    pub rule: String,
}

/// One concept the classifier picked as relevant to the query.
#[derive(Clone, Serialize)]
pub struct ConceptPick {
    pub section: String,
    pub name: String,
    pub prob: f64,
}

/// Outcome of the weak-seed routing stage: whether Jev was consulted, what it
/// picked, and (on failure) why the pipeline fell back to lexical seeds.
#[derive(Clone, Serialize)]
pub struct RoutingInfo {
    pub used: bool,
    pub gate: GateInfo,
    pub candidates: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picks: Vec<ConceptPick>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lead_entities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// True when the legacy LLM lay→technical bridge supplied the recovery
    /// (Jev disabled/unconfigured/failed, or no concept cleared the gate).
    pub escalated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub secs: f64,
}

#[derive(Clone, Serialize)]
pub struct Timing {
    pub step: String,
    pub secs: f64,
}

#[derive(Clone, Serialize)]
pub struct Trace {
    pub query: String,
    pub depth: &'static str,
    pub seeds: Vec<SeedInfo>,
    pub anchors: Vec<AnchorInfo>,
    pub topic_votes: Vec<TopicVote>,
    pub guides: Vec<String>,
    pub guide_picks: Vec<GuidePick>,
    pub visited_count: usize,
    pub hop_depth: usize,
    pub evidence_sections: Vec<EvidenceBlock>,
    pub budget_dropped: usize,
    pub triples_used: usize,
    pub routing: RoutingInfo,
    pub timings: Vec<Timing>,
    pub total_seconds: f64,
}

pub struct QueryOutcome {
    pub answer: String,
    pub trace: Trace,
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("LLM call failed: {0}")]
    Llm(LlmError),
    #[error("the query was cancelled")]
    Cancelled,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
}

impl From<LlmError> for QueryError {
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::Cancelled => QueryError::Cancelled,
            other => QueryError::Llm(other),
        }
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Per-stage reporting sink: `(retrieval pass, typed event)`. Replaces the
/// legacy `FnMut(&str, &str)` string callback — [`StageEvent::log_parts`]
/// reproduces those log lines byte-for-byte, and every emission site below
/// matches its legacy position exactly so event order == historical log
/// order. Single-pass retrieval reports with `pass = 1`.
pub type StageSink<'a> = &'a mut dyn FnMut(u8, StageEvent);

/// Payload-bound id list for stage events (mirrors [`stage::STAGE_CAP`]).
fn stage_cap<T: Clone>(v: &[T]) -> Vec<T> {
    v.iter().take(stage::STAGE_CAP).cloned().collect()
}

// ---------------------------------------------------------------------------
// One retrieval pass + public entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn retrieve_pass(
    query: &str,
    kg: &dyn GraphReader,
    t: &QueryTunables,
    llm: &dyn ChatClient,
    decision: Option<&dyn DecisionClient>,
    sink: StageSink<'_>,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(&str),
) -> Result<(String, Trace), QueryError> {
    let t0 = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Err(QueryError::Cancelled);
    }
    let depth = classify_depth(query);
    sink(1, Ev::Depth { depth });

    // ---- BM25 seeding ----
    let tb = Instant::now();
    let base_ent = kg.bm25_entity_search(query, t.seed_pool_k)?;
    let base_sec = kg.bm25_section_search(query, 5)?;
    let bm25_secs = tb.elapsed().as_secs_f64();

    // ---- weak-seed gate + Jev concept routing + legacy bridge fallback ----
    let te0 = Instant::now();
    let (ent_hits, sec_hits, routing) = seeds::maybe_route(
        query, kg, t, decision, llm, sink, 1, base_ent, base_sec, cancel)?;
    let routing_secs = te0.elapsed().as_secs_f64();

    // ---- concept picks lead: pinned evidence shelves ----
    let mut priority_sections: Vec<String> = vec![];
    if !routing.sections.is_empty() {
        sink(1, Ev::Elevation {
            leads: routing.lead_entities.len(),
            shelves: routing.sections.len(),
        });
        priority_sections = routing.sections.iter().take(5).cloned().collect();
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(QueryError::Cancelled);
    }

    // ---- pool + anchors ----
    let pool_ids: Vec<String> = ent_hits.iter().map(|(e, _)| e.clone()).collect();
    let seen: HashSet<String> = pool_ids.iter().cloned().collect();
    let mut pool = pool_ids;
    for (title, _) in &sec_hits {
        let Some(info) = kg.section(title)? else { continue };
        for eid in &info.entity_ids {
            if kg.entity(eid)?.is_some() && !seen.contains(eid) {
                pool.push(eid.clone());
            }
        }
    }
    let anchors: Vec<String> =
        ent_hits.iter().take(t.anchor_k).map(|(e, _)| e.clone()).collect();
    let max_s = ent_hits.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
    let max_s = if max_s > 0.0 { max_s } else { 1.0 };
    let weights: HashMap<String, f64> = ent_hits.iter()
        .map(|(e, s)| (e.clone(), s / max_s)).collect();

    let mut trace_seeds: Vec<SeedInfo> = Vec::new();
    for (i, s) in &ent_hits {
        if let Some(ent) = kg.entity(i)? {
            trace_seeds.push(SeedInfo { id: i.clone(), name: ent.name, score: round3(*s) });
        }
    }
    let mut trace_anchors: Vec<AnchorInfo> = Vec::new();
    for i in &anchors {
        if let Some(ent) = kg.entity(i)? {
            trace_anchors.push(AnchorInfo { id: i.clone(), name: ent.name });
        }
    }
    sink(1, Ev::Bm25 {
        entity_hits: ent_hits.iter().take(stage::STAGE_CAP)
            .map(|(i, s)| stage::StageScoredEntity { id: i.clone(), score: round3(*s) })
            .collect(),
        section_hits: sec_hits.len(),
        pool_size: pool.len(),
    });
    // the final accent list: concept picks first, then the lexical seed order
    sink(1, Ev::seeds(
        &ent_hits.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
        &anchors,
        &routing.lead_entities,
    ));

    // ---- ancestor voting → guides ----
    let tc = Instant::now();
    let mut vote_weights: HashMap<String, f64> =
        pool.iter().map(|e| (e.clone(), seeds::WEAK_W)).collect();
    vote_weights.extend(weights.clone());
    let votes = seeds::ancestors_for(kg, &pool, &vote_weights)?;
    use std::cmp::Ordering as Rank;
    let mut votes_sorted = votes.clone();
    votes_sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Rank::Equal));
    let eligible_filtered: Vec<(String, f64)> = votes_sorted.iter()
        .filter(|(tp, _)| !GUIDE_BAN_TOPICS.contains(&tp.as_str()))
        .cloned()
        .collect();
    let eligible: Vec<(String, f64)> = if eligible_filtered.is_empty() {
        votes // fallback: unfiltered, unsorted (insertion order, like Python)
    } else {
        eligible_filtered
    };
    let total_votes: f64 = eligible.iter().map(|(_, v)| v).sum();
    let guide_cap = if depth == "deep" { t.max_guides } else { t.max_guides_simple };
    let mut guides: Vec<String> = vec![];
    let mut cum = 0.0;
    for (topic, v) in &eligible {
        guides.push(topic.clone());
        cum += v;
        if cum >= t.guide_coverage * total_votes || guides.len() >= guide_cap {
            break;
        }
    }
    let voting_secs = tc.elapsed().as_secs_f64();
    let trace_votes: Vec<TopicVote> = votes_sorted.iter()
        .map(|(tp, v)| TopicVote { topic: tp.clone(), score: round3(*v) }).collect();
    sink(1, Ev::votes(&votes_sorted, &guides));

    // ---- guided descent picks (0 LLM calls) ----
    let td = Instant::now();
    let mut sec_by_topic: HashMap<String, Vec<String>> = HashMap::new();
    for (title, _) in &sec_hits {
        let Some(tp) = kg.section(title)?.and_then(|i| i.topic) else { continue };
        if guides.contains(&tp) {
            let bucket = sec_by_topic.entry(tp).or_default();
            if bucket.len() < t.descent_pick_max {
                bucket.push(title.clone());
            }
        }
    }
    let hit_ids: Vec<String> = ent_hits.iter().map(|(e, _)| e.clone()).collect();
    let primaries = kg.entity_primaries(&hit_ids)?;
    let mut picks_by_topic: HashMap<String, Vec<String>> = HashMap::new();
    let mut picked_order: Vec<String> = vec![];
    for (eid, _) in &ent_hits {
        let Some(primary) = primaries.get(eid) else { continue };
        let tp = primary.topic.clone().unwrap_or_default();
        // setdefault semantics: the bucket exists regardless of guidance,
        // but only guided topics accumulate picks (pipeline.py parity)
        let bucket = picks_by_topic.entry(tp.clone()).or_default();
        if guides.contains(&tp) && bucket.len() < t.descent_pick_max {
            bucket.push(eid.clone());
            picked_order.push(eid.clone());
        }
    }
    let mut guide_picks: Vec<GuidePick> = vec![];
    for tp in &guides {
        let mut entities: Vec<String> = Vec::new();
        if let Some(ids) = picks_by_topic.get(tp) {
            for eid in ids {
                if let Some(ent) = kg.entity(eid)? {
                    entities.push(ent.name);
                }
            }
        }
        guide_picks.push(GuidePick {
            topic: tp.clone(),
            sections: sec_by_topic.get(tp).cloned().unwrap_or_default(),
            entities,
        });
    }
    sink(1, Ev::descent(&picked_order));
    let descent_secs = td.elapsed().as_secs_f64();

    // ---- traversal (BFS from anchors) ----
    let te = Instant::now();
    let mut visited: HashMap<String, usize> = HashMap::new();
    {
        let mut seed_seen: HashSet<String> = HashSet::new();
        for eid in picked_order.iter().chain(anchors.iter()) {
            if kg.entity(eid)?.is_some() && !seed_seen.contains(eid) {
                seed_seen.insert(eid.clone());
                visited.insert(eid.clone(), 0);
            }
        }
    }
    let bm25_scores: HashMap<String, f64> = ent_hits.iter().cloned().collect();
    let max_hops = if depth == "deep" { t.hop_depth } else { t.hop_depth.min(1) };
    let mut frontier: Vec<String> = Vec::new();
    for anchor in &anchors {
        if kg.entity(anchor)?.is_some() {
            frontier.push(anchor.clone());
        }
    }
    for hop in 1..=max_hops {
        let neighbors = kg.neighbors(&frontier)?;
        let mut nxt: Vec<String> = vec![];
        for node in &frontier {
            let mut taken = 0usize;
            for n in neighbors.get(node).map(|v| v.as_slice()).unwrap_or_default() {
                if taken >= t.neighbor_cap {
                    break;
                }
                if kg.entity(n)?.is_none() || visited.contains_key(n) {
                    continue;
                }
                if hop > 1 && bm25_scores.get(n).copied().unwrap_or(0.0) <= 0.0 {
                    continue;
                }
                visited.insert(n.clone(), hop);
                nxt.push(n.clone());
                taken += 1;
            }
        }
        let level_added = nxt;
        let level_added_capped = stage_cap(&level_added);
        frontier = level_added;
        if frontier.is_empty() {
            break;
        }
        // one frame per completed BFS level — the visualizer's hopping beat
        sink(1, Ev::hop(hop, &level_added_capped, visited.len()));
    }
    let visited_count = visited.len();
    let traversal_secs = te.elapsed().as_secs_f64();
    sink(1, Ev::Traversal { visited_count, hop_depth: max_hops });

    // ---- section scores (visit-weighted + probe bonus) + locator + delivery ----
    let tf = Instant::now();
    let mut sec_order: Vec<String> = vec![];
    let mut sec_scores_map: HashMap<String, f64> = HashMap::new();
    let bump = |title: &str, w: f64,
                order: &mut Vec<String>, map: &mut HashMap<String, f64>| {
        if !map.contains_key(title) {
            order.push(title.to_string());
        }
        *map.entry(title.to_string()).or_insert(0.0) += w;
    };
    for (v, hop) in &visited {
        let wv = if *hop == 0 { 1.0 } else { t.hop_decay.powi((*hop as i32) - 1) };
        if let Some(ent) = kg.entity(v)? {
            for s in &ent.sections {
                bump(s, wv, &mut sec_order, &mut sec_scores_map);
            }
        }
    }
    for (title, _) in kg.bm25_section_search(query, 3)? {
        if kg.section(&title)?.is_some() {
            bump(&title, 3.0, &mut sec_order, &mut sec_scores_map);
        }
    }
    let mut sec_scores: Vec<(String, f64)> = sec_order.into_iter()
        .map(|s| { let v = sec_scores_map[&s]; (s, v) })
        .collect();
    sec_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Rank::Equal));

    let direct_sections =
        kg.locate_direct_sections(query, t.locator_max_sections)?;
    sink(1, Ev::scored(&sec_scores, &direct_sections));

    let relevance_sort_active = t.evidence_relevance_sort && !query.is_empty();
    let ridx = if relevance_sort_active {
        Some(RelationIndex::build(kg, query, deliver::EXPANSION_ANCHOR_K)?)
    } else {
        None
    };
    let delivered = deliver::deliver_full_routed(
        kg, t, &guides, &guide_picks, &picks_by_topic, &sec_scores,
        &direct_sections, &priority_sections, query, ridx.as_ref())?;
    sink(1, Ev::delivered(&delivered.tier_order));
    sink(1, Ev::reorder(
        &delivered.blocks.iter().map(|(s, c, _)| (s.clone(), *c)).collect::<Vec<_>>(),
        &delivered.dropped.iter().map(|d| d.section.clone()).collect::<Vec<_>>(),
        relevance_sort_active,
    ));
    let evidence_secs = tf.elapsed().as_secs_f64();

    // ---- KNOWN RELATIONS triples of the visited subgraph ----
    let visited_ids: HashSet<String> = visited.keys().cloned().collect();
    let triple_lines: Vec<String> = kg
        .relations_among(&visited_ids, TRIPLES_CAP)?
        .into_iter()
        .map(|(source, rel, target)| format!("{source} -[{rel}]-> {target}"))
        .collect();

    let mut evidence_sections: Vec<EvidenceBlock> = Vec::new();
    for (section, chars, _) in &delivered.blocks {
        let info = kg.section(section)?;
        evidence_sections.push(EvidenceBlock {
            section: section.clone(),
            chars: *chars,
            doc: info.as_ref().map(|i| i.source_doc.clone()).unwrap_or_default(),
            start_line: info.as_ref().filter(|i| i.start_line > 0).map(|i| i.start_line),
            end_line: info.as_ref().filter(|i| i.end_line > 0).map(|i| i.end_line),
        });
    }
    sink(1, Ev::EvidenceSummary {
        sections: delivered.blocks.len(),
        chars: delivered.blocks.iter().map(|(_, c, _)| c).sum::<usize>(),
        dropped: delivered.dropped.len(),
        triples: triple_lines.len(),
    });

    // ---- streaming synthesis ----
    if cancel.load(Ordering::Relaxed) {
        return Err(QueryError::Cancelled);
    }
    let tg = Instant::now();
    sink(1, Ev::Synthesis { phase: "start", chars: None });
    let block_texts: Vec<String> =
        delivered.blocks.iter().map(|(_, _, b)| b.clone()).collect();
    let answer = synthesis::synthesize(llm, query, &block_texts, &triple_lines, cancel, on_delta)?;
    sink(1, Ev::Synthesis { phase: "end", chars: Some(answer.chars().count()) });
    let synth_secs = tg.elapsed().as_secs_f64();

    let total_seconds = t0.elapsed().as_secs_f64();
    let _ = synth_secs; // folded into total; kept local for parity with timings list
    sink(1, Ev::Done { total_secs: total_seconds });
    Ok((answer, Trace {
        query: query.to_string(),
        depth,
        seeds: trace_seeds,
        anchors: trace_anchors,
        topic_votes: trace_votes,
        guides,
        guide_picks,
        visited_count,
        hop_depth: max_hops,
        evidence_sections,
        budget_dropped: delivered.dropped.len(),
        triples_used: triple_lines.len(),
        routing,
        timings: vec![
            Timing { step: "bm25".into(), secs: bm25_secs },
            Timing { step: "routing".into(), secs: routing_secs },
            Timing { step: "ancestor_voting".into(), secs: voting_secs },
            Timing { step: "guided_descent".into(), secs: descent_secs },
            Timing { step: "traversal".into(), secs: traversal_secs },
            Timing { step: "evidence".into(), secs: evidence_secs },
            Timing { step: "synthesis".into(), secs: synth_secs },
        ],
        total_seconds,
    }))
}

/// Public entry point: one retrieval pass. Weak lexical seeding is recovered
/// inside the pass by Jev concept routing; there is no refusal retry — an
/// evidence gap is reported honestly by the writer.
#[allow(clippy::too_many_arguments)]
pub fn retrieve(
    query: &str,
    kg: &dyn GraphReader,
    t: &QueryTunables,
    llm: &dyn ChatClient,
    decision: Option<&dyn DecisionClient>,
    sink: StageSink<'_>,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(&str),
) -> Result<QueryOutcome, QueryError> {
    let (answer, trace) =
        retrieve_pass(query, kg, t, llm, decision, sink, cancel, on_delta)?;
    Ok(QueryOutcome { answer, trace })
}
