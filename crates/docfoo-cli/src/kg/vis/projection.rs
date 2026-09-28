//! Graph projection for the visualizer — port of the desktop app's
//! `src-tauri/src/kg/graph_data.rs`.
//!
//! The `graph.sqlite` store is hydrated into the in-memory model and projected
//! into the frontend-friendly JSON shape: nodes as `[id, name, type, degree]`
//! tuples, edges as node-index pairs, sections as
//! `{title: {t: topic, e: [node idx]}}`, the measured noise floor, and the
//! store's `build_uid` as the layout/cache key. Results are cached per graph
//! file keyed by that uid, so repeated fetches cost one store open + hydrate
//! only when the graph actually changed.
//!
//! (The packed binary scene transport lives in the full desktop app; the CLI
//! viewer still consumes this JSON projection.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use docfoo_kg::graph::KnowledgeGraph;
use docfoo_kg::store::{KgStore, OpenMode};
use serde_json::{json, Value};

/// Description snippet ceiling for the tap-a-node popover (characters).
const DESC_SNIPPET_CHARS: usize = 400;

fn truncate_desc(desc: &str) -> String {
    let end = desc
        .char_indices()
        .nth(DESC_SNIPPET_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(desc.len());
    if end >= desc.len() {
        desc.to_string()
    } else {
        format!("{}…", &desc[..end])
    }
}

#[derive(Default)]
pub struct Projector {
    cache: Mutex<HashMap<PathBuf, (String, Value)>>,
}

impl Projector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Project `graph_path`, reusing the cached projection when the store's
    /// build uid is unchanged.
    pub fn project(&self, graph_path: &Path) -> std::result::Result<Value, String> {
        let store = KgStore::open(graph_path, OpenMode::ReadOnly)
            .map_err(|error| format!("could not open the knowledge graph: {error}"))?;
        let hash = store
            .build_uid()
            .map_err(|error| format!("could not read the knowledge graph: {error}"))?;

        if let Ok(cache) = self.cache.lock() {
            if let Some((cached_hash, cached)) = cache.get(graph_path) {
                if *cached_hash == hash {
                    return Ok(cached.clone());
                }
            }
        }

        let mut graph = KnowledgeGraph::default();
        store
            .load_into(&mut graph)
            .map_err(|error| format!("could not read the knowledge graph: {error}"))?;
        let projection = project_graph(&graph, &hash);

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(graph_path.to_path_buf(), (hash, projection.clone()));
        }
        Ok(projection)
    }
}

fn project_graph(graph: &KnowledgeGraph, hash: &str) -> Value {
    // Node order = BTreeMap iteration (id-sorted, deterministic).
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for relation in &graph.relations {
        if graph.entities.contains_key(&relation.source) {
            *degree.entry(relation.source.as_str()).or_insert(0) += 1;
        }
        if graph.entities.contains_key(&relation.target) {
            *degree.entry(relation.target.as_str()).or_insert(0) += 1;
        }
    }
    let mut index: HashMap<&str, usize> = HashMap::new();
    let nodes: Vec<Value> = graph
        .entities
        .iter()
        .enumerate()
        .map(|(i, (id, entity))| {
            index.insert(id.as_str(), i);
            json!([
                id,
                entity.name,
                entity.etype,
                degree.get(id.as_str()).copied().unwrap_or(0)
            ])
        })
        .collect();

    // Description snippets aligned with `nodes` for the tap-a-node popover.
    let descs: Vec<Value> = graph
        .entities
        .values()
        .map(|entity| json!(truncate_desc(&entity.desc)))
        .collect();

    // Relations with a missing endpoint are not drawable.
    let edges: Vec<Value> = graph
        .relations
        .iter()
        .filter_map(|relation| {
            let (Some(&source), Some(&target)) = (
                index.get(relation.source.as_str()),
                index.get(relation.target.as_str()),
            ) else {
                return None;
            };
            Some(json!([source, target]))
        })
        .collect();

    let sections: Value = graph
        .sections
        .iter()
        .map(|(title, info)| {
            let entities: Vec<usize> = info
                .entity_ids
                .iter()
                .filter_map(|id| index.get(id.as_str()).copied())
                .collect();
            (
                title.clone(),
                json!({ "t": info.topic.clone().unwrap_or_default(), "e": entities }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();

    json!({
        "hash": hash,
        "nodes": nodes,
        "edges": edges,
        "sections": sections,
        "noiseFloor": graph.noise_floor,
        "descs": descs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfoo_kg::graph::SectionInfo;

    fn sample() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::default();
        kg.add_entity(
            "DEVICE_a",
            "A",
            "DEVICE",
            "x",
            "[DOC] Apple facts",
            Some("doc/content.md"),
        );
        kg.add_entity(
            "CONCEPT_b",
            "B",
            "CONCEPT",
            "y",
            "[DOC] Apple facts",
            Some("doc/content.md"),
        );
        kg.add_entity("CONCEPT_iso", "Iso", "CONCEPT", "z", "[DOC] Apple facts", None);
        kg.add_relation(
            "DEVICE_a",
            "CONCEPT_b",
            "USES",
            "[DOC] Apple facts",
            Some("doc/content.md"),
        );
        kg.add_relation(
            "CONCEPT_b",
            "DEVICE_a",
            "ENABLES",
            "[DOC] Apple facts",
            Some("doc/content.md"),
        );
        kg.sections.insert(
            "[DOC] Apple facts".into(),
            SectionInfo {
                topic: Some("[CO] Chapter 3".into()),
                entity_ids: vec![
                    "DEVICE_a".into(),
                    "CONCEPT_b".into(),
                    "GONE_missing".into(),
                ],
                text: "t".into(),
                source_doc: "doc/content.md".into(),
                ..Default::default()
            },
        );
        kg.noise_floor = Some(0.031);
        kg
    }

    fn write_store(dir: &Path) -> PathBuf {
        let path = dir.join("graph.sqlite");
        KgStore::write_full(&sample(), &path).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "docfoo-vis-proj-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn projection_matches_the_frontend_shape_and_counts() {
        let dir = temp_dir("shape");
        let path = write_store(&dir);
        let expected_uid = KgStore::open(&path, OpenMode::ReadOnly)
            .unwrap()
            .build_uid()
            .unwrap();

        let projector = Projector::new();
        let projection = projector.project(&path).unwrap();
        assert_eq!(projection["hash"], expected_uid);
        assert_eq!(
            projection["nodes"],
            json!([
                ["CONCEPT_b", "B", "CONCEPT", 2],
                ["CONCEPT_iso", "Iso", "CONCEPT", 0],
                ["DEVICE_a", "A", "DEVICE", 2],
            ])
        );
        assert_eq!(projection["edges"], json!([[2, 0], [0, 2]]));
        assert_eq!(
            projection["sections"]["[DOC] Apple facts"],
            json!({ "t": "[CO] Chapter 3", "e": [2, 0] })
        );
        assert_eq!(projection["noiseFloor"], json!(0.031));

        // The cache survives a re-projection: same uid, same value.
        let again = projector.project(&path).unwrap();
        assert_eq!(again["hash"], projection["hash"]);
        assert_eq!(again["nodes"], projection["nodes"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn descriptions_are_truncated_for_the_popover() {
        let long = "x".repeat(500);
        let truncated = truncate_desc(&long);
        assert_eq!(truncated.chars().count(), DESC_SNIPPET_CHARS + 1);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate_desc("short"), "short");
    }
}
