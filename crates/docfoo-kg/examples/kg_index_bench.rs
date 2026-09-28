//! Dev-only: time the native build (indexing) path with an instant mock LLM.
//!
//! Usage:
//!   kg_index_bench build <graph.sqlite> <docs> <sections_per_doc> [entities_per_section=8]
//!   kg_index_bench components <graph.sqlite>
//!
//! `build` runs a full build, then changes one document and rebuilds to
//! measure the incremental path. `components` measures the incremental
//! finalize pieces (sync scan, topic rebuild, one section transaction,
//! glossary) against an existing large store.

use docfoo_kg::build::{self, IndexOptions, InputDoc};
use docfoo_kg::llm::{ChatClient, LlmError};
use docfoo_kg::store::{KgStore, OpenMode, SectionInput};
use docfoo_kg::text::Rng;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Answers extraction and topic-grouping prompts instantly, minting unique
/// entity names per call so the graph really grows.
struct MockLlm {
    counter: AtomicUsize,
    per_section: usize,
}

impl ChatClient for MockLlm {
    fn chat(
        &self,
        messages: &[Value],
        _max_tokens: u32,
        _response_format: Option<Value>,
        _cancel: &AtomicBool,
    ) -> Result<String, LlmError> {
        let content = messages
            .iter()
            .find_map(|m| m["content"].as_str())
            .unwrap_or("");
        if content.contains("Group these encyclopedia section titles") {
            let sections: Vec<String> = content
                .lines()
                .filter_map(|line| line.strip_prefix("- ").map(str::to_string))
                .collect();
            return Ok(serde_json::json!({
                "topics": [{"name": "Mock Topic", "sections": sections}]
            })
            .to_string());
        }
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut entities = Vec::new();
        let mut relations = Vec::new();
        for i in 0..self.per_section {
            entities.push(serde_json::json!({
                "name": format!("Widget {n} {i}"),
                "type": "CONCEPT",
                "description": format!(
                    "synthetic widget {n} unit {i} about cache memory topic {}",
                    n % 50
                ),
            }));
            if i > 0 {
                relations.push(serde_json::json!({
                    "source": format!("Widget {n} {}", i - 1),
                    "relation": "USES",
                    "target": format!("Widget {n} {i}"),
                }));
            }
        }
        Ok(serde_json::json!({
            "concept": {
                "name": format!("Widget batch {n}"),
                "summary": "synthetic batch of widgets",
                "terms": ["widget", "cache"],
            },
            "entities": entities,
            "relations": relations,
        })
        .to_string())
    }

