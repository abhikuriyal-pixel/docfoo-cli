//! Dev-only: build a large render-benchmark graph directly as SQLite rows.
//!
//! The extraction-era `kg_gen` hydrates a whole `KnowledgeGraph` and runs the
//! full `write_full` tokenizer/FTS pipeline, which is the right way to make a
//! *query* fixture but is memory-heavy and slow at 1M entities. This example
//! writes the base rows the visualizer reads (entities, relations, sections
//! and the two provenance joins) in one transaction and then packs the scene
//! buffer with the same `store::scene::build_scene` the app command uses.
//!
//! The result is a valid-schema store with `derived_ready=1`; FTS corpora are
//! intentionally empty (triggers index empty tokens), so it is a *render*
//! fixture, not a query fixture. `scene.bin` is written next to the database
//! for `tests/perf/kg-viz-bench.mjs`.
//!
//! Usage: kg_render_gen <graph.sqlite> <entities> [per_section=10] [cross_every=4]

use docfoo_kg::store::scene;
use docfoo_kg::store::{KgStore, OpenMode};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: kg_render_gen <graph.sqlite> <entities> [per_section=10] [cross_every=4]");
        std::process::exit(2);
    }
    let target = PathBuf::from(&args[1]);
    let entities: usize = args[2].parse().unwrap_or_else(|_| {
        eprintln!("kg_render_gen: <entities> must be an integer");
        std::process::exit(2);
    });
    let per_section: usize = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .filter(|&value| value > 0)
        .unwrap_or(10);
    let cross_every: usize = args
        .get(4)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).expect("create graph dir");
        }
    }
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut raw = target.as_os_str().to_os_string();
        raw.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(raw));
    }

    let sections = entities.div_ceil(per_section);
    let started = Instant::now();
    let store = KgStore::open(&target, OpenMode::ReadWrite).expect("open store");
    let conn = store.connection();

    let tx = conn.unchecked_transaction().expect("begin transaction");
    {
        let mut section_stmt = tx
            .prepare(
                "INSERT INTO sections(row, title, topic, text, source_doc, start_line, end_line,
                                      content_hash, retry_pending, tokens)
                 VALUES (?1, ?2, ?3, '', '', 0, 0, '', 0, '')",
            )
            .unwrap();
        for s in 0..sections {
            section_stmt
                .execute(rusqlite::params![
                    s as i64,
                    format!("[RND] Section {s:07}"),
                    format!("[RND] Topic {:02}", s % 64)
                ])
                .unwrap();
        }

        let mut entity_stmt = tx
            .prepare(
                "INSERT INTO entities(node, entity_key, entity_slug, name, type, desc, tokens)
                 VALUES (?1, ?2, ?3, ?4, 'CONCEPT', '', '')",
            )
            .unwrap();
        let mut entity_section_stmt = tx
            .prepare("INSERT INTO entity_sections(node, section_row, ord) VALUES (?1, ?2, 0)")
            .unwrap();
        let mut section_entity_stmt = tx
            .prepare("INSERT INTO section_entities(section_row, entity_key, ord) VALUES (?1, ?2, ?3)")
            .unwrap();
        let mut relation_stmt = tx
            .prepare(
                "INSERT OR IGNORE INTO relations(src, dst, rel, section_row, source_doc)
                 VALUES (?1, ?2, 'USES', ?3, NULL)",
            )
            .unwrap();

        let mut relation_count = 0usize;
        for s in 0..sections {
            let first = s * per_section;
            let count = per_section.min(entities - first);
            for e in 0..count {
                let node = first + e;
                let key = format!("CONCEPT_rnd_{node:07}");
                entity_stmt
                    .execute(rusqlite::params![
                        node as i64,
                        key,
                        key,
                        format!("generated concept {node}")
                    ])
                    .unwrap();
                entity_section_stmt
                    .execute(rusqlite::params![node as i64, s as i64])
                    .unwrap();
                section_entity_stmt
                    .execute(rusqlite::params![s as i64, key, e as i64])
                    .unwrap();
            }
            // Local chain inside the section.
            for e in 1..count {
                let src = (first + e - 1) as i64;
                let dst = (first + e) as i64;
                relation_stmt
                    .execute(rusqlite::params![src, dst, s as i64])
                    .unwrap();
                relation_count += 1;
            }
        }
        // Deterministic long-range links so communities merge and the layout
        // has to place something richer than 100k separate chains.
        for node in (0..entities).step_by(cross_every) {
            let dst = (node * 7919 + 104_729) % entities;
            if dst == node {
                continue;
            }
            relation_stmt
                .execute(rusqlite::params![
                    node as i64,
                    dst as i64,
                    (node / per_section) as i64
                ])
                .unwrap();
            relation_count += 1;
        }

        store
            .set_meta("entity_count", &entities.to_string())
            .unwrap();
        store
            .set_meta("relation_count", &relation_count.to_string())
            .unwrap();
        store
            .set_meta("section_count", &sections.to_string())
            .unwrap();
        store.set_meta("derived_ready", "1").unwrap();
        store.set_meta("noise_floor", "0.0").unwrap();
        store
            .set_meta("build_uid", &format!("render-{entities}-{}", std::process::id()))
            .unwrap();
    }
    tx.commit().expect("commit");
    let wal_started = Instant::now();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint");
    let rows_ms = started.elapsed().as_secs_f64() * 1000.0;

    let hash = store.build_uid().unwrap();
    let scene_started = Instant::now();
    let bytes = scene::build_scene(&store, &hash).expect("pack scene");
    let scene_ms = scene_started.elapsed().as_secs_f64() * 1000.0;
    let scene_path = target.with_file_name("scene.bin");
    std::fs::write(&scene_path, &bytes).expect("write scene.bin");

    println!(
        "wrote {} ({} entities, {} sections, {} relations)",
        target.display(),
        entities,
        sections,
        store.meta("relation_count").unwrap().unwrap_or_default()
    );
    println!(
        "rows+meta: {:.1} ms (checkpoint {:.1} ms) · scene.bin {:.1} MB in {:.1} ms · total {:.1} s",
        rows_ms,
        wal_started.elapsed().as_secs_f64() * 1000.0,
        bytes.len() as f64 / (1024.0 * 1024.0),
        scene_ms,
        started.elapsed().as_secs_f64()
    );
}
