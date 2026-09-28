//! Dev-only: generate a deterministic, sections-rich knowledge graph and write
//! it to a SQLite store.
//!
//! The synthetic `demo-1m` fixture is entity-heavy with only 96 sections, so
//! it cannot exercise the section-BM25, locator or concept-routing stages. A
//! real 1M-entity graph built from documents would have ~100-350k sections
//! (the extraction cap is 12 entities/section), which this generator models.
//!
//! Usage: kg_gen <graph.sqlite> <sections> [entities_per_section=4]

use docfoo_kg::graph::{Concept, Entity, KnowledgeGraph, Relation, SectionInfo};
use docfoo_kg::store::KgStore;
use std::path::Path;
use std::time::Instant;

const WORDS: &[&str] = &[
    "cache", "memory", "pipeline", "processor", "voltage", "current", "signal", "protocol",
    "network", "packet", "routing", "switch", "storage", "index", "query", "search", "ranking",
    "token", "vector", "matrix", "kernel", "gradient", "model", "training", "inference",
    "calibration", "sensor", "actuator", "controller", "feedback", "loop", "gain", "phase",
    "frequency", "filter", "amplifier", "antenna", "modulation", "channel", "coding", "error",
    "correction", "compression", "entropy", "latency", "throughput", "bandwidth", "topology",
    "cluster", "shard", "replica", "consensus", "transaction", "journal", "checkpoint",
    "segment", "page", "buffer", "thread", "scheduler", "interrupt", "register", "instruction",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: kg_gen <graph.sqlite> <sections> [entities_per_section=4]");
        std::process::exit(2);
    }
    let target = Path::new(&args[1]);
    let sections: usize = args[2].parse().unwrap_or_else(|_| {
        eprintln!("kg_gen: <sections> must be an integer");
        std::process::exit(2);
    });
    let per_section: usize = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);

    let started = Instant::now();
    let mut kg = KnowledgeGraph::default();
    let mut rng: u64 = 0x243F_6A88_85A3_08D3;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let pick = |value: u64| WORDS[(value as usize) % WORDS.len()];

    for s in 0..sections {
        let topic_index = s % 50;
        let title = format!("[GEN] Section {s:06} (part 1/1)");
        let concept_word = pick(next());
        let mut ids: Vec<String> = Vec::with_capacity(per_section);
        let mut text = String::new();
        for e in 0..per_section {
            let n = s * per_section + e;
            let id = format!("CONCEPT_gen_{n:07}");
            let first = pick(next());
            let second = pick(next());
            // The `n` suffix keeps canonical slugs unique in the store.
            let name = format!("{first} {second} {n}");
            let desc = format!(
                "generated {first} {second} concept for topic {topic_index} discussing {third} and {fourth}",
                third = pick(next()),
                fourth = pick(next())
            );
            kg.entities.insert(
                id.clone(),
                Entity {
                    id: id.clone(),
                    name,
                    etype: "CONCEPT".to_string(),
                    desc,
                    sections: vec![title.clone()],
                    source_doc: Vec::new(),
                },
            );
            ids.push(id);
            text.push_str(&format!(
                "This section discusses {first} and {second} in the context of topic {topic_index}. "
            ));
        }
        for pair in ids.windows(2) {
            kg.relations.push(Relation {
                source: pair[0].clone(),
                target: pair[1].clone(),
                rel: "USES".to_string(),
                section: title.clone(),
                source_doc: None,
            });
        }
        // A rare digit-bearing code every 1000 sections for the locator tier.
        if s % 1000 == 0 {
            text.push_str(&format!("Code Z{s:05}X triggers calibration. "));
        }
        kg.sections.insert(
            title.clone(),
            SectionInfo {
                topic: Some(format!("[GEN] Topic {topic_index:02}")),
                entity_ids: ids,
                concept: Some(Concept {
                    name: format!("{concept_word} concept"),
                    summary: format!("how {concept_word} behaves under load"),
                    terms: vec![
                        pick(next()).to_string(),
                        pick(next()).to_string(),
                    ],
                }),
                text,
                source_doc: format!("doc{:03}/content.md", s % 1000),
                start_line: 1,
                end_line: 20,
                content_hash: format!("hash-{s}"),
                retry_pending: false,
            },
        );
    }

    KgStore::write_full(&kg, target).unwrap_or_else(|e| {
        eprintln!("kg_gen: could not write {}: {e}", target.display());
        std::process::exit(1);
    });
    println!(
        "wrote {} ({} entities, {} sections, {} relations) in {:.1}s",
        target.display(),
        kg.entities.len(),
        kg.sections.len(),
        kg.relations.len(),
        started.elapsed().as_secs_f64()
    );
}