    fn chat_stream(
        &self,
        messages: &[Value],
        max_tokens: u32,
        _on_delta: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<String, LlmError> {
        self.chat(messages, max_tokens, None, cancel)
    }
}

fn corpus(docs: usize, per_doc: usize) -> Vec<InputDoc> {
    (0..docs)
        .map(|d| {
            let mut markdown = String::new();
            for s in 0..per_doc {
                markdown.push_str(&format!(
                    "## {s}. Section {d}-{s}\n\n\
                     This section discusses cache memory topic {s} and actuator calibration {d}. \
                     The controller coordinates refresh cycles across the register file. \
                     Entropy coding keeps the channel busy while the pipeline drains.\n\n"
                ));
            }
            InputDoc {
                name: format!("doc{d:04}/content.md"),
                tag: format!("D{d:04}"),
                markdown,
                source_hash: format!("hash-{d}"),
            }
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    let Some(path) = args.get(2) else {
        eprintln!("usage: kg_index_bench build|components <graph.sqlite> ...");
        std::process::exit(2);
    };
    let path = std::path::Path::new(path);

    match mode {
        "build" => {
            let docs_n: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(20);
            let per_doc: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(100);
            let per_section: usize = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(8);
            let mut docs = corpus(docs_n, per_doc);
            let llm = Arc::new(MockLlm {
                counter: AtomicUsize::new(0),
                per_section,
            });
            let run = |docs: &[InputDoc], fresh: bool| {
                let opts = IndexOptions {
                    llm: Arc::clone(&llm) as Arc<dyn ChatClient>,
                    workers: 4,
                    fresh,
                    graph_path: path.to_path_buf(),
                    settings: Default::default(),
                };
                let cancel = AtomicBool::new(false);
                let start = Instant::now();
                let stats = build::run(docs, &opts, |_| {}, &cancel).expect("build");
                (
                    start.elapsed().as_secs_f64(),
                    stats.extracted_this_run,
                    stats.skipped_resume,
                    stats.entities,
                    stats.sections,
                )
            };

            let (full, extracted, _skipped, entities, sections) = run(&docs, true);
            println!(
                "full build: {full:.1}s for {sections} sections / {entities} entities \
                 ({:.1} ms/section, {:.0} sections/s, {extracted} extracted)",
                full * 1000.0 / sections.max(1) as f64,
                sections as f64 / full.max(0.001)
            );

            // One changed document (a new section) drives the incremental path.
            docs[0].markdown.push_str(
                "\n## 999. Fresh Section\n\n\
                 A brand new paragraph about entropy coding and channel calibration. \
                 The controller coordinates refresh cycles across the register file while the pipeline drains. \
                 Synthetic widgets batch their work to keep the cache memory busy under load.\n",
            );
            docs[0].source_hash.push_str("-v2");
            let (inc, extracted, skipped, entities, sections) = run(&docs, false);
            println!(
                "incremental build: {inc:.2}s ({extracted} extracted, {skipped} skipped, \
                 {entities} entities, {sections} sections)"
            );
        }
        "components" => {
            let store = KgStore::open(path, OpenMode::ReadWrite).expect("open store");
            let was_ready = store.derived_ready().unwrap_or(false);
            let start = Instant::now();
            let docs: BTreeSet<String> = store.section_docs().unwrap();
            let rows = store.sections_in_scope(&docs).unwrap().len();
            println!(
                "sync scan: {:.0} ms ({rows} sections, {} docs)",
                start.elapsed().as_secs_f64() * 1000.0,
                docs.len()
            );

            let start = Instant::now();
            let topics = store.section_topics().unwrap();
            let map: HashMap<String, String> = topics
                .iter()
                .map(|(title, topic)| {
                    (
                        title.clone(),
                        topic.clone().unwrap_or_else(|| "Other".to_string()),
                    )
                })
                .collect();
            println!(
                "topics read: {:.0} ms ({} sections)",
                start.elapsed().as_secs_f64() * 1000.0,
                topics.len()
            );
            let start = Instant::now();
            store.apply_topics(&map).unwrap();
            println!(
                "topics apply: {:.0} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );

            let start = Instant::now();
            let row = store
                .upsert_section(&SectionInput {
                    title: "[BENCH] Fresh Section",
                    text: "A fresh bench paragraph about entropy coding and cache memory.",
                    source_doc: "bench/content.md",
                    start_line: 1,
                    end_line: 2,
                    content_hash: "bench-hash",
                    retry_pending: false,
                    topic: None,
                })
                .unwrap();
            store.set_section_entities(row, &[]).unwrap();
            store.refresh_section_tokens(row).unwrap();
            store.upsert_concept(row, None).unwrap();
            println!(
                "one section write: {:.1} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );

            let text: String = store
                .connection()
                .query_row("SELECT text FROM sections ORDER BY row LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let start = Instant::now();
            let entries = store.glossary(&text, 60).unwrap().len();
            println!(
                "glossary: {:.1} ms ({entries} entries)",
                start.elapsed().as_secs_f64() * 1000.0
            );

            // Noise floor at the real (300-sample) size.
            let df = store.entity_df().unwrap();
            let mut common: Vec<String> = df
                .iter()
                .filter(|(_, &c)| c >= 20)
                .map(|(t, _)| t.clone())
                .collect();
            common.sort();
            if common.len() < 4 {
                let mut by_freq: Vec<(String, usize)> = df.into_iter().collect();
                by_freq.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                common = by_freq.into_iter().take(10).map(|(t, _)| t).collect();
            }
            let mut rng = Rng::new(42);
            let mut tops: Vec<f64> = vec![];
            let start = Instant::now();
            for _ in 0..300 {
                let picks = rng.sample_indices(4.min(common.len()), common.len());
                let q: Vec<String> = picks.iter().map(|&i| common[i].clone()).collect();
                if let Some((_, best)) = store.bm25_entity_search(&q.join(" "), 1).unwrap().first() {
                    if *best > 0.0 {
                        tops.push(*best);
                    }
                }
            }
            println!(
                "noise floor (300 samples): {:.1}s ({} positive)",
                start.elapsed().as_secs_f64(),
                tops.len()
            );

            // Leave the store as found (drop the probe section, restore
            // the query gate).
            store
                .remove_sections(&["[BENCH] Fresh Section".to_string()])
                .unwrap();
            if was_ready {
                store.set_meta("derived_ready", "1").unwrap();
            }
        }
        _ => {
            eprintln!("usage: kg_index_bench build|components <graph.sqlite> ...");
            std::process::exit(2);
        }
    }
}
