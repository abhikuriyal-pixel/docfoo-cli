//! Seeding: BM25 pool, the DD-17 escalation gate, Jev concept routing and the
//! legacy LLM bridge fallback.
//!
//! Weak lexical seeding is recovered in two tiers:
//! 1. Jev concept routing (when enabled + configured): ONE System One request
//!    scores a BM25 shortlist of section concepts; picked concepts contribute
//!    their sections (pinned evidence) and their entities (leading seeds).
//! 2. The legacy lay→technical LLM bridge: when concept routing is disabled,
//!    unconfigured, fails, or yields no picks, ONE LLM call generates technical
//!    terms that re-seed BM25 (DD-17/DD-20 behaviour, preserved as the safety
//!    net so disabling Jev restores the previous pipeline exactly).
//!
//! Neither tier can fail a query: the fallback returns the base hits unchanged
//! and records the reason in the routing trace.

use super::stage::StageEvent;
use super::{round3, ConceptPick, GateInfo, RoutingInfo, StageSink};
use crate::decision::{DecisionAnswer, DecisionClient, DecisionQuestion, NoulCriteria};
use crate::graph::Concept;
use crate::llm::{extract_json, ChatClient};
use crate::reader::GraphReader;
use crate::store::StoreError;
use crate::tunables::QueryTunables;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Weight of a weak (pool-only) entity's topic vote (pipeline.py weak_w).
pub const WEAK_W: f64 = 0.25;

/// Merged section shortlist ceiling (bridge/lexical fallback).
const SECTION_MERGE_CAP: usize = 8;

// ---------------------------------------------------------------------------
// Topic votes (kg.py ancestors_for)
// ---------------------------------------------------------------------------

/// Every seed entity votes once for the topic of its primary (first)
/// section, weighted. Returns votes in first-encounter order; callers sort.
pub fn ancestors_for(
    kg: &dyn GraphReader,
    seed_ids: &[String],
    weights: &HashMap<String, f64>,
) -> Result<Vec<(String, f64)>, StoreError> {
    let primaries = kg.entity_primaries(seed_ids)?;
    let mut order: Vec<String> = vec![];
    let mut votes: HashMap<String, f64> = HashMap::new();
    for sid in seed_ids {
        let Some(primary) = primaries.get(sid) else { continue };
        let Some(topic) = primary.topic.as_ref() else { continue };
        if !votes.contains_key(topic) {
            order.push(topic.clone());
        }
        *votes.entry(topic.clone()).or_insert(0.0) +=
            weights.get(sid).copied().unwrap_or(1.0);
    }
    Ok(order.into_iter()
        .map(|tp| { let v = votes[&tp]; (tp, v) })
        .collect())
}

// ---------------------------------------------------------------------------
// Seed gate (DD-17)
// ---------------------------------------------------------------------------

pub struct GateEval {
    pub weak: bool,
    pub info: GateInfo,
}

fn seed_gate(ent_hits: &[(String, f64)], noise_floor: Option<f64>, t: &QueryTunables) -> GateEval {
    let s1 = ent_hits.first().map(|(_, s)| *s).unwrap_or(0.0);
    let n = ent_hits.len();
    match noise_floor.filter(|f| *f > 0.0) {
        // A graph without a measured floor cannot be score-judged; count only.
        None => GateEval {
            weak: ent_hits.is_empty() || n < t.seed_escalate_min_count,
            info: GateInfo {
                s1: round3(s1), floor: None, ratio: None, n,
                rule: format!("n<{} (no floor)", t.seed_escalate_min_count),
            },
        },
        Some(floor) => {
            let ratio = s1 / floor;
            GateEval {
                weak: ent_hits.is_empty()
                    || ratio < t.escalate_floor_ratio
                    || n < t.seed_escalate_min_count,
                info: GateInfo {
                    s1: round3(s1),
                    floor: Some(round3(floor)),
                    ratio: Some(round3(ratio)),
                    n,
                    rule: format!("ratio<{}", t.escalate_floor_ratio),
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Score merging + elevation helpers (legacy bridge, DD-20)
// ---------------------------------------------------------------------------

/// Merge-scored keeps the max score per key and first-encounter order on
/// ties, then sorts score-descending (stable, like Python's sorted).
fn merge_scored(pairs_lists: Vec<Vec<(String, f64)>>, cap: usize) -> Vec<(String, f64)> {
    let mut merged: Vec<(String, f64)> = vec![];
    for pairs in pairs_lists {
        for (key, s) in pairs {
            match merged.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => { if s > e.1 { e.1 = s; } }
                None => merged.push((key, s)),
            }
        }
    }
    use std::cmp::Ordering as Rank;
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Rank::Equal));
    merged.truncate(cap);
    merged
}

/// Highest-scored keys from the per-term hit lists, excluding everything
/// already present in the base pass (`sorted(items, key=(-score, key))`).
fn top_new(
    pairs_lists: &[Vec<(String, f64)>],
    base: &HashSet<String>,
    cap: usize,
) -> Vec<String> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for pairs in pairs_lists {
        for (id, s) in pairs {
            if base.contains(id) { continue; }
            let e = scores.entry(id.clone()).or_insert(0.0);
            if *s > *e { *e = *s; }
        }
    }
    use std::cmp::Ordering as Rank;
    let mut items: Vec<(String, f64)> = scores.into_iter().collect();
    items.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(Rank::Equal).then_with(|| a.0.cmp(&b.0))
    });
    items.into_iter().take(cap).map(|(k, _)| k).collect()
}

