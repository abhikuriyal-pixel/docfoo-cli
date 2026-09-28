//! Evidence assembly: the rare-token locator (DD-21) and full-text delivery
//! with the DD-23/24/25 unified relevance ordering — ported from
//! kg_demo `pipeline.py` (`_deliver_full_routed` + locator block).
//!
//! The locator's scoring lives here as shared helpers: the in-memory reader
//! computes term occurrences by scanning every section, while `KgStore` reads
//! them from `locator_df` and only regex-scores the sections
//! `section_locator_terms` returned. Both rank with [`locator_rank`], so the
//! ordering cannot drift.

use super::{EvidenceBlock, GuidePick};
use crate::expand::RelationIndex;
use crate::reader::GraphReader;
use crate::store::StoreError;
use crate::tunables::QueryTunables;
use std::cmp::Ordering as Rank;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Anchor width used by the DD-25 relation-expansion boost
/// (expansion.py expansion_set default).
pub(crate) const EXPANSION_ANCHOR_K: usize = 8;

// ---------------------------------------------------------------------------
// Rare-token locator (DD-21)
// ---------------------------------------------------------------------------

/// Raw ASCII word runs, lowercased — the locator's tokenization contract
/// (`[A-Za-z0-9]+`), shared by the query side and the persisted raw corpus.
pub(crate) fn locator_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c.to_ascii_lowercase());
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Candidate patterns for the query: digit-bearing codes and words of 3-20
/// characters, each compiled to a word-boundary regex.
pub(crate) fn locator_patterns(query: &str) -> Vec<(String, regex::Regex)> {
    let raw: BTreeSet<String> = locator_tokens(query).into_iter().collect();
    raw.into_iter()
        .filter(|token| {
            (token.len() >= 5 && token.chars().any(|c| c.is_ascii_digit()))
                || (3..=20).contains(&token.len())
        })
        .filter_map(|token| {
            let pattern = regex::Regex::new(&format!(r"\b{}\b", regex::escape(&token))).ok()?;
            Some((token, pattern))
        })
        .collect()
}

/// A candidate stays active when it is a digit-bearing code or appears in at
/// most 15 sections — the same rarity rule as the pre-SQLite scan.
pub(crate) fn locator_active(
    patterns: &[(String, regex::Regex)],
    occurrences: &HashMap<String, usize>,
) -> Vec<(String, regex::Regex)> {
    patterns
        .iter()
        .filter(|(token, _)| {
            token.chars().any(|c| c.is_ascii_digit())
                || occurrences.get(token.as_str()).copied().unwrap_or(0) <= 15
        })
        .cloned()
        .collect()
}

