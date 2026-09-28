//! Store tests: lossless round trips, native ranking and the atomic-write
//! sidecar rules.

use super::*;
use crate::graph::{entity_text, Concept, KnowledgeGraph, SectionInfo, TopicInfo};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("docfoo-kg-store-{tag}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn db_path(tag: &str) -> (PathBuf, PathBuf) {
    let dir = temp_dir(tag);
    (dir.join("graph.sqlite"), dir)
}

/// Exercises entity merging, ordered provenance, concepts (Some/None/empty),
/// topics with a dangling section, dangling entity ids, and an empty entity
/// description.
fn sample_graph() -> KnowledgeGraph {
    let mut kg = KnowledgeGraph::default();
    kg.add_entity("CONCEPT_beta", "Beta Device", "CONCEPT", "second entity about caches", "S2 No Concept", Some("docs/b.md"));
    kg.add_entity("CONCEPT_alpha", "Alpha", "CONCEPT", "first entity", "S1", Some("docs/a.md"));
    kg.add_entity("CONCEPT_alpha", "Alpha", "CONCEPT", "a much longer alpha description", "S3 Empty Concept", Some("docs/c.md"));
    kg.add_entity("DEVICE_iso", "Isolated", "DEVICE", "no links here", "S1", None);
    kg.add_entity("PERSON_zed", "Zed", "PERSON", "", "S2 No Concept", None);

    kg.add_relation("CONCEPT_alpha", "CONCEPT_beta", "USES", "S1", Some("docs/a.md"));
    kg.add_relation("CONCEPT_beta", "DEVICE_iso", "ENABLES", "S2 No Concept", None);
    kg.add_relation("PERSON_zed", "CONCEPT_alpha", "INVENTED_BY", "S3 Empty Concept", Some("docs/c.md"));

    kg.sections.insert(
        "S1".into(),
        SectionInfo {
            topic: Some("[A] Chapter 1".into()),
            entity_ids: vec![
                "CONCEPT_alpha".into(),
                "DEVICE_iso".into(),
                "GONE_dangling".into(),
            ],
            concept: Some(Concept {
                name: "Alpha concept".into(),
                summary: "about alpha and caches".into(),
                terms: vec!["first thing".into(), "primo".into()],
            }),
            text: "Alpha text with caches and processors. ".repeat(3),
            source_doc: "docs/a.md".into(),
            start_line: 3,
            end_line: 9,
            content_hash: "hash-s1".into(),
            retry_pending: false,
        },
    );
    kg.sections.insert(
        "S2 No Concept".into(),
        SectionInfo {
            topic: None,
            entity_ids: vec!["CONCEPT_beta".into(), "PERSON_zed".into()],
            concept: None,
            text: "Second section text about memory.".into(),
            source_doc: "docs/b.md".into(),
            start_line: 0,
            end_line: 0,
            content_hash: String::new(),
            retry_pending: true,
        },
    );
    kg.sections.insert(
        "S3 Empty Concept".into(),
        SectionInfo {
            topic: Some("[A] Chapter 2".into()),
            entity_ids: vec![],
            concept: Some(Concept::default()),
            text: String::new(),
            source_doc: "docs/c.md".into(),
            ..Default::default()
        },
    );

    // Dangling *topic* provenance still survives (topics are plain titles),
    // and `section_entities` may cite entity ids that no longer exist.
    kg.topics.insert(
        "[A] Chapter 1".into(),
        TopicInfo { sections: vec!["S1".into(), "S_MISSING".into()] },
    );
    kg.topics.insert(
        "Other".into(),
        TopicInfo { sections: vec!["S2 No Concept".into()] },
    );
    kg.source_hashes.insert("docs/a.md".into(), "hash-a".into());
    kg.source_hashes.insert("docs/b.md".into(), "hash-b".into());
    kg.noise_floor = Some(0.031);
    kg
}

fn assert_same_graph(expected: &KnowledgeGraph, actual: &KnowledgeGraph) {
    assert_eq!(actual.entities, expected.entities, "entities");
    assert_eq!(actual.relations, expected.relations, "relations");
    assert_eq!(actual.sections, expected.sections, "sections");
    assert_eq!(actual.topics, expected.topics, "topics");
    assert_eq!(actual.source_hashes, expected.source_hashes, "source hashes");
    assert_eq!(actual.noise_floor, expected.noise_floor, "noise floor");
}

