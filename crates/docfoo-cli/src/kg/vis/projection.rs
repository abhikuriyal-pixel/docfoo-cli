//! Graph projection for the visualizer — port of the desktop app's
//! `src-tauri/src/kg/graph_data.rs`.
//!
//! `graph.json` is projected into a compact, frontend-friendly shape: nodes
//! as `[id, name, type, degree]` tuples, edges as node-index pairs, sections
//! as `{title: {t: topic, e: [node idx]}}`, the measured noise floor, and an
//! fnv1a-64 hash of the raw file used as the layout/cache key. Results are
//! cached per graph file keyed by that hash, so repeated fetches cost one
//! disk read + parse only when the graph actually changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use docfoo_kg::graph::KnowledgeGraph;
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

/// fnv1a-64 over the raw file bytes — stable across reads of the same file,
/// cheap, and distinct per content.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Default)]
pub struct Projector {
    cache: Mutex<HashMap<PathBuf, (u64, Value)>>,
}

impl Projector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Project `graph_path`, reusing the cached projection when the file's
    /// content hash is unchanged.
    pub fn project(&self, graph_path: &Path) -> std::result::Result<Value, String> {
        let bytes = std::fs::read(graph_path)
            .map_err(|error| format!("could not read the graph file: {error}"))?;
        let hash = fnv1a64(&bytes);

        if let Ok(cache) = self.cache.lock() {
            if let Some((cached_hash, cached)) = cache.get(graph_path) {
                if *cached_hash == hash {
                    return Ok(cached.clone());
                }
            }
        }

        let graph: KnowledgeGraph = serde_json::from_slice(&bytes)
            .map_err(|error| format!("graph file parse failed: {error}"))?;
        let projection = project_graph(&graph, hash);

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(graph_path.to_path_buf(), (hash, projection.clone()));
        }
        Ok(projection)
    }
}

fn project_graph(graph: &KnowledgeGraph, hash: u64) -> Value {
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
        "hash": format!("{hash:016x}"),
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

    fn write_graph(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("graph.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn projection_matches_the_desktop_shape_and_counts() {
        let dir = std::env::temp_dir().join(format!("docfoo-vis-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_graph(
            &dir,
            r#"{
            "entities": {
                "DEVICE_a": { "id": "DEVICE_a", "name": "A", "type": "DEVICE", "desc": "x", "sections": ["S1"], "source_doc": [] },
                "CONCEPT_b": { "id": "CONCEPT_b", "name": "B", "type": "CONCEPT", "desc": "y", "sections": ["S1"], "source_doc": [] },
                "CONCEPT_iso": { "id": "CONCEPT_iso", "name": "Iso", "type": "CONCEPT", "desc": "z", "sections": [], "source_doc": [] }
            },
            "relations": [
                { "source": "DEVICE_a", "target": "CONCEPT_b", "rel": "USES", "section": "S1" },
                { "source": "CONCEPT_b", "target": "DEVICE_a", "rel": "ENABLES", "section": "S1" },
                { "source": "DEVICE_a", "target": "GONE_missing", "rel": "USES", "section": "S1" }
            ],
            "sections": {
                "S1": { "topic": "[CO] Chapter 3", "entity_ids": ["DEVICE_a", "CONCEPT_b", "GONE_missing"], "text": "t", "source_doc": "d.md", "content_hash": "" }
            },
            "topics": {},
            "noise_floor": 0.031
        }"#,
        );

        let projector = Projector::new();
        let projection = projector.project(&path).unwrap();
        assert_eq!(
            projection["hash"],
            format!("{:016x}", fnv1a64(&std::fs::read(&path).unwrap()))
        );
        assert_eq!(
            projection["nodes"],
            json!([
                ["CONCEPT_b", "B", "CONCEPT", 2],
                ["CONCEPT_iso", "Iso", "CONCEPT", 0],
                ["DEVICE_a", "A", "DEVICE", 3],
            ])
        );
        assert_eq!(projection["edges"], json!([[2, 0], [0, 2]]));
        assert_eq!(
            projection["sections"]["S1"],
            json!({ "t": "[CO] Chapter 3", "e": [2, 0] })
        );
        assert_eq!(projection["noiseFloor"], json!(0.031));

        // The cache survives a changed file: same hash, same value.
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
