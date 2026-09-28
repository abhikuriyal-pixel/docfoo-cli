//! Dev-only: time the SQLite read path on a graph store.
//!
//! Usage: kg_bench <graph.sqlite> [query]

use docfoo_kg::store::{KgStore, OpenMode};
use std::path::Path;
use std::time::Instant;

fn avg_ms(total: std::time::Duration, runs: u32) -> f64 {
    total.as_secs_f64() * 1000.0 / runs as f64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: kg_bench <graph.sqlite> [query]");
        std::process::exit(2);
    }
    let path = Path::new(&args[1]);
    let query = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "generated concept topic".to_string());

    let started = Instant::now();
    let store = KgStore::open(path, OpenMode::ReadOnly).unwrap_or_else(|e| {
        eprintln!("kg_bench: could not open {}: {e}", path.display());
        std::process::exit(1);
    });
    println!("open: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    println!(
        "counts: entities={} relations={} sections={}",
        store.meta("entity_count").unwrap().unwrap_or_default(),
        store.meta("relation_count").unwrap().unwrap_or_default(),
        store.meta("section_count").unwrap().unwrap_or_default(),
    );

    // Exact BM25 scores every document that contains at least one query
    // token, so candidate counts explain the timings below.
    let tokens = docfoo_kg::text::tokenize(&query);
    let fts_query = tokens.join(" OR ");
    for (label, table) in [
        ("entity", "entity_fts"),
        ("section", "section_fts"),
        ("concept", "concept_fts"),
    ] {
        let sql = format!("SELECT count(*) FROM {table} WHERE {table} MATCH ?1");
        let candidates: i64 = store
            .connection()
            .query_row(&sql, [&fts_query], |row| row.get(0))
            .unwrap();
        println!("candidates[{label}]: {candidates}");
    }

    const RUNS: u32 = 5;
    let mut start = Instant::now();
    let mut hits = 0usize;
    for _ in 0..RUNS {
        hits = store.bm25_entity_search(&query, 12).unwrap().len();
    }
    println!(
        "entity bm25 warm: {:.1} ms avg ({hits} hits) [query {query:?}]",
        avg_ms(start.elapsed(), RUNS)
    );

    start = Instant::now();
    for _ in 0..RUNS {
        hits = store.bm25_section_search(&query, 12).unwrap().len();
    }
    println!("section bm25 warm: {:.1} ms avg ({hits} hits)", avg_ms(start.elapsed(), RUNS));

    start = Instant::now();
    for _ in 0..RUNS {
        hits = store.bm25_concept_search(&query, 12).unwrap().len();
    }
    println!("concept bm25 warm: {:.1} ms avg ({hits} hits)", avg_ms(start.elapsed(), RUNS));

    start = Instant::now();
    for _ in 0..RUNS {
        hits = store.concept_shortlist(&query, 200).unwrap().len();
    }
    println!("concept shortlist: {:.1} ms avg ({hits} candidates)", avg_ms(start.elapsed(), RUNS));

    start = Instant::now();
    for _ in 0..RUNS {
        hits = store.locate_direct_sections(&query, 6).unwrap().len();
    }
    println!("locator: {:.1} ms avg ({hits} sections)", avg_ms(start.elapsed(), RUNS));

    // Extraction glossary: exact-name matches + a live FTS fill seeded by the
    // section's rarest tokens (entity_vocab).
    let section_text: String = store
        .connection()
        .query_row("SELECT text FROM sections ORDER BY row LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap_or_default();
    start = Instant::now();
    let mut glossary = 0;
    for _ in 0..RUNS {
        glossary = store
            .glossary(&section_text, docfoo_kg::config::GLOSSARY_K)
            .unwrap()
            .len();
    }
    println!(
        "glossary: {:.1} ms avg ({glossary} entries, {} chars)",
        avg_ms(start.elapsed(), RUNS),
        section_text.len()
    );

    // Noise floor: seeded samples of the common vocabulary, each scored with
    // the same native search the query gate uses (build-finalize cost).
    // Optional 3rd arg sets the sample count (default 300).
    let samples: usize = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);
    let df = store.entity_df().unwrap();
    let mut common: Vec<String> = df
        .iter()
        .filter(|(_, &count)| count >= 20)
        .map(|(term, _)| term.clone())
        .collect();
    common.sort();
    if common.len() < 4 {
        let mut by_freq: Vec<(String, usize)> = df.into_iter().collect();
        by_freq.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        common = by_freq.into_iter().take(10).map(|(term, _)| term).collect();
    }
    let mut rng = docfoo_kg::text::Rng::new(42);
    let mut tops: Vec<f64> = vec![];
    start = Instant::now();
    for _ in 0..samples {
        let picks = rng.sample_indices(4.min(common.len()), common.len());
        let q: Vec<String> = picks.iter().map(|&i| common[i].clone()).collect();
        let best = store
            .bm25_entity_search(&q.join(" "), 1)
            .unwrap()
            .first()
            .map(|(_, score)| *score)
            .unwrap_or(0.0);
        if best > 0.0 {
            tops.push(best);
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "noise floor: {:.0} ms ({samples} samples, {} positive; ~{:.0} ms at 300)",
        elapsed,
        tops.len(),
        elapsed * 300.0 / samples.max(1) as f64
    );

    let first: String = store
        .connection()
        .query_row("SELECT entity_key FROM entities ORDER BY node LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    start = Instant::now();
    let neighbors = store.neighbors(std::slice::from_ref(&first)).unwrap();
    println!(
        "neighbors({first}): {:.2} ms ({} ids)",
        start.elapsed().as_secs_f64() * 1000.0,
        neighbors.get(&first).map(|v| v.len()).unwrap_or(0)
    );

    // Viewer projection: the SQL half of kg_graph_data (the shell still builds
    // and serializes the JSON value on top of this).
    start = Instant::now();
    let conn = store.connection();
    let mut ids: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT entity_key FROM entities ORDER BY node").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            ids.push(row.get(0).unwrap());
        }
    }
    let n = ids.len();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::with_capacity(n);
    for (i, id) in ids.iter().enumerate() {
        index.insert(id.as_str(), i);
    }
    let mut degrees = vec![0u32; n];
    {
        let mut stmt = conn
            .prepare(
                "SELECT node, COUNT(*) FROM (
                     SELECT src AS node FROM relations
                     UNION ALL SELECT dst FROM relations
                 ) GROUP BY node",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let node: i64 = row.get(0).unwrap();
            let count: i64 = row.get(1).unwrap();
            if node >= 0 && (node as usize) < n {
                degrees[node as usize] = count as u32;
            }
        }
    }
    let mut edges: Vec<u32> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT src, dst FROM relations ORDER BY rid").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            edges.push(row.get::<_, i64>(0).unwrap() as u32);
            edges.push(row.get::<_, i64>(1).unwrap() as u32);
        }
    }
    let mut section_count = 0usize;
    {
        let mut stmt = conn.prepare("SELECT row, title FROM sections ORDER BY row").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let _: String = row.get(1).unwrap();
            section_count += 1;
        }
    }
    println!(
        "viewer projection SQL: {:.1} ms ({n} nodes, {} edges, {section_count} sections)",
        start.elapsed().as_secs_f64() * 1000.0,
        edges.len() / 2
    );
}
