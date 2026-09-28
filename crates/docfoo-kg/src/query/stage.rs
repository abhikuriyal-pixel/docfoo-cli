//! Typed stage events for the KG visualizer (docs/kg-viz-plan.md Stage 0).
//!
//! The legacy string callbacks (`cb(step, message)`) reported every pipeline
//! moment to the log file only. These typed events carry the same moments —
//! plus richer data (seed/anchor/hop id lists, tier vs. relevance ordering) —
//! so the shell can forward them as `kg-query-event` frames with `pass` and a
//! monotonically increasing `seq`. Serialization contract: each event renders
//! as `{ "step": "<name>", "data": {...} }` with camelCase data keys; every
//! consumer must tolerate unknown steps (forward compat).
//!
//! [`StageEvent::log_parts`] reproduces the legacy log lines byte-for-byte
//! where they existed; events without a legacy counterpart use additive
//! lines, never edits to the old ones.

use serde::Serialize;

/// Payload bound: no stage event ships more than this many list entries.
pub const STAGE_CAP: usize = 64;

/// Scored-section ceiling of the `scored` event (top-of-list preview only).
const SCORED_CAP: usize = 40;

pub(crate) fn take_cap<T: Clone>(v: &[T]) -> Vec<T> {
    v.iter().take(STAGE_CAP).cloned().collect()
}

impl StageEvent {
    /// Cap-aware constructors keep the list/total pairs consistent at every
    /// emission site.
    pub(crate) fn seeds(ids: &[String], anchors: &[String], lead_ids: &[String]) -> Self {
        StageEvent::Seeds {
            ids: take_cap(ids),
            anchors: take_cap(anchors),
            lead_ids: take_cap(lead_ids),
        }
    }

    pub(crate) fn votes(votes_sorted: &[(String, f64)], guides: &[String]) -> Self {
        StageEvent::Votes {
            votes: votes_sorted.iter().take(STAGE_CAP)
                .map(|(t, s)| StageVote { topic: t.clone(), score: super::round3(*s) })
                .collect(),
            guides: take_cap(guides),
        }
    }

    pub(crate) fn descent(picked_order: &[String]) -> Self {
        StageEvent::Descent { picked_order: take_cap(picked_order) }
    }

    pub(crate) fn hop(hop: usize, added: &[String], visited_so_far: usize) -> Self {
        StageEvent::Hop { hop, added: take_cap(added), visited_so_far }
    }

    pub(crate) fn scored(sec_scores: &[(String, f64)], direct: &[String]) -> Self {
        StageEvent::Scored {
            order: sec_scores.iter().take(SCORED_CAP)
                .map(|(s, v)| StageScoredSection { section: s.clone(), score: super::round3(*v) })
                .collect(),
            direct: take_cap(direct),
        }
    }

    pub(crate) fn delivered(tier: &[String]) -> Self {
        StageEvent::Delivered { tier_order: take_cap(tier), tier_total: tier.len() }
    }