#[test]
fn fts5_is_compiled_in() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE VIRTUAL TABLE probe USING fts5(x)").unwrap();
    conn.execute("INSERT INTO probe(x) VALUES ('hello world')", []).unwrap();
    let hits: i64 = conn
        .query_row("SELECT count(*) FROM probe WHERE probe MATCH 'hello'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(hits, 1);
}

#[test]
fn round_trip_is_lossless_and_order_preserving() {
    let (path, dir) = db_path("round-trip");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();

    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    let mut back = KnowledgeGraph::default();
    store.load_into(&mut back).unwrap();
    assert_same_graph(&kg, &back);

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(store.derived_ready().unwrap());
    assert!(!store.build_uid().unwrap().is_empty());
    assert_eq!(store.meta(META_ENTITY_COUNT).unwrap().as_deref(), Some("4"));
    assert_eq!(store.meta(META_RELATION_COUNT).unwrap().as_deref(), Some("3"));
    assert_eq!(store.meta(META_SECTION_COUNT).unwrap().as_deref(), Some("3"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fts_indexes_are_populated_from_the_tokenized_corpus() {
    let (path, dir) = db_path("fts");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let conn = Connection::open(&path).unwrap();

    // Entity corpus: "alpha" lives on CONCEPT_alpha's description.
    let key: String = conn
        .query_row(
            "SELECT e.entity_key FROM entity_fts
             JOIN entities e ON e.node = entity_fts.rowid
             WHERE entity_fts MATCH 'alpha' ORDER BY e.node",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(key, "CONCEPT_alpha");

    // Section corpus: the title and body are indexed.
    let title: String = conn
        .query_row(
            "SELECT s.title FROM section_fts
             JOIN sections s ON s.row = section_fts.rowid
             WHERE section_fts MATCH 'memory' ORDER BY s.row",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "S2 No Concept");

    // Concept corpus: Every section has a row (the title fallback keeps
    // concept-less sections searchable). "empty" only exists in S3's title.
    let concept_hits: i64 = conn
        .query_row("SELECT count(*) FROM concept_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(concept_hits, 3);
    let fallback: String = conn
        .query_row(
            "SELECT s.title FROM concept_fts
             JOIN sections s ON s.row = concept_fts.rowid
             WHERE concept_fts MATCH 'empty' ORDER BY s.row",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fallback, "S3 Empty Concept");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_full_is_atomic_and_leaves_no_sidecars() {
    let (path, dir) = db_path("sidecars");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    assert!(path.is_file());
    for suffix in [".tmp", "-wal", "-shm", "-journal"] {
        assert!(
            !sidecar_path(&path, suffix).exists(),
            "leftover {}",
            sidecar_path(&path, suffix).display()
        );
    }

    // A second write replaces the graph and refreshes the build uid.
    let uid_before = KgStore::open(&path, OpenMode::ReadOnly).unwrap().build_uid().unwrap();
    let mut second = sample_graph();
    second.noise_floor = Some(0.5);
    KgStore::write_full(&second, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    assert_ne!(store.build_uid().unwrap(), uid_before);
    assert_eq!(store.noise_floor().unwrap(), Some(0.5));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dangling_relation_endpoints_are_rejected() {
    let (path, dir) = db_path("dangling");
    let mut kg = sample_graph();
    kg.add_relation("CONCEPT_alpha", "GONE_missing", "USES", "S1", None);
    let error = KgStore::write_full(&kg, &path).unwrap_err();
    assert!(matches!(error, StoreError::Data(_)), "got {error:?}");
    assert!(!path.exists(), "failed write must not create the target");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_graph_round_trips() {
    let (path, dir) = db_path("empty");
    let kg = KnowledgeGraph::default();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    let mut back = KnowledgeGraph::default();
    store.load_into(&mut back).unwrap();
    assert_same_graph(&kg, &back);
    assert!(store.derived_ready().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_only_open_rejects_missing_or_wrong_schema() {
    let dir = temp_dir("schema-check");
    let missing = dir.join("missing.sqlite");
    assert!(matches!(
        KgStore::open(&missing, OpenMode::ReadOnly),
        Err(StoreError::Sqlite(_))
    ));

    let empty = dir.join("empty.sqlite");
    Connection::open(&empty).unwrap();
    assert!(matches!(
        KgStore::open(&empty, OpenMode::ReadOnly),
        Err(StoreError::Schema(_))
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_write_open_rejects_a_wrong_schema_version() {
    let dir = temp_dir("schema-rw");
    let old = dir.join("old.sqlite");
    {
        let conn = Connection::open(&old).unwrap();
        conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '2')",
            [],
        )
        .unwrap();
    }
    let result = KgStore::open(&old, OpenMode::ReadWrite);
    assert!(
        matches!(result, Err(StoreError::Schema(_))),
        "expected the wrong version to be refused"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Chunk 2: the SQLite query path against the in-memory fit
// ---------------------------------------------------------------------------

#[test]
fn native_search_ranks_with_fts_bm25() {
    let (path, dir) = db_path("native-rank");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();

    let hits = store.bm25_entity_search("alpha caches", 5).unwrap();
    assert!(hits.iter().any(|(id, _)| id == "CONCEPT_alpha"), "{hits:?}");
    assert!(hits.iter().any(|(id, _)| id == "CONCEPT_beta"), "{hits:?}");
    // `-bm25()` scores are ordered best-first, matching the query plan.
    assert!(
        hits.windows(2).all(|pair| pair[0].1 >= pair[1].1),
        "scores must be non-increasing: {hits:?}"
    );
    assert_eq!(store.bm25_entity_search("concept", 1).unwrap().len(), 1);
    assert!(store.bm25_entity_search("the and", 5).unwrap().is_empty());

    let sections = store.bm25_section_search("memory", 5).unwrap();
    assert_eq!(sections.first().map(|(title, _)| title.as_str()), Some("S2 No Concept"));
    let concepts = store.bm25_concept_search("alpha", 5).unwrap();
    assert!(!concepts.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_locator_prefers_rare_tokens_and_titles() {
    let (path, dir) = db_path("locator");
    let mut kg = sample_graph();
    kg.sections.insert(
        "S3 Codes".into(),
        SectionInfo {
            text: "The Z80X12 controller coordinates refresh cycles. ".repeat(4),
            ..Default::default()
        },
    );
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    let hits = store.locate_direct_sections("Z80X12 controller", 6).unwrap();
    assert_eq!(hits.first().map(String::as_str), Some("S3 Codes"));
    // Tokens absent from every section select no candidates.
    assert!(store.locate_direct_sections("xyzzy plugh", 6).unwrap().is_empty());
    assert!(store.locate_direct_sections("Z80X12", 0).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_adjacency_and_primaries_match_in_memory() {
    let (path, dir) = db_path("adjacency-parity");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();

    let ids: Vec<String> = kg.entities.keys().cloned().collect();
    let expected: HashMap<String, Vec<String>> = ids
        .iter()
        .map(|id| (id.clone(), kg.neighbors(id)))
        .collect();
    assert_eq!(store.neighbors(&ids).unwrap(), expected);

    let primaries = store.entity_primaries(&ids).unwrap();
    for id in &ids {
        let expected = kg
            .entities
            .get(id)
            .and_then(|entity| entity.sections.first())
            .map(|primary| crate::reader::EntityPrimary {
                section: primary.clone(),
                topic: kg.sections.get(primary).and_then(|info| info.topic.clone()),
            });
        assert_eq!(primaries.get(id), expected.as_ref(), "{id}");
    }

    let visited: HashSet<String> = ids.iter().cloned().collect();
    let expected_triples: Vec<(String, String, String)> = kg
        .relations
        .iter()
        .filter(|relation| {
            visited.contains(&relation.source) && visited.contains(&relation.target)
        })
        .take(40)
        .map(|relation| {
            (
                kg.entities[&relation.source].name.clone(),
                relation.rel.clone(),
                kg.entities[&relation.target].name.clone(),
            )
        })
        .collect();
    assert_eq!(store.relations_among(&visited, 40).unwrap(), expected_triples);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concept_shortlist_covers_small_graphs_and_respects_the_budget() {
    let (path, dir) = db_path("shortlist");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();

    let all: Vec<String> = kg.sections.keys().cloned().collect();
    let shortlist = store.concept_shortlist("alpha caches", 10).unwrap();
    assert_eq!(shortlist.len(), all.len());
    for title in &all {
        assert!(shortlist.contains(title), "missing {title}");
    }

    let capped = store.concept_shortlist("alpha caches", 2).unwrap();
    assert_eq!(capped.len(), 2);
    assert!(capped.contains(&"S1".to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fts_triggers_track_insert_update_delete() {
    let (path, dir) = db_path("fts-triggers");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    let conn = store.connection();

    // Insert through the base table only: the trigger fills the FTS index.
    conn.execute(
        "INSERT INTO entities(node, entity_key, entity_slug, name, type, desc, tokens)
         VALUES (99, 'X_new', 'nova', 'Nova', 'CONCEPT', 'novel widget', 'nova widget')",
        [],
    )
    .unwrap();
    let count = |table: &str, term: &str| -> i64 {
        conn.query_row(
            &format!("SELECT count(*) FROM {table} WHERE {table} MATCH ?1"),
            [term],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(count("entity_fts", "nova"), 1);

    // Updating the tokens column swaps the indexed terms.
    conn.execute("UPDATE entities SET tokens = 'changed corpus' WHERE node = 99", [])
        .unwrap();
    assert_eq!(count("entity_fts", "nova"), 0);
    assert_eq!(count("entity_fts", "changed"), 1);

    // The writer maintains the raw locator vocabulary; its triggers keep
    // locator_df current across replaces and cascading section deletes.
    let row = store
        .upsert_section(&SectionInput {
            title: "T",
            text: "Z80X12 controller",
            ..Default::default()
        })
        .unwrap();
    assert_eq!(count("section_fts", "z80x12"), 1);
    let df = |term: &str| -> i64 {
        conn.query_row("SELECT df FROM locator_df WHERE term = ?1", [term], |row| {
            row.get(0)
        })
        .unwrap_or(0)
    };
    assert_eq!(df("z80x12"), 1);
    store
        .upsert_section(&SectionInput {
            title: "T",
            text: "other text",
            ..Default::default()
        })
        .unwrap();
    assert_eq!(df("z80x12"), 0);
    assert_eq!(df("other"), 1);

    // Deletes remove the row from every index; the section cascade also
    // drains its locator vocabulary.
    conn.execute("DELETE FROM sections WHERE row = ?1", [row]).unwrap();
    conn.execute("DELETE FROM entities WHERE node = 99", []).unwrap();
    assert_eq!(count("section_fts", "other"), 0);
    assert_eq!(count("entity_fts", "changed"), 0);
    assert_eq!(df("other"), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn section_delete_cascades_and_gcs_orphan_entities() {
    let (path, dir) = db_path("cascade-gc");
    let mut kg = KnowledgeGraph::default();
    kg.add_entity("CONCEPT_orphan", "Orphan", "CONCEPT", "only in S1", "S1", Some("a.md"));
    kg.add_entity("CONCEPT_shared", "Shared", "CONCEPT", "in both", "S1", Some("a.md"));
    kg.add_entity("CONCEPT_shared", "Shared", "CONCEPT", "in both", "S2", Some("b.md"));
    kg.add_entity("CONCEPT_keep", "Keep", "CONCEPT", "only in S2", "S2", Some("b.md"));
    kg.add_relation("CONCEPT_orphan", "CONCEPT_shared", "USES", "S1", Some("a.md"));
    kg.add_relation("CONCEPT_shared", "CONCEPT_keep", "USES", "S2", Some("b.md"));
    kg.sections.insert("S1".into(), SectionInfo {
        source_doc: "a.md".into(),
        entity_ids: vec!["CONCEPT_orphan".into(), "CONCEPT_shared".into()],
        ..Default::default()
    });
    kg.sections.insert("S2".into(), SectionInfo {
        source_doc: "b.md".into(),
        entity_ids: vec!["CONCEPT_shared".into(), "CONCEPT_keep".into()],
        ..Default::default()
    });
    KgStore::write_full(&kg, &path).unwrap();

    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    store.remove_sections(&["S1".to_string()]).unwrap();
    drop(store);

    let stored = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    let mut back = KnowledgeGraph::default();
    stored.load_into(&mut back).unwrap();

    assert!(!back.entities.contains_key("CONCEPT_orphan"), "orphan must be GCed");
    assert!(!back.sections.contains_key("S1"));
    assert_eq!(back.relations.len(), 1, "S1's relation cascaded");
    assert_eq!(back.relations[0].source, "CONCEPT_shared");
    let shared = &back.entities["CONCEPT_shared"];
    assert_eq!(shared.sections, vec!["S2".to_string()]);
    assert_eq!(shared.source_doc, vec!["b.md".to_string()], "docs recomputed");
    assert_eq!(back.entities["CONCEPT_keep"].sections, vec!["S2".to_string()]);

    // The derived indexes no longer mention the removed section's content.
    assert_eq!(stored.bm25_entity_search("orphan", 5).unwrap().len(), 0);
    assert_eq!(stored.bm25_section_search("only in s1", 5).unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn relation_triples_are_unique() {
    let (path, dir) = db_path("unique-triple");
    let mut kg = sample_graph();
    // A hand-built graph may contain an exact duplicate; the unique index
    // keeps the first occurrence instead of failing or double-counting.
    kg.relations.push(crate::graph::Relation {
        source: "CONCEPT_alpha".into(),
        target: "CONCEPT_beta".into(),
        rel: "USES".into(),
        section: "S1".into(),
        source_doc: Some("docs/a.md".into()),
    });
    KgStore::write_full(&kg, &path).unwrap();

    let store = KgStore::open(&path, OpenMode::ReadOnly).unwrap();
    let rows: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM relations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 3, "the duplicate triple is ignored");
    let mut back = KnowledgeGraph::default();
    store.load_into(&mut back).unwrap();
    assert_eq!(back.relations.len(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn foreign_keys_reject_dangling_provenance() {
    let (path, dir) = db_path("fk");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    let conn = store.connection();

    // Join to a section that does not exist.
    assert!(conn
        .execute(
            "INSERT INTO entity_sections(node, section_row, ord) VALUES (0, 999999, 0)",
            [],
        )
        .is_err());
    // Relation with a missing endpoint.
    assert!(conn
        .execute(
            "INSERT INTO relations(src, dst, rel, section_row)
             SELECT 0, 999999, 'USES', row FROM sections LIMIT 1",
            [],
        )
        .is_err());

    // Deleting an entity cascades its joins.
    let iso_node: i64 = conn
        .query_row(
            "SELECT node FROM entities WHERE entity_key = 'DEVICE_iso'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute("DELETE FROM entities WHERE entity_key = 'DEVICE_iso'", [])
        .unwrap();
    let joins: i64 = conn
        .query_row(
            "SELECT count(*) FROM entity_sections WHERE node = ?1",
            [iso_node],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(joins, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_full_rejects_dangling_entity_sections() {
    let (path, dir) = db_path("dangling-entity-section");
    let mut kg = sample_graph();
    kg.entities
        .get_mut("DEVICE_iso")
        .unwrap()
        .sections
        .push("S_MISSING".into());
    let error = KgStore::write_full(&kg, &path).unwrap_err();
    assert!(matches!(error, StoreError::Data(_)), "got {error:?}");
    assert!(!path.exists(), "failed write must not create the target");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Phase 1: native writer primitives and the live glossary
// ---------------------------------------------------------------------------

/// The same logical graph built by `write_full` and by the native primitives:
/// a longer description wins, the first name/type wins, and an exact duplicate
/// relation triple is ignored.
fn merge_fixture() -> KnowledgeGraph {
    let mut kg = KnowledgeGraph::default();
    kg.add_entity("CONCEPT_alpha", "Alpha", "CONCEPT", "short", "S1", Some("a.md"));
    kg.add_entity("CONCEPT_beta", "Beta", "CONCEPT", "beta one", "S1", Some("a.md"));
    kg.add_relation("CONCEPT_alpha", "CONCEPT_beta", "USES", "S1", Some("a.md"));
    kg.sections.insert(
        "S1".into(),
        SectionInfo {
            entity_ids: vec!["CONCEPT_alpha".into(), "CONCEPT_beta".into()],
            concept: Some(Concept {
                name: "Alpha beta".into(),
                summary: "about alpha and beta".into(),
                terms: vec!["first".into()],
            }),
            text: "Alpha and beta text. ".repeat(2),
            source_doc: "a.md".into(),
            start_line: 1,
            end_line: 5,
            content_hash: "h1".into(),
            ..Default::default()
        },
    );
    kg.add_entity(
        "CONCEPT_alpha",
        "Alpha",
        "CONCEPT",
        "a much longer alpha description",
        "S2 No Concept",
        Some("b.md"),
    );
    kg.add_entity("CONCEPT_beta", "Beta", "DEVICE", "beta two", "S2 No Concept", Some("b.md"));
    kg.add_entity("PERSON_zed", "Zed", "PERSON", "", "S2 No Concept", Some("b.md"));
    kg.add_relation("CONCEPT_alpha", "CONCEPT_beta", "USES", "S2 No Concept", Some("b.md"));
    kg.add_relation("CONCEPT_beta", "PERSON_zed", "ENABLES", "S2 No Concept", None);
    kg.sections.insert(
        "S2 No Concept".into(),
        SectionInfo {
            entity_ids: vec![
                "CONCEPT_alpha".into(),
                "CONCEPT_beta".into(),
                "PERSON_zed".into(),
            ],
            concept: None,
            text: "Second text about beta.".into(),
            source_doc: "b.md".into(),
            ..Default::default()
        },
    );
    kg.sections.insert(
        "S3 Empty Concept".into(),
        SectionInfo {
            concept: Some(Concept::default()),
            text: String::new(),
            source_doc: "c.md".into(),
            ..Default::default()
        },
    );
    kg
}

/// Write `kg` through the native primitives, section by section.
fn build_native(store: &KgStore, kg: &KnowledgeGraph) {
    for (title, info) in &kg.sections {
        let row = store
            .upsert_section(&SectionInput {
                title,
                text: &info.text,
                source_doc: &info.source_doc,
                start_line: info.start_line,
                end_line: info.end_line,
                content_hash: &info.content_hash,
                retry_pending: info.retry_pending,
                topic: info.topic.as_deref(),
            })
            .unwrap();
        for key in &info.entity_ids {
            let entity = &kg.entities[key];
            let upserted = store
                .upsert_entity(&entity.id, &entity.name, &entity.etype, &entity.desc)
                .unwrap();
            store.link_entity_section(upserted.node, row).unwrap();
            if !info.source_doc.is_empty() {
                store.link_entity_doc(upserted.node, &info.source_doc).unwrap();
            }
        }
        store.set_section_entities(row, &info.entity_ids).unwrap();
        store.refresh_section_tokens(row).unwrap();
        store.upsert_concept(row, info.concept.as_ref()).unwrap();
        for relation in kg.relations.iter().filter(|r| r.section == *title) {
            store
                .insert_relation(
                    &relation.source,
                    &relation.target,
                    &relation.rel,
                    row,
                    relation.source_doc.as_deref(),
                )
                .unwrap();
        }
    }
}

#[test]
fn native_writer_matches_write_full_round_trip() {
    let (write_path, dir) = db_path("native-parity");
    let native_path = dir.join("native.sqlite");
    let kg = merge_fixture();

    KgStore::write_full(&kg, &write_path).unwrap();
    let native = KgStore::open(&native_path, OpenMode::ReadWrite).unwrap();
    build_native(&native, &kg);

    let mut expected = KnowledgeGraph::default();
    let write_store = KgStore::open(&write_path, OpenMode::ReadOnly).unwrap();
    write_store.load_into(&mut expected).unwrap();
    let mut actual = KnowledgeGraph::default();
    native.load_into(&mut actual).unwrap();
    assert_eq!(actual.entities, expected.entities, "entities");
    assert_eq!(actual.relations, expected.relations, "relations");
    assert_eq!(actual.sections, expected.sections, "sections");

    // Stored corpora match the model's tokenization exactly.
    for (title, info) in &kg.sections {
        let stored: String = native
            .connection()
            .query_row(
                "SELECT tokens FROM sections WHERE title = ?1",
                [title],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored,
            kg.tokenize(&kg.section_text(title, info)).join(" "),
            "section {title}"
        );
        let concept: String = native
            .connection()
            .query_row(
                "SELECT c.tokens FROM concepts c JOIN sections s ON s.row = c.section_row
                 WHERE s.title = ?1",
                [title],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            concept,
            kg.tokenize(&crate::graph::concept_text(title, info)).join(" "),
            "concept {title}"
        );
    }
    for (key, entity) in &kg.entities {
        let stored: String = native
            .connection()
            .query_row(
                "SELECT tokens FROM entities WHERE entity_key = ?1",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, kg.tokenize(&entity_text(entity)).join(" "), "entity {key}");
    }

    // Both stores rank identically with native FTS5 bm25 (no stats tables).
    for query in ["alpha caches", "beta device", "isolated zed", "missing", "concept", ""] {
        assert_eq!(
            native.bm25_entity_search(query, 5).unwrap(),
            write_store.bm25_entity_search(query, 5).unwrap(),
            "entities {query:?}"
        );
        assert_eq!(
            native.bm25_section_search(query, 5).unwrap(),
            write_store.bm25_section_search(query, 5).unwrap(),
            "sections {query:?}"
        );
        assert_eq!(
            native.bm25_concept_search(query, 5).unwrap(),
            write_store.bm25_concept_search(query, 5).unwrap(),
            "concepts {query:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn native_writer_dedups_joins_and_relations() {
    let (path, dir) = db_path("native-dedup");
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    let row = store
        .upsert_section(&SectionInput {
            title: "S",
            text: "body",
            ..Default::default()
        })
        .unwrap();
    let node = store.upsert_entity("CONCEPT_x", "X", "CONCEPT", "x").unwrap().node;
    store.link_entity_section(node, row).unwrap();
    store.link_entity_section(node, row).unwrap();
    store.link_entity_doc(node, "a.md").unwrap();
    store.link_entity_doc(node, "a.md").unwrap();
    assert!(store
        .insert_relation("CONCEPT_x", "CONCEPT_x", "SELF", row, None)
        .unwrap());
    assert!(!store
        .insert_relation("CONCEPT_x", "CONCEPT_x", "SELF", row, None)
        .unwrap());

    let count = |table: &str| -> i64 {
        store
            .connection()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(count("entity_sections"), 1);
    assert_eq!(count("entity_docs"), 1);
    assert_eq!(count("relations"), 1);
    assert!(matches!(
        store.insert_relation("CONCEPT_x", "GONE_y", "USES", row, None),
        Err(StoreError::Data(_))
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn upsert_entity_merges_like_the_in_memory_model() {
    let (path, dir) = db_path("native-merge");
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    let node = store
        .upsert_entity("CONCEPT_x", "First Name", "CONCEPT", "short")
        .unwrap();
    let same = store
        .upsert_entity("CONCEPT_x", "Second Name", "DEVICE", "a longer description")
        .unwrap();
    assert_eq!(node, same);
    let (name, etype, desc): (String, String, String) = store
        .connection()
        .query_row(
            "SELECT name, type, desc FROM entities WHERE node = ?1",
            [node.node],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(name, "First Name", "first name wins");
    assert_eq!(etype, "CONCEPT", "first type wins");
    assert_eq!(desc, "a longer description", "longer description wins");

    // A shorter description never replaces the stored one.
    store
        .upsert_entity("CONCEPT_x", "First Name", "CONCEPT", "tiny")
        .unwrap();
    let desc: String = store
        .connection()
        .query_row("SELECT desc FROM entities WHERE node = ?1", [node.node], |row| row.get(0))
        .unwrap();
    assert_eq!(desc, "a longer description");

    // Exact-name terms exist and cascade with the entity.
    let terms: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM entity_name_terms WHERE node = ?1",
            [node.node],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terms, 2, "first name's tokens");
    store
        .connection()
        .execute("DELETE FROM entities WHERE node = ?1", [node.node])
        .unwrap();
    let terms: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM entity_name_terms WHERE node = ?1",
            [node.node],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terms, 0, "name terms cascade");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn glossary_exact_matches_are_complete_and_first() {
    let (path, dir) = db_path("native-glossary");
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    // Fillers share the common tokens of the exact match's name, so a plain
    // FTS top-k could rank the exact match out of the candidate window.
    for i in 0..300 {
        store
            .upsert_entity(
                &format!("CONCEPT_filler_{i}"),
                &format!("System Filler {i}"),
                "CONCEPT",
                "system filler unit",
            )
            .unwrap();
    }
    store
        .upsert_entity("CONCEPT_cache", "Cache", "CONCEPT", "a cache")
        .unwrap();
    store
        .upsert_entity("CONCEPT_memory", "Memory", "CONCEPT", "a memory")
        .unwrap();
    store
        .upsert_entity("CONCEPT_cache_memory", "Cache Memory", "CONCEPT", "fast memory")
        .unwrap();

    let glossary = store.glossary("The cache memory holds system data", 10).unwrap();
    assert_eq!(glossary.len(), 10, "budget respected");
    let ids: Vec<&str> = glossary.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(
        &ids[..3],
        &["CONCEPT_cache", "CONCEPT_cache_memory", "CONCEPT_memory"],
        "exact matches first, entity_key order"
    );
    assert_eq!(
        glossary.iter().filter(|e| e.id == "CONCEPT_cache_memory").count(),
        1,
        "no duplicates between exact and fill"
    );

    assert!(store.glossary("", 10).unwrap().is_empty());
    assert!(store.glossary("   ", 10).unwrap().is_empty());
    assert!(store.glossary("cache", 0).unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn glossary_skips_the_fill_when_every_token_is_common() {
    let (path, dir) = db_path("native-glossary-common");
    let store = KgStore::open(&path, OpenMode::ReadWrite).unwrap();
    // One entity makes the corpus non-empty; 2001 rows make "ubiquitous"
    // exceed GLOSSARY_MAX_DF (the entity_vocab df lookup is live).
    let mut stmt = store
        .connection()
        .prepare(
            "INSERT INTO entities(entity_key, entity_slug, name, type, desc, tokens)
             VALUES (?1, ?2, ?3, 'CONCEPT', '', 'ubiquitous filler')",
        )
        .unwrap();
    for i in 0..2001 {
        let key = format!("CONCEPT_f{i}");
        let slug = format!("f{i}");
        let name = format!("Filler {i}");
        stmt.execute([key.as_str(), slug.as_str(), name.as_str()]).unwrap();
    }
    // No exact match and no token rare enough to seed the FTS fill.
    assert!(store.glossary("ubiquitous text", 10).unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_prefix_applies_to_section_reads_only() {
    let (path, dir) = db_path("prefix");
    let kg = sample_graph();
    KgStore::write_full(&kg, &path).unwrap();

    let store = KgStore::open(&path, OpenMode::ReadOnly)
        .unwrap()
        .with_source_prefix("papers/ml");
    let info = store.section("S1").unwrap().unwrap();
    assert_eq!(info.source_doc, "papers/ml/docs/a.md");

    // stored_docs stays graph-root-relative for the staleness check
    let docs = store.stored_docs().unwrap();
    assert!(docs.contains_key("docs/a.md"));
    assert!(!docs.contains_key("papers/ml/docs/a.md"));

    // prefixing is idempotent
    let store = KgStore::open(&path, OpenMode::ReadOnly)
        .unwrap()
        .with_source_prefix("papers/ml");
    let info = store.section("S1").unwrap().unwrap();
    assert_eq!(info.source_doc, "papers/ml/docs/a.md");

    let _ = std::fs::remove_dir_all(&dir);
}