/// Score candidate sections exactly like the original full scan and return
/// the top `max_sections` (score desc, title asc). `sections` texts must
/// already be lowercased.
pub(crate) fn locator_rank(
    total_secs: usize,
    active: &[(String, regex::Regex)],
    occurrences: &HashMap<String, usize>,
    sections: &[(String, String)],
    max_sections: usize,
) -> Vec<String> {
    let mut scored: Vec<(f64, String)> = vec![];
    for (title, low) in sections {
        let title_low = title.to_lowercase();
        let mut score = 0.0;
        for (token, pattern) in active {
            let count = pattern.find_iter(low).count();
            if count > 0 {
                let idf = (1.0
                    + total_secs as f64
                        / occurrences.get(token.as_str()).copied().unwrap_or(0).max(1) as f64)
                    .ln();
                score += count as f64 * idf;
                if title_low.contains(token.as_str()) {
                    score += 3.0 * idf; // token appears in the title
                }
            }
        }
        if score > 0.0 {
            scored.push((score, title.clone()));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Rank::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.into_iter().take(max_sections).map(|(_, s)| s).collect()
}

// ---------------------------------------------------------------------------
// Evidence delivery (DD-23/24/25 unified ordering)
// ---------------------------------------------------------------------------

pub struct Delivered {
    /// (section title, chars, formatted evidence block)
    pub blocks: Vec<(String, usize, String)>,
    pub dropped: Vec<EvidenceBlock>,
    /// Queue order as handed to the budget fill loop (post unified-relevance
    /// sort when it ran) — the visualizer's `delivered` frame tier order.
    pub tier_order: Vec<String>,
}

/// Push a title onto the delivery queue when the section exists and is new.
fn push_section(
    kg: &dyn GraphReader,
    queue: &mut Vec<String>,
    queued: &mut HashSet<String>,
    title: String,
) -> Result<(), StoreError> {
    if kg.section(&title)?.is_some() && queued.insert(title.clone()) {
        queue.push(title);
    }
    Ok(())
}

/// Deliver the FULL text of routed sections within the char budget.
///
/// Queue order: expanded-vocabulary shelf hits (priority), guide-pick
/// sections, picked entities' primary sections, rare-token locator direct
/// hits, then vote-ranked fallback; deduped on first occurrence. When
/// `evidence_relevance_sort` is on and a query is given, the assembled
/// queue is stable-sorted by ONE fused score — normalized section-BM25
/// probe + DD-25 relation-expansion boost — with two hard pins ahead of the
/// sort: escalation bridge shelves ship first, locator hits immediately
/// after (no lexical+graph score can be trusted for their recall).
#[allow(clippy::too_many_arguments)]
pub fn deliver_full_routed(
    kg: &dyn GraphReader,
    t: &QueryTunables,
    guides: &[String],
    guide_picks: &[GuidePick],
    picks_by_topic: &HashMap<String, Vec<String>>,
    sec_scores: &[(String, f64)],
    direct_sections: &[String],
    priority_sections: &[String],
    query: &str,
    ridx: Option<&RelationIndex>,
) -> Result<Delivered, StoreError> {
    let mut queue: Vec<String> = vec![];
    let mut queued: HashSet<String> = HashSet::new();
    for title in priority_sections {
        push_section(kg, &mut queue, &mut queued, title.clone())?;
    }
    for topic in guides {
        for pick in guide_picks.iter().filter(|g| g.topic == *topic) {
            for fragment in &pick.sections {
                if let Some(title) = kg.resolve_section_fragment(fragment)? {
                    push_section(kg, &mut queue, &mut queued, title)?;
                }
            }
        }
    }
    for topic in guides {
        for eid in picks_by_topic.get(topic).map(|p| p.as_slice()).unwrap_or(&[]) {
            if let Some(first) = kg.entity(eid)?.and_then(|ent| ent.sections.first().cloned()) {
                push_section(kg, &mut queue, &mut queued, first)?;
            }
        }
    }
    for title in direct_sections {
        push_section(kg, &mut queue, &mut queued, title.clone())?;
    }
    for (title, _) in sec_scores {
        push_section(kg, &mut queue, &mut queued, title.clone())?;
    }

    // DD-23/24/25 unified ordering: ONE fused score for every mode.
    if t.evidence_relevance_sort && !query.is_empty() {
        let width = t.relevance_search_k.max(queue.len());
        let rel: HashMap<String, f64> =
            kg.bm25_section_search(query, width)?.into_iter().collect();
        let boosts = if t.enable_query_expansion {
            match ridx {
                Some(index) => index.section_boosts(kg, &queue, 2.0, 1.0)?,
                None => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        let peak_rel = queue.iter()
            .map(|s| rel.get(s).copied().unwrap_or(0.0))
            .fold(f64::NEG_INFINITY, f64::max);
        let fused = |s: &str| -> f64 {
            let lex = if peak_rel > 0.0 {
                rel.get(s).copied().unwrap_or(0.0) / peak_rel
            } else {
                0.0
            };
            -(lex + t.expansion_weight * boosts.get(s).copied().unwrap_or(0.0))
        };
        let mut pinned: Vec<String> = priority_sections.iter()
            .filter(|s| queued.contains(*s)).cloned().collect();
        for s in direct_sections {
            if queued.contains(s) && !pinned.contains(s) {
                pinned.push(s.clone());
            }
        }
        let pinned_set: HashSet<String> = pinned.iter().cloned().collect();
        let mut rest: Vec<(String, f64)> = queue.iter()
            .filter(|s| !pinned_set.contains(*s))
            .map(|s| (s.clone(), fused(s)))
            .collect();
        rest.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Rank::Equal));
        pinned.extend(rest.into_iter().map(|(s, _)| s));
        queue = pinned;
    }

    let mut out = Delivered { blocks: vec![], dropped: vec![], tier_order: queue.clone() };
    let mut remaining = t.evidence_char_budget;
    for s in &queue {
        let Some(info) = kg.section(s)? else { continue };
        if info.text.is_empty() {
            continue;
        }
        let doc_dir = info.source_doc.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let text = qualify_asset_paths(info.text.as_str(), doc_dir);
        let block = format_block(s, &info, &text);
        if t.evidence_budget_unlimited {
            // DD-24: big-context serving ships every queued section whole;
            // the char budget and its drop list are bypassed entirely.
            out.blocks.push((s.clone(), block.len(), block));
            continue;
        }
        if block.len() <= remaining {
            remaining -= block.len();
            out.blocks.push((s.clone(), block.len(), block));
        } else {
            out.dropped.push(EvidenceBlock {
                section: s.clone(),
                chars: block.len(),
                doc: info.source_doc.clone(),
                start_line: (info.start_line > 0).then_some(info.start_line),
                end_line: (info.end_line > 0).then_some(info.end_line),
            });
        }
    }
    Ok(out)
}

/// Prepend the document folder to relative markdown image/link targets so
/// evidence paths are document-qualified (chat resolves them):
/// `](assets/fig.png)` -> `](multi_column_1/assets/fig.png)`.
/// Absolute URLs (`://`), root-absolute (`/...`), note images
/// (`notes-images/...`) and already-qualified paths are left untouched.
fn qualify_asset_paths(text: &str, doc_dir: &str) -> String {
    if doc_dir.is_empty() {
        return text.to_string();
    }
    let re = regex::Regex::new(r"\]\(([^)\s]+)\)").expect("static regex");
    re.replace_all(text, |caps: &regex::Captures| {
        let target = &caps[1];
        let qualified = target.starts_with('/')
            || target.contains("://")
            || target.starts_with("notes-images/")
            || target.starts_with(&format!("{doc_dir}/"));
        if qualified {
            caps[0].to_string()
        } else {
            format!("]({doc_dir}/{target})")
        }
    }).into_owned()
}

/// One evidence block: header names the section title and — when the graph
/// carries line provenance — the source document + inclusive line span.
fn format_block(title: &str, info: &crate::graph::SectionInfo, text: &str) -> String {
    let header = if info.start_line > 0 && !info.source_doc.is_empty() {
        format!("[{title} — {} lines {}-{}]", info.source_doc, info.start_line, info.end_line)
    } else {
        format!("[{title}]") // legacy graph without line provenance
    };
    format!("{header}\n{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{KnowledgeGraph, SectionInfo};
    use crate::test_support::StoreFixture;

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
            content_hash: String::new(),
            ..Default::default()
        });
        kg.sections.insert("S2".into(), SectionInfo {
            topic: Some("[CO] Chapter 4".into()),
            entity_ids: vec!["CONCEPT_ram".into()],
            text: "Random access memory holds running programs in main storage cells.".repeat(6),
            source_doc: "d.md".into(),
            content_hash: String::new(),
            ..Default::default()
        });
        kg.noise_floor = Some(5.0);
        kg
    }

    fn fixture() -> StoreFixture {
        StoreFixture::new("deliver", &fixture_kg())
    }

    #[test]
    fn locator_prefers_rare_tokens_and_titles() {
        let mut kg = fixture_kg();
        kg.sections.insert("S3".into(), SectionInfo {
            topic: None,
            entity_ids: vec![],
            text: "The Z80X12 controller coordinates refresh cycles oddly.".repeat(4),
            source_doc: "d.md".into(),
            content_hash: String::new(),
            ..Default::default()
        });
        let fx = StoreFixture::new("deliver-locator", &kg);
        let hits = fx.store.locate_direct_sections("Z80X12 controller", 6).unwrap();
        assert_eq!(hits.first().map(String::as_str), Some("S3"));
    }

    #[test]
    fn unlimited_delivery_ships_everything_in_tier_order() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default();
        let picks_by_topic = HashMap::from([
            ("[CO] Chapter 3".to_string(), vec!["CONCEPT_cache".to_string()]),
        ]);
        let delivered = deliver_full_routed(
            kg, &t,
            &["[CO] Chapter 3".to_string()],
            &[GuidePick {
                topic: "[CO] Chapter 3".into(),
                sections: vec!["S2".into()], // guide-pick shelf resolves exactly
                entities: vec![],
            }],
            &picks_by_topic,
            &[("S1".into(), 1.0)],
            &[],
            &[],
            "",
            None,
        ).unwrap();
        let titles: Vec<&str> =
            delivered.blocks.iter().map(|(s, _, _)| s.as_str()).collect();
        // guide-pick fragment S2 first, then pick primary S1, then scores S1(dup skipped)
        assert_eq!(titles, vec!["S2", "S1"]);
        assert!(delivered.blocks[0].2.contains("[S2]"));
        assert!(delivered.blocks[0].2.contains("Random access memory"));
        assert!(delivered.blocks[1].2.contains("[S1]"));
        assert!(delivered.blocks[1].2.contains("Cache stores instructions"));
        assert!(delivered.dropped.is_empty());
        // tier_order is the pre-budget queue handed to the fill loop
        assert_eq!(delivered.tier_order, vec!["S2".to_string(), "S1".to_string()]);

        // budget mode: tiny budget drops oversized sections
        let mut t2 = t.clone();
        t2.evidence_budget_unlimited = false;
        t2.evidence_char_budget = 10;
        let d2 = deliver_full_routed(
            kg, &t2, &[], &[], &HashMap::new(),
            &[("S1".into(), 1.0), ("S2".into(), 0.5)],
            &[], &[], "", None).unwrap();
        assert!(d2.blocks.is_empty());
        assert_eq!(d2.dropped.len(), 2);
        assert_eq!(d2.tier_order, vec!["S1".to_string(), "S2".to_string()]);
    }

    #[test]
    fn qualify_rewrites_relative_and_keeps_qualified() {
        assert_eq!(qualify_asset_paths("![a](assets/f.png)", "multi_column_1"),
                   "![a](multi_column_1/assets/f.png)");
        assert_eq!(qualify_asset_paths("![a](multi_column_1/assets/f.png)", "multi_column_1"),
                   "![a](multi_column_1/assets/f.png)");
        assert_eq!(qualify_asset_paths("![a](https://x/y.png)", "multi_column_1"),
                   "![a](https://x/y.png)");
        assert_eq!(qualify_asset_paths("![a](/root.png)", "multi_column_1"), "![a](/root.png)");
        assert_eq!(qualify_asset_paths("![a](notes-images/d/i.jpg)", "multi_column_1"),
                   "![a](notes-images/d/i.jpg)");
        assert_eq!(qualify_asset_paths("![a](assets/f.png)", ""), "![a](assets/f.png)");
    }

    #[test]
    fn format_block_includes_provenance_when_available() {
        let info = SectionInfo {
            source_doc: "multi_column_1/content.md".into(),
            start_line: 42,
            end_line: 108,
            ..Default::default()
        };
        assert_eq!(
            format_block("T", &info, "the text"),
            "[T — multi_column_1/content.md lines 42-108]\nthe text"
        );

        let legacy = SectionInfo { start_line: 0, ..Default::default() };
        assert_eq!(format_block("T", &legacy, "the text"), "[T]\nthe text");
    }

    /// Relevance sort reshuffles the queue before delivery — tier_order must
    /// report the post-sort order so the visualizer's `delivered` frame
    /// matches the final `reorder` frame.
    #[test]
    fn tier_order_reflects_unified_relevance_sort() {
        let fx = fixture();
        let kg = &fx.store;
        let t = QueryTunables::default(); // evidence_relevance_sort is on by default
        let delivered = deliver_full_routed(
            kg, &t,
            &[], &[], &HashMap::new(),
            &[("S1".into(), 1.0), ("S2".into(), 0.5)],
            &[], &[], "cache", None,
        ).unwrap();
        // S1 is the only section whose text matches "cache" — it leads the
        // fused-score order even though S2 was queued first.
        assert_eq!(delivered.tier_order.first().map(String::as_str), Some("S1"));
    }
}