    pub(crate) fn reorder(
        final_blocks: &[(String, usize)],
        dropped_titles: &[String],
        reordered: bool,
    ) -> Self {
        StageEvent::Reorder {
            final_order: final_blocks.iter().take(STAGE_CAP)
                .map(|(s, c)| StageSectionChars { section: s.clone(), chars: *c })
                .collect(),
            final_total: final_blocks.len(),
            dropped: take_cap(dropped_titles),
            dropped_total: dropped_titles.len(),
            reordered,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StageScoredEntity {
    pub id: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StageVote {
    pub topic: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StageScoredSection {
    pub section: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StageSectionChars {
    pub section: String,
    pub chars: usize,
}

/// One concept pick from the weak-seed routing call.
#[derive(Clone, Debug, Serialize)]
pub struct StageConceptPick {
    pub section: String,
    pub name: String,
    pub prob: f64,
}

/// One retrieval-pipeline moment, tagged by step name.
///
/// Variants mirror the callback sites in `query/mod.rs` / `seeds.rs`; see
/// docs/kg-viz-plan.md §2 for the frontend choreography that consumes them.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "step", content = "data", rename_all = "camelCase")]
pub enum StageEvent {
    Depth {
        depth: &'static str,
    },
    /// Container-level `rename_all` does not reach struct-variant fields
    /// (serde internally-tagged quirk) — each multi-word variant carries its
    /// own `rename_all` so payload keys match the plan's contract table.
    #[serde(rename_all = "camelCase")]
    Bm25 {
        entity_hits: Vec<StageScoredEntity>,
        section_hits: usize,
        pool_size: usize,
    },
    #[serde(rename_all = "camelCase")]
    Gate {
        fired: bool,
        forced: bool,
        rule: String,
        s1: f64,
        floor: Option<f64>,
        ratio: Option<f64>,
        n: usize,
        /// Whether Jev concept routing is selected (drives the UI beat:
        /// which engine widens weak seeds).
        concept_routing: bool,
    },
    /// The weak-seed concept-routing call returned (any outcome).
    #[serde(rename_all = "camelCase")]
    Concepts {
        used: bool,
        model: String,
        candidates: usize,
        picks: Vec<StageConceptPick>,
        secs: f64,
        error: Option<String>,
    },
    /// The legacy lay→technical LLM bridge returned (fallback path when
    /// concept routing is disabled, unconfigured or yields no picks).
    Expansion {
        escalated: bool,
        terms: Vec<String>,
        entities: Vec<String>,
        sections: Vec<String>,
        error: Option<String>,
    },
    /// The bridge LLM call itself failed before any terms existed.
    ExpansionFailed {
        message: String,
    },
    Elevation {
        leads: usize,
        shelves: usize,
    },
    /// Final seed list after merge + DD-20 elevation (the accent moment).
    #[serde(rename_all = "camelCase")]
    Seeds {
        ids: Vec<String>,
        anchors: Vec<String>,
        lead_ids: Vec<String>,
    },
    Votes {
        votes: Vec<StageVote>,
        guides: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Descent {
        picked_order: Vec<String>,
    },
    /// One completed BFS level from the anchors.
    #[serde(rename_all = "camelCase")]
    Hop {
        hop: usize,
        added: Vec<String>,
        visited_so_far: usize,
    },
    #[serde(rename_all = "camelCase")]
    Traversal {
        visited_count: usize,
        hop_depth: usize,
    },
    Scored {
        order: Vec<StageScoredSection>,
        direct: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Delivered {
        tier_order: Vec<String>,
        tier_total: usize,
    },
    #[serde(rename_all = "camelCase")]
    Reorder {
        final_order: Vec<StageSectionChars>,
        final_total: usize,
        dropped: Vec<String>,
        dropped_total: usize,
        reordered: bool,
    },
    EvidenceSummary {
        sections: usize,
        chars: usize,
        dropped: usize,
        triples: usize,
    },
    Synthesis {
        phase: &'static str,
        chars: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        total_secs: f64,
    },
}

impl StageEvent {
    /// `(log tag, human line)` with byte parity against the legacy string
    /// callbacks (`kg-query [<tag>] <line>` in workspace logs).
    pub fn log_parts(&self) -> (&'static str, String) {
        match self {
            StageEvent::Depth { depth } => ("depth", format!("depth={depth}")),
            StageEvent::Bm25 { pool_size, .. } => ("bm25", format!("pool={pool_size}")),
            StageEvent::Gate { fired, forced, rule, s1, floor, ratio, n, .. } => {
                let _ = forced; // parity line predates the forced marker
                let floor_part = floor.map(|f| {
                    format!(" floor={f:.3} ratio={:.3}", ratio.unwrap_or(0.0))
                }).unwrap_or_default();
                let verdict = if *fired { "FIRE" } else { "pass" };
                ("gate", format!("{rule} s1={s1:.3}{floor_part} n={n} -> {verdict}"))
            }
            StageEvent::Concepts { used, model, candidates, picks, error, .. } => (
                "concepts",
                format!(
                    "used={used} candidates={candidates} picks={}{}{}",
                    picks.len(),
                    if model.is_empty() { String::new() } else { format!(" model={model}") },
                    error.as_deref().map(|e| format!(" error={e}")).unwrap_or_default(),
                ),
            ),
            StageEvent::Expansion { escalated, terms, error, .. } => (
                "expansion",
                format!(
                    "escalated={} terms={terms:?}{}",
                    escalated,
                    error.as_deref().map(|e| format!(" error={e}")).unwrap_or_default(),
                ),
            ),
            StageEvent::ExpansionFailed { message } => (
                "expansion",
                format!("escalation LLM call failed: {message}"),
            ),
            StageEvent::Elevation { leads, shelves } => (
                "elevation",
                format!("{leads} expanded entities lead seeds; {shelves} shelves queued"),
            ),
            StageEvent::Seeds { ids, anchors, lead_ids } => (
                "seeds",
                format!("entities={} anchors={} elevated={}", ids.len(), anchors.len(), lead_ids.len()),
            ),
            StageEvent::Votes { guides, .. } => ("voting", format!("guides={guides:?}")),
            StageEvent::Descent { picked_order } => (
                "descent",
                format!("picked={} entities, 0 LLM calls", picked_order.len()),
            ),
            StageEvent::Hop { hop, visited_so_far, .. } => (
                "traversal",
                format!("hop {hop}: reached total={visited_so_far}"),
            ),
            StageEvent::Traversal { visited_count, hop_depth } => (
                "traversal",
                format!("visited={visited_count} hops={hop_depth}"),
            ),
            StageEvent::Scored { direct, .. } => ("locator", format!("direct hits={}", direct.len())),
            StageEvent::Delivered { tier_total, .. } => ("delivery", format!("tier queue={tier_total}")),
            StageEvent::Reorder { final_total, dropped_total, reordered, .. } => (
                "reorder",
                format!("final={final_total} reordered={reordered} dropped={dropped_total}"),
            ),
            StageEvent::EvidenceSummary { sections, chars, dropped, triples } => (
                "evidence",
                format!("sections={sections} chars={chars} dropped={dropped} triples={triples}"),
            ),
            StageEvent::Synthesis { phase, chars } => match *phase {
                "start" => ("synthesis", "streaming writer".to_string()),
                _ => ("synthesis", format!("wrote {} chars", chars.unwrap_or(0))),
            },
            StageEvent::Done { total_secs } => ("done", format!("total={total_secs:.1}s")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Byte parity with the legacy string callbacks — the exact lines that
    /// `kg-query [<tag>] <line>` produced before the typed sink existed.
    #[test]
    fn log_parts_byte_parity_with_legacy_lines() {
        let (tag, line) = StageEvent::Depth { depth: "deep" }.log_parts();
        assert_eq!((tag, line.as_str()), ("depth", "depth=deep"));

        let (tag, line) = StageEvent::Bm25 {
            entity_hits: vec![],
            section_hits: 2,
            pool_size: 12,
        }
        .log_parts();
        assert_eq!((tag, line.as_str()), ("bm25", "pool=12"));

        // FIRE with a measured floor
        let (tag, line) = StageEvent::Gate {
            fired: true,
            forced: false,
            rule: "ratio<1.1".to_string(),
            s1: 4.0,
            floor: Some(5.0),
            ratio: Some(0.8),
            n: 1,
            concept_routing: true,
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("gate", "ratio<1.1 s1=4.000 floor=5.000 ratio=0.800 n=1 -> FIRE")
        );

        // pass without a floor (count rule only)
        let (tag, line) = StageEvent::Gate {
            fired: false,
            forced: false,
            rule: "n<3 (no floor)".to_string(),
            s1: 9.0,
            floor: None,
            ratio: None,
            n: 3,
            concept_routing: false,
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("gate", "n<3 (no floor) s1=9.000 n=3 -> pass")
        );

        let (tag, line) = StageEvent::Concepts {
            used: true,
            model: "jev-1.13.0".to_string(),
            candidates: 12,
            picks: vec![StageConceptPick {
                section: "S1".to_string(),
                name: "Cache hierarchy".to_string(),
                prob: 0.91,
            }],
            secs: 0.3,
            error: None,
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("concepts", "used=true candidates=12 picks=1 model=jev-1.13.0")
        );

        let (tag, line) = StageEvent::Concepts {
            used: false,
            model: String::new(),
            candidates: 0,
            picks: vec![],
            secs: 0.0,
            error: Some("Jev call failed: boom".to_string()),
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("concepts", "used=false candidates=0 picks=0 error=Jev call failed: boom")
        );

        let (tag, line) = StageEvent::Expansion {
            escalated: true,
            terms: vec!["alpha".to_string(), "beta".to_string()],
            entities: vec![],
            sections: vec![],
            error: None,
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("expansion", "escalated=true terms=[\"alpha\", \"beta\"]")
        );

        let (tag, line) = StageEvent::Expansion {
            escalated: true,
            terms: vec![],
            entities: vec![],
            sections: vec![],
            error: Some("LLM call failed: boom".to_string()),
        }
        .log_parts();
        assert_eq!(
            (tag, line.as_str()),
            ("expansion", "escalated=true terms=[] error=LLM call failed: boom")
        );

        let (tag, line) =
            StageEvent::ExpansionFailed { message: "boom".to_string() }.log_parts();
        assert_eq!((tag, line.as_str()), ("expansion", "escalation LLM call failed: boom"));

        let (tag, line) = StageEvent::Elevation { leads: 3, shelves: 2 }.log_parts();
        assert_eq!((tag, line.as_str()), ("elevation", "3 expanded entities lead seeds; 2 shelves queued"));

        let (tag, line) = StageEvent::Seeds {
            ids: vec!["a".into()],
            anchors: vec!["b".into()],
            lead_ids: vec!["c".into()],
        }
        .log_parts();
        assert_eq!((tag, line.as_str()), ("seeds", "entities=1 anchors=1 elevated=1"));

        let (tag, line) = StageEvent::Votes { votes: vec![], guides: vec!["G1".into()] }.log_parts();
        assert_eq!((tag, line.as_str()), ("voting", "guides=[\"G1\"]"));

        let (tag, line) =
            StageEvent::Descent { picked_order: vec!["e".into(); 4] }.log_parts();
        assert_eq!((tag, line.as_str()), ("descent", "picked=4 entities, 0 LLM calls"));

        let (tag, line) =
            StageEvent::Hop { hop: 2, added: vec![], visited_so_far: 47 }.log_parts();
        assert_eq!((tag, line.as_str()), ("traversal", "hop 2: reached total=47"));

        let (tag, line) = StageEvent::Traversal { visited_count: 12, hop_depth: 2 }.log_parts();
        assert_eq!((tag, line.as_str()), ("traversal", "visited=12 hops=2"));

        let (tag, line) = StageEvent::Scored { order: vec![], direct: vec!["s".into(); 3] }.log_parts();
        assert_eq!((tag, line.as_str()), ("locator", "direct hits=3"));

        let (tag, line) = StageEvent::Delivered { tier_order: vec![], tier_total: 5 }.log_parts();
        assert_eq!((tag, line.as_str()), ("delivery", "tier queue=5"));

        let (tag, line) = StageEvent::Reorder {
            final_order: vec![],
            final_total: 5,
            dropped: vec![],
            dropped_total: 2,
            reordered: true,
        }
        .log_parts();
        assert_eq!((tag, line.as_str()), ("reorder", "final=5 reordered=true dropped=2"));

        let (tag, line) =
            StageEvent::EvidenceSummary { sections: 3, chars: 1200, dropped: 1, triples: 40 }
                .log_parts();
        assert_eq!((tag, line.as_str()), ("evidence", "sections=3 chars=1200 dropped=1 triples=40"));

        let (tag, line) = StageEvent::Synthesis { phase: "start", chars: None }.log_parts();
        assert_eq!((tag, line.as_str()), ("synthesis", "streaming writer"));

        let (tag, line) = StageEvent::Synthesis { phase: "end", chars: Some(123) }.log_parts();
        assert_eq!((tag, line.as_str()), ("synthesis", "wrote 123 chars"));

        let (tag, line) = StageEvent::Done { total_secs: 9.42 }.log_parts();
        assert_eq!((tag, line.as_str()), ("done", "total=9.4s"));
    }

    /// Contract freeze: every event serializes as `{step, data}` with
    /// camelCase data keys — the shape the shell forwards as stage frames.
    #[test]
    fn serde_shape_is_internally_tagged_step_data() {
        let hop = StageEvent::hop(2, &["CONCEPT_a".into(), "CONCEPT_b".into()], 47);
        let value = serde_json::to_value(hop).unwrap();
        assert_eq!(value["step"], "hop");
        assert_eq!(value["data"]["hop"], 2);
        assert_eq!(value["data"]["added"], json!(["CONCEPT_a", "CONCEPT_b"]));
        assert_eq!(value["data"]["visitedSoFar"], 47);

        let gate = StageEvent::Gate {
            fired: true,
            forced: true,
            rule: "forced (retry-on-refusal)".into(),
            s1: 1.5,
            floor: Some(2.0),
            ratio: Some(0.75),
            n: 2,
            concept_routing: true,
        };
        let value = serde_json::to_value(gate).unwrap();
        assert_eq!(value["step"], "gate");
        assert_eq!(value["data"]["forced"], true);
        assert_eq!(value["data"]["floor"], 2.0);
        assert!(value["data"].get("s1").is_some());

        let concepts = StageEvent::Concepts {
            used: true,
            model: "jev-1.13.0".into(),
            candidates: 3,
            picks: vec![StageConceptPick {
                section: "S1".into(),
                name: "Cache".into(),
                prob: 0.5,
            }],
            secs: 0.1,
            error: None,
        };
        let value = serde_json::to_value(concepts).unwrap();
        assert_eq!(value["step"], "concepts");
        assert_eq!(value["data"]["used"], true);
        assert_eq!(value["data"]["picks"][0]["section"], "S1");
        assert_eq!(value["data"]["picks"][0]["prob"], 0.5);
    }

    #[test]
    fn constructors_cap_payload_lists() {
        let big: Vec<String> = (0..200).map(|i| format!("id{i}")).collect();
        let seeds = StageEvent::seeds(&big, &big, &big);
        match seeds {
            StageEvent::Seeds { ids, anchors, lead_ids } => {
                assert_eq!(ids.len(), STAGE_CAP);
                assert_eq!(anchors.len(), STAGE_CAP);
                assert_eq!(lead_ids.len(), STAGE_CAP);
            }
            _ => panic!("wrong variant"),
        }

        let votes: Vec<(String, f64)> = (0..200).map(|i| (format!("t{i}"), i as f64)).collect();
        match StageEvent::votes(&votes, &big) {
            StageEvent::Votes { votes, guides } => {
                assert_eq!(votes.len(), STAGE_CAP);
                assert_eq!(guides.len(), STAGE_CAP);
                assert_eq!(votes[0].topic, "t0");
                assert_eq!(votes[0].score, 0.0);
            }
            _ => panic!("wrong variant"),
        }

        match StageEvent::descent(&big) {
            StageEvent::Descent { picked_order } => assert_eq!(picked_order.len(), STAGE_CAP),
            _ => panic!("wrong variant"),
        }

        match StageEvent::hop(3, &big, 999) {
            StageEvent::Hop { hop, added, visited_so_far } => {
                assert_eq!(hop, 3);
                assert_eq!(added.len(), STAGE_CAP);
                assert_eq!(visited_so_far, 999);
            }
            _ => panic!("wrong variant"),
        }

        let secs: Vec<(String, f64)> = (0..200).map(|i| (format!("s{i}"), 1.0)).collect();
        match StageEvent::scored(&secs, &big) {
            StageEvent::Scored { order, direct } => {
                assert_eq!(order.len(), SCORED_CAP); // scored preview is capped tighter
                assert_eq!(direct.len(), STAGE_CAP);
            }
            _ => panic!("wrong variant"),
        }

        match StageEvent::delivered(&big) {
            StageEvent::Delivered { tier_order, tier_total } => {
                assert_eq!(tier_order.len(), STAGE_CAP);
                assert_eq!(tier_total, 200); // totals report the true count
            }
            _ => panic!("wrong variant"),
        }

        let blocks: Vec<(String, usize)> = (0..200).map(|i| (format!("s{i}"), i)).collect();
        match StageEvent::reorder(&blocks, &big, true) {
            StageEvent::Reorder { final_order, final_total, dropped, dropped_total, reordered } => {
                assert_eq!(final_order.len(), STAGE_CAP);
                assert_eq!(final_total, 200);
                assert_eq!(dropped.len(), STAGE_CAP);
                assert_eq!(dropped_total, 200);
                assert!(reordered);
            }
            _ => panic!("wrong variant"),
        }
    }
}