/// DD-20 seed elevation: bridge-discovered entities lead the seed list,
/// ahead of lexical decoys that merely share surface words with the
/// question. Order within each group is preserved.
fn elevate_expanded(
    ent_hits: Vec<(String, f64)>,
    lead_ids: &HashSet<String>,
) -> Vec<(String, f64)> {
    let (lead, rest): (Vec<_>, Vec<_>) =
        ent_hits.into_iter().partition(|(i, _)| lead_ids.contains(i));
    lead.into_iter().chain(rest).collect()
}

// ---------------------------------------------------------------------------
// Jev concept routing + legacy bridge orchestration
// ---------------------------------------------------------------------------

/// The classifier-facing description of a concept: name + summary + the lay
/// search terms. Written as a rubric because "the description IS the
/// classifier".
fn concept_prompt(concept: &Concept) -> String {
    let terms = if concept.terms.is_empty() {
        String::new()
    } else {
        format!(" Search terms: {}.", concept.terms.join(", "))
    };
    format!("\"{}\" — {}.{}", concept.name, concept.summary, terms)
}

/// When lexical seeding is weak: try Jev concept routing, then the legacy
/// LLM bridge. Never fails the query — on any failure the base hits return
/// with the reason in the routing trace.
#[allow(clippy::too_many_arguments)]
pub fn maybe_route(
    query: &str,
    kg: &dyn GraphReader,
    t: &QueryTunables,
    decision: Option<&dyn DecisionClient>,
    llm: &dyn ChatClient,
    sink: StageSink<'_>,
    pass: u8,
    base_ent: Vec<(String, f64)>,
    base_sec: Vec<(String, f64)>,
    cancel: &AtomicBool,
) -> Result<(Vec<(String, f64)>, Vec<(String, f64)>, RoutingInfo), StoreError> {
    let eval = seed_gate(&base_ent, kg.noise_floor()?, t);
    sink(pass, StageEvent::Gate {
        fired: eval.weak,
        forced: false,
        rule: eval.info.rule.clone(),
        s1: eval.info.s1,
        floor: eval.info.floor,
        ratio: eval.info.ratio,
        n: eval.info.n,
        concept_routing: t.enable_concept_routing,
    });

    let mut info = RoutingInfo {
        used: false,
        gate: eval.info,
        candidates: 0,
        picks: vec![],
        lead_entities: vec![],
        sections: vec![],
        fallback: None,
        escalated: false,
        terms: vec![],
        model: String::new(),
        secs: 0.0,
    };

    if !eval.weak {
        emit_concepts(sink, pass, &info);
        return Ok((base_ent, base_sec, info));
    }

    // ---- tier 1: Jev concept routing (every concept, chunked) ----
    let mut fallback: Option<String> = None;
    let mut picks: Vec<(String, f64)> = vec![];
    if t.enable_concept_routing {
        match decision {
            Some(client) => {
                info.model = client.label();
                let sections: Vec<String> =
                    kg.concept_shortlist(query, t.concept_candidate_k)?;
                info.candidates = sections.len();
                if sections.is_empty() {
                    fallback = Some("no concepts indexed".to_string());
                } else {
                    let t0 = Instant::now();
                    let state = json!({ "query": query });
                    let batch = t.concept_batch_max.max(1);
                    let mut failure: Option<String> = None;
                    for chunk in sections.chunks(batch) {
                        if cancel.load(Ordering::Relaxed) {
                            failure = Some("cancelled".to_string());
                            break;
                        }
                        // One request per chunk, one Noul per concept: every
                        // question in a request is scored in parallel.
                        let mut questions: BTreeMap<String, DecisionQuestion> = BTreeMap::new();
                        for (i, title) in chunk.iter().enumerate() {
                            let described = kg.section(title)?
                                .and_then(|s| s.concept)
                                .map(|concept| concept_prompt(&concept))
                                .unwrap_or_else(|| format!("the section titled \"{title}\""));
                            questions.insert(format!("c{i}"), DecisionQuestion::Noul {
                                instructions: json!({
                                    "question": "Does the user's query ask about this concept?",
                                    "concept": described,
                                }),
                                criteria: Some(NoulCriteria {
                                    yes: Some("the query is about this concept".to_string()),
                                    no: Some("the query is unrelated to this concept".to_string()),
                                }),
                            });
                        }
                        match client.decide(&state, &questions, cancel) {
                            Ok(answers) => {
                                for (i, title) in chunk.iter().enumerate() {
                                    if let Some(DecisionAnswer::Noul { noul }) =
                                        answers.get(&format!("c{i}"))
                                    {
                                        if *noul >= t.concept_min_prob {
                                            picks.push((title.clone(), *noul));
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                failure = Some(match e {
                                    crate::decision::DecisionError::Cancelled =>
                                        "cancelled".to_string(),
                                    other => format!("Jev call failed: {other}"),
                                });
                                break;
                            }
                        }
                    }
                    info.secs = t0.elapsed().as_secs_f64();
                    use std::cmp::Ordering as Rank;
                    picks.sort_by(|a, b| {
                        b.1.partial_cmp(&a.1).unwrap_or(Rank::Equal)
                            .then_with(|| a.0.cmp(&b.0))
                    });
                    picks.truncate(t.concept_max_picks);
                    if picks.is_empty() {
                        fallback = failure
                            .or_else(|| Some("no concept above threshold".to_string()));
                    } else {
                        // Picks from earlier chunks are usable; a later-batch
                        // failure stays visible in the trace.
                        info.fallback = failure;
                    }
                }
            }
            None => fallback = Some("no API key configured".to_string()),
        }
    } else {
        fallback = Some("concept routing disabled".to_string());
    }

    if !picks.is_empty() {
        // Concept sections lead the evidence queue; their entities lead the
        // seed list (semantic picks outrank lexical base hits).
        let cap = 2 * t.seed_pool_k;
        let mut lead_entities: Vec<String> = vec![];
        let mut merged: Vec<(String, f64)> = vec![];
        let mut seen: HashSet<String> = HashSet::new();
        for (title, prob) in &picks {
            if let Some(sec) = kg.section(title)? {
                for eid in &sec.entity_ids {
                    if kg.entity(eid)?.is_some() && seen.insert(eid.clone()) {
                        lead_entities.push(eid.clone());
                        merged.push((eid.clone(), *prob));
                    }
                }
            }
        }
        for (id, score) in base_ent {
            if seen.insert(id.clone()) && merged.len() < cap {
                merged.push((id, score));
            }
        }
        merged.truncate(cap);
        lead_entities.truncate(cap);

        info.used = true;
        let mut pick_infos: Vec<ConceptPick> = Vec::new();
        for (title, prob) in &picks {
            let name = kg.section(title)?
                .and_then(|s| s.concept)
                .map(|c| c.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| title.clone());
            pick_infos.push(ConceptPick {
                section: title.clone(),
                name,
                prob: round3(*prob),
            });
        }
        info.picks = pick_infos;
        info.lead_entities = lead_entities;

        let mut sec_merged: Vec<(String, f64)> =
            picks.iter().map(|(title, prob)| (title.clone(), *prob)).collect();
        info.sections = sec_merged.iter().map(|(t, _)| t.clone()).collect();
        for (title, score) in base_sec {
            if sec_merged.len() >= SECTION_MERGE_CAP {
                break;
            }
            if !sec_merged.iter().any(|(t, _)| t == &title) {
                sec_merged.push((title, score));
            }
        }
        emit_concepts(sink, pass, &info);
        return Ok((merged, sec_merged, info));
    }

    // ---- tier 2: legacy lay→technical bridge (safety net) ----
    info.fallback = fallback;
    emit_concepts(sink, pass, &info);
    escalate_with_llm(query, kg, t, llm, sink, pass, base_ent, base_sec, info, cancel)
}

/// Emit the `concepts` stage frame for a routing outcome (always one frame
/// per weak-seed attempt, so the trace keeps its fixed beat order).
fn emit_concepts(sink: StageSink<'_>, pass: u8, routing: &RoutingInfo) {
    let picks: Vec<super::stage::StageConceptPick> = routing.picks.iter()
        .take(super::stage::STAGE_CAP)
        .map(|p| super::stage::StageConceptPick {
            section: p.section.clone(),
            name: p.name.clone(),
            prob: p.prob,
        })
        .collect();
    sink(pass, StageEvent::Concepts {
        used: routing.used,
        model: routing.model.clone(),
        candidates: routing.candidates,
        picks,
        secs: routing.secs,
        error: routing.fallback.clone(),
    });
}

/// The legacy LLM lay→technical bridge (DD-17/DD-20): one call generates
/// technical terms, each re-seeds BM25, the merged hits lead with the
/// bridge-discovered entities. Only reached when Jev could not seed.
#[allow(clippy::too_many_arguments)]
fn escalate_with_llm(
    query: &str,
    kg: &dyn GraphReader,
    t: &QueryTunables,
    llm: &dyn ChatClient,
    sink: StageSink<'_>,
    pass: u8,
    base_ent: Vec<(String, f64)>,
    base_sec: Vec<(String, f64)>,
    mut info: RoutingInfo,
    cancel: &AtomicBool,
) -> Result<(Vec<(String, f64)>, Vec<(String, f64)>, RoutingInfo), StoreError> {
    let prompt = format!(
        "The user asks a lay question. List up to {} technical terms or short phrases that a reference work in the question's own field would use to discuss what the question is really about.\nQuestion: {}\nOutput ONLY a raw JSON object: {{\"terms\":[\"term1\",\"term2\"]}}",
        t.expansion_max_terms, query);

    let parsed = llm.chat(&[json!({ "role": "user", "content": prompt })], 300, None, cancel);
    let raw = match parsed {
        Ok(raw) => raw,
        Err(e) => {
            sink(pass, StageEvent::ExpansionFailed { message: e.to_string() });
            info.fallback = Some(match info.fallback {
                Some(reason) => format!("{reason}; LLM bridge failed: {e}"),
                None => format!("LLM bridge failed: {e}"),
            });
            return Ok((base_ent, base_sec, info));
        }
    };

    let data = extract_json(&raw);
    let terms: Vec<String> = data.and_then(|d| {
        let arr = if d.is_array() {
            d.as_array().cloned().unwrap_or_default()
        } else {
            d.get("terms").and_then(Value::as_array).cloned().unwrap_or_default()
        };
        (!arr.is_empty()).then(|| {
            arr.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>()
        })
    }).map(|ts| {
        ts.into_iter().map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .take(t.expansion_max_terms)
            .collect()
    }).unwrap_or_default();

    if terms.is_empty() {
        sink(pass, StageEvent::Expansion {
            escalated: true, terms: vec![], entities: vec![], sections: vec![],
            error: Some("parse failure or empty term list".to_string()),
        });
        return Ok((base_ent, base_sec, info));
    }

    let per_term_ent: Vec<Vec<(String, f64)>> = terms
        .iter()
        .map(|term| kg.bm25_entity_search(term, 5))
        .collect::<Result<_, _>>()?;
    let per_term_sec: Vec<Vec<(String, f64)>> = terms
        .iter()
        .map(|term| kg.bm25_section_search(term, 3))
        .collect::<Result<_, _>>()?;

    // DD-20 bookkeeping: base-pass members so bridge-only discoveries can be
    // separated out for elevation.
    let base_eids: HashSet<String> = base_ent.iter().map(|(k, _)| k.clone()).collect();
    let base_titles: HashSet<String> = base_sec.iter().map(|(k, _)| k.clone()).collect();

    let mut ent_lists = vec![base_ent];
    ent_lists.extend(per_term_ent.iter().cloned());
    let ent_merged = merge_scored(ent_lists, 2 * t.seed_pool_k);

    let mut sec_lists = vec![base_sec];
    sec_lists.extend(per_term_sec.iter().cloned());
    let sec_merged = merge_scored(sec_lists, SECTION_MERGE_CAP);

    let lead_entities = top_new(&per_term_ent, &base_eids, 15);
    let lead_sections = top_new(&per_term_sec, &base_titles, 8);

    let lead_set: HashSet<String> = lead_entities.iter().cloned().collect();
    let ent_elevated = elevate_expanded(ent_merged, &lead_set);

    info.escalated = true;
    info.terms = terms.clone();
    info.lead_entities = lead_entities.clone();
    info.sections = lead_sections.clone();

    sink(pass, StageEvent::Expansion {
        escalated: true,
        terms,
        entities: lead_entities,
        sections: lead_sections,
        error: None,
    });
    Ok((ent_elevated, sec_merged, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::DecisionError;
    use crate::graph::{KnowledgeGraph, SectionInfo};
    use crate::test_support::StoreFixture;
    use crate::llm::LlmError;

    /// Routes by concept-name substring, exactly like the real call routes by
    /// question name; counts calls so gate-pass paths can prove none happened.
    struct FakeDecision {
        hits: Vec<(&'static str, f64)>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeDecision {
        fn new(hits: Vec<(&'static str, f64)>) -> Self {
            FakeDecision { hits, calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl DecisionClient for FakeDecision {
        fn decide(
            &self,
            _state: &Value,
            questions: &BTreeMap<String, DecisionQuestion>,
            _cancel: &AtomicBool,
        ) -> Result<BTreeMap<String, DecisionAnswer>, DecisionError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut out = BTreeMap::new();
            for (key, question) in questions {
                let text = serde_json::to_string(question).unwrap_or_default();
                let noul = self.hits.iter()
                    .find(|(needle, _)| text.contains(needle))
                    .map(|(_, p)| *p)
                    .unwrap_or(0.0);
                out.insert(key.clone(), DecisionAnswer::Noul { noul });
            }
            Ok(out)
        }

        fn label(&self) -> String {
            "fake-jev".to_string()
        }
    }

    /// Panics if the gate-pass path actually calls the classifier.
    struct NoDecision;

    impl DecisionClient for NoDecision {
        fn decide(
            &self,
            _state: &Value,
            _questions: &BTreeMap<String, DecisionQuestion>,
            _cancel: &AtomicBool,
        ) -> Result<BTreeMap<String, DecisionAnswer>, DecisionError> {
            panic!("this path must not call the decision client");
        }
    }

    struct FailingDecision;

    impl DecisionClient for FailingDecision {
        fn decide(
            &self,
            _state: &Value,
            _questions: &BTreeMap<String, DecisionQuestion>,
            _cancel: &AtomicBool,
        ) -> Result<BTreeMap<String, DecisionAnswer>, DecisionError> {
            Err(DecisionError::Transport("connection refused".to_string()))
        }
    }

    /// Replies with a fixed string to the legacy bridge prompt.
    struct FakeLlm {
        reply: String,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeLlm {
        fn new(reply: &str) -> Self {
            FakeLlm { reply: reply.to_string(), calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl ChatClient for FakeLlm {
        fn chat(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _response_format: Option<Value>,
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.reply.clone())
        }

        fn chat_stream(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _on_delta: &mut dyn FnMut(&str),
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Ok(self.reply.clone())
        }
    }

    /// Panics if the LLM is called (concept-routed and gate-pass paths).
    struct NoLlm;

    impl ChatClient for NoLlm {
        fn chat(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _response_format: Option<Value>,
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            panic!("this path must not call the LLM");
        }

        fn chat_stream(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _on_delta: &mut dyn FnMut(&str),
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            panic!("this path must not call the LLM");
        }
    }

    struct FailingLlm;

    impl ChatClient for FailingLlm {
        fn chat(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _response_format: Option<Value>,
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Err(LlmError::Failed("bridge down".to_string()))
        }

        fn chat_stream(
            &self,
            _messages: &[Value],
            _max_tokens: u32,
            _on_delta: &mut dyn FnMut(&str),
            _cancel: &AtomicBool,
        ) -> Result<String, LlmError> {
            Err(LlmError::Failed("bridge down".to_string()))
        }
    }

    fn fixture_kg() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::default();
        kg.add_entity("CONCEPT_cache", "Cache", "CONCEPT", "fast memory near cpu", "S1", None);
        kg.add_entity("DEVICE_cpu", "CPU", "DEVICE", "processor core", "S1", None);
        kg.add_entity("CONCEPT_ram", "RAM", "CONCEPT", "main memory bank", "S2", None);
        kg.add_relation("DEVICE_cpu", "CONCEPT_cache", "USES", "S1", None);
        kg.add_relation("CONCEPT_cache", "CONCEPT_ram", "ENABLES", "S2", None);
        kg.sections.insert("S1".into(), SectionInfo {
            topic: Some("[CO] Chapter 3".into()),
            entity_ids: vec!["CONCEPT_cache".into(), "DEVICE_cpu".into()],
            text: "Cache stores instructions close to the CPU pipeline. ".repeat(10),
            source_doc: "d.md".into(),
            concept: Some(Concept {
                name: "Cache hierarchy".into(),
                summary: "How caches keep the pipeline fed".into(),
                terms: vec!["fast memory".into(), "memory speed".into()],
            }),
            ..Default::default()
        });
        kg.sections.insert("S2".into(), SectionInfo {
            topic: Some("[CO] Chapter 4".into()),
            entity_ids: vec!["CONCEPT_ram".into()],
            text: "Random access memory holds running programs in main storage cells.".repeat(6),
            source_doc: "d.md".into(),
            concept: Some(Concept {
                name: "Main memory".into(),
                summary: "Working storage for running programs".into(),
                terms: vec!["ram".into(), "storage".into()],
            }),
            content_hash: String::new(),
            ..Default::default()
        });
        // Filler keeps exclusive terms' IDF positive (two docs give idf 0).
        kg.sections.insert("S3".into(), SectionInfo {
            topic: Some("[CO] Chapter 5".into()),
            text: "Electric motors convert electrical energy into torque.".repeat(6),
            source_doc: "d.md".into(),
            concept: Some(Concept {
                name: "Electric motors".into(),
                summary: "Torque production and control".into(),
                terms: vec!["motor".into(), "torque".into()],
            }),
            content_hash: String::new(),
            ..Default::default()
        });
        kg.noise_floor = Some(5.0);
        kg
    }

    fn fixture() -> StoreFixture {
        StoreFixture::new("seeds", &fixture_kg())
    }

    fn weak_base() -> Vec<(String, f64)> {
        vec![("CONCEPT_ram".into(), 1.0)]
    }

    #[test]
    fn seed_gate_rules_follow_the_reference() {
        let t = QueryTunables::default();
        // strong hits above the floor with enough of them pass
        let hits = vec![("A".into(), 9.0), ("B".into(), 4.0), ("C".into(), 4.0)];
        assert!(!seed_gate(&hits, Some(5.0), &t).weak);
        // best score below ESCALATE_FLOOR_RATIO * floor fires
        let weak_hits = vec![("A".into(), 4.0)];
        assert!(seed_gate(&weak_hits, Some(5.0), &t).weak);
        // too few hits fire regardless of score
        assert!(seed_gate(&weak_hits, Some(50.0), &t).weak);
        // no floor: count rule only
        let g = seed_gate(&weak_hits, None, &t);
        assert!(g.weak);
        assert!(g.info.rule.contains("no floor"));
    }

    #[test]
    fn ancestors_vote_for_primary_section_topics_with_weights() {
        let fx = fixture();
        let kg = &fx.store;
        let seeds = vec!["CONCEPT_cache".to_string(), "DEVICE_cpu".to_string()];
        let weights = HashMap::from([
            ("CONCEPT_cache".to_string(), 2.0),
            ("DEVICE_cpu".to_string(), 1.0),
        ]);
        let votes = ancestors_for(kg, &seeds, &weights).unwrap();
        assert_eq!(votes, vec![("[CO] Chapter 3".to_string(), 3.0)]);
    }

    /// Gate-pass path never touches the classifier or the LLM: the sink sees
    /// exactly `gate` then `concepts` (used=false), hits unchanged.
    #[test]
    fn gate_pass_emits_gate_then_concepts_without_calls() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let hits: Vec<(String, f64)> = vec![
            ("CONCEPT_cache".into(), 9.0),
            ("DEVICE_cpu".into(), 4.0),
            ("CONCEPT_ram".into(), 4.0),
        ];
        let mut events: Vec<(&'static str, String)> = vec![];
        let cancel = AtomicBool::new(false);
        let (out_ent, out_sec, routing) = maybe_route(
            "cpu cache",
            kg,
            &t,
            Some(&NoDecision),
            &NoLlm,
            &mut |_, ev| events.push(ev.log_parts()),
            1,
            hits.clone(),
            vec![("S1".into(), 1.0)],
            &cancel,
        )
        .unwrap();
        assert_eq!(events, vec![
            ("gate", "ratio<1.1 s1=9.000 floor=5.000 ratio=1.800 n=3 -> pass".to_string()),
            ("concepts", "used=false candidates=0 picks=0".to_string()),
        ]);
        assert_eq!(out_ent, hits);
        assert_eq!(out_sec, vec![("S1".to_string(), 1.0)]);
        assert!(!routing.used);
        assert!(!routing.escalated);
        assert!(routing.fallback.is_none());
    }

    #[test]
    fn weak_seeds_route_concepts_ahead_of_lexical_hits() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let decision = FakeDecision::new(vec![("Cache hierarchy", 0.9), ("Main memory", 0.2)]);
        let base: Vec<(String, f64)> = vec![
            ("CONCEPT_ram".into(), 1.0),
            ("CONCEPT_cache".into(), 0.8),
        ];
        let cancel = AtomicBool::new(false);
        let (ent, sec, routing) = maybe_route(
            "fast memory",
            kg,
            &t,
            Some(&decision),
            &NoLlm,
            &mut |_, _| {},
            1,
            base,
            vec![("S2".into(), 0.5)],
            &cancel,
        )
        .unwrap();
        assert_eq!(decision.calls(), 1);
        assert!(routing.used);
        assert!(routing.model == "fake-jev");
        assert_eq!(routing.picks.len(), 1, "0.2 is below the default threshold");
        assert_eq!(routing.picks[0].section, "S1");
        assert_eq!(routing.picks[0].name, "Cache hierarchy");
        // concept entities lead; RAM (base-only) follows; cache is not duplicated
        let ids: Vec<&str> = ent.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, vec!["CONCEPT_cache", "DEVICE_cpu", "CONCEPT_ram"]);
        assert_eq!(ent[0].1, 0.9, "lead entities carry the concept probability");
        assert_eq!(routing.lead_entities, vec!["CONCEPT_cache", "DEVICE_cpu"]);
        // concept sections lead the section list, lexical fallback follows
        assert_eq!(sec[0].0, "S1");
        assert!(sec.iter().any(|(t, _)| t == "S2"));
        assert!(!routing.escalated);
        assert!(routing.fallback.is_none());
    }

    /// Every concept within the candidate budget is sent — split across
    /// requests by `concept_batch_max` — and picks from all chunks merge.
    /// The fixture has fewer sections than the budget, so all are candidates.
    #[test]
    fn concepts_are_chunked_across_requests_within_the_budget() {
        let fx = fixture();
        let kg = &fx.store; // 3 sections with concepts
        let mut t = QueryTunables::default();
        t.concept_batch_max = 2;
        t.concept_min_prob = 0.0; // every answer clears the threshold
        let decision = FakeDecision::new(vec![]);
        let cancel = AtomicBool::new(false);
        let (_ent, _sec, routing) = maybe_route(
            "anything at all",
            kg,
            &t,
            Some(&decision),
            &NoLlm,
            &mut |_, _| {},
            1,
            weak_base(),
            vec![],
            &cancel,
        )
        .unwrap();
        assert_eq!(decision.calls(), 2, "3 concepts / batch 2 => 2 requests");
        assert!(routing.used);
        assert_eq!(routing.candidates, 3, "all concepts are candidates");
        assert_eq!(routing.picks.len(), 3);
    }

    /// Jev finds nothing → the legacy bridge still runs and elevates its
    /// discoveries ahead of the lexical base hits.
    #[test]
    fn no_picks_falls_back_to_the_llm_bridge() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let decision = FakeDecision::new(vec![]); // all 0.0
        let llm = FakeLlm::new(r#"{"terms":["cache"]}"#);
        let cancel = AtomicBool::new(false);
        let (ent, _sec, routing) = maybe_route(
            "memory",
            kg,
            &t,
            Some(&decision),
            &llm,
            &mut |_, _| {},
            1,
            weak_base(),
            vec![],
            &cancel,
        )
        .unwrap();
        assert!(routing.used == false);
        assert!(routing.escalated, "bridge must run when Jev yields no picks");
        assert_eq!(routing.terms, vec!["cache".to_string()]);
        assert_eq!(llm.calls(), 1);
        assert!(routing.fallback.as_deref().unwrap_or("").contains("no concept above threshold"));
        // bridge-discovered cache entity leads the seeds
        assert_eq!(ent.first().map(|(i, _)| i.as_str()), Some("CONCEPT_cache"));
    }

    #[test]
    fn decision_failure_falls_back_to_the_llm_bridge() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let llm = FakeLlm::new(r#"{"terms":["cache"]}"#);
        let cancel = AtomicBool::new(false);
        let (ent, _sec, routing) = maybe_route(
            "memory",
            kg,
            &t,
            Some(&FailingDecision),
            &llm,
            &mut |_, _| {},
            1,
            weak_base(),
            vec![],
            &cancel,
        )
        .unwrap();
        assert!(!routing.used);
        assert!(routing.escalated);
        assert_eq!(llm.calls(), 1);
        assert!(routing.fallback.as_deref().unwrap_or("").contains("Jev call failed"));
        assert_eq!(ent.first().map(|(i, _)| i.as_str()), Some("CONCEPT_cache"));
    }

    /// Both recovery tiers down → base hits untouched, reasons recorded.
    #[test]
    fn bridge_failure_returns_base_hits_unchanged() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let decision = FakeDecision::new(vec![]);
        let cancel = AtomicBool::new(false);
        let base = weak_base();
        let (ent, sec, routing) = maybe_route(
            "memory",
            kg,
            &t,
            Some(&decision),
            &FailingLlm,
            &mut |_, _| {},
            1,
            base.clone(),
            vec![("S2".into(), 0.5)],
            &cancel,
        )
        .unwrap();
        assert!(!routing.used);
        assert!(!routing.escalated);
        assert_eq!(ent, base);
        assert_eq!(sec, vec![("S2".to_string(), 0.5)]);
        let fallback = routing.fallback.unwrap_or_default();
        assert!(fallback.contains("no concept above threshold"));
        assert!(fallback.contains("LLM bridge failed"));
    }

    /// The user's switch: disabling Jev restores the legacy pipeline exactly —
    /// the classifier is never called and the LLM bridge takes over.
    #[test]
    fn disabled_routing_restores_the_llm_bridge() {
        let fx = fixture();
        let kg = &fx.store;
        let mut t = QueryTunables::default();
        t.enable_concept_routing = false;
        let llm = FakeLlm::new(r#"{"terms":["cache"]}"#);
        let cancel = AtomicBool::new(false);
        let base = weak_base();
        let (ent, _sec, routing) = maybe_route(
            "memory",
            kg,
            &t,
            Some(&NoDecision), // panics if called
            &llm,
            &mut |_, _| {},
            1,
            base,
            vec![],
            &cancel,
        )
        .unwrap();
        assert!(!routing.used);
        assert!(routing.escalated);
        assert_eq!(llm.calls(), 1);
        assert_eq!(routing.terms, vec!["cache".to_string()]);
        assert!(routing.fallback.as_deref().unwrap_or("").contains("disabled"));
        assert_eq!(ent.first().map(|(i, _)| i.as_str()), Some("CONCEPT_cache"));
    }
}
