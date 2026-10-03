//! Bounded, query-local replay data. No web server, graph hydration or upstream
//! crate changes: all reads use the query's existing SQLite read transaction.
use std::collections::BTreeSet;

use docfoo_kg::query::{stage::STAGE_CAP, StageEvent, Trace};
use docfoo_kg::store::{rusqlite::OptionalExtension, KgStore, StoreResult};
use serde_json::{json, Value};

pub const CAPABILITY: &str = "kg.query.snapshot.v1";
pub const SCHEMA: &str = "docfoo.kg.replay/1";
pub const NODE_CAP: usize = 1000;
pub const EDGE_CAP: usize = 4000;
pub const FRAME_CAP: usize = 500;
pub const BYTE_CAP: usize = 4 * 1024 * 1024;
pub const TEXT_CAP: usize = 12000;
/// Leave room for the consumer's UUID/artifact envelope; preserve identifiers
/// rather than truncating them differently from their recorded stage frames.
pub fn payload_fits(value: &Value) -> bool {
    fn strings_fit(value: &Value) -> bool {
        match value {
            Value::String(s) => s.encode_utf16().count() <= TEXT_CAP,
            Value::Array(items) => items.iter().all(strings_fit),
            Value::Object(items) => items.values().all(strings_fit),
            _ => true,
        }
    }
    strings_fit(value) && value.to_string().len() <= BYTE_CAP - 512
}

#[derive(Default)]
pub struct Capture {
    pub frames: Vec<Value>,
    pub truncated: bool,
    seq: usize,
}
impl Capture {
    pub fn push(&mut self, pass: u8, event: &StageEvent) {
        self.seq += 1;
        if self.frames.len() < FRAME_CAP {
            let mut frame = serde_json::to_value(event).expect("stage events serialize");
            frame["type"] = json!("stage");
            frame["pass"] = json!(pass);
            frame["seq"] = json!(self.seq);
            self.frames.push(frame);
        } else if !self.truncated {
            self.truncated = true;
            self.frames
                .push(json!({"type":"stage", "pass":pass, "seq":self.seq,
                "step":"truncated", "data":{"cap":FRAME_CAP}}));
        }
    }
}

fn strings(value: &Value) -> impl Iterator<Item = &str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}
fn add_ids(
    store: &KgStore,
    selected: &mut BTreeSet<i64>,
    ids: impl IntoIterator<Item = String>,
) -> StoreResult<()> {
    let mut stmt = store
        .connection()
        .prepare("SELECT node FROM entities WHERE entity_key=?1")?;
    for id in ids {
        if selected.len() == NODE_CAP {
            break;
        }
        if let Some(node) = stmt.query_row([id], |row| row.get(0)).optional()? {
            selected.insert(node);
        }
    }
    Ok(())
}

pub fn build(store: &KgStore, scope: &str, trace: &Trace, capture: &Capture) -> StoreResult<Value> {
    let conn = store.connection();
    let counts = store.counts()?;
    let complete = counts.entities <= NODE_CAP && counts.relations <= EDGE_CAP;
    let mut selected = BTreeSet::new();
    // Section titles are opaque graph keys, not filesystem paths. Include only
    // recorded evidence/guide sections, and bound their exported membership.
    let titles: BTreeSet<String> = trace
        .evidence_sections
        .iter()
        .map(|s| s.section.clone())
        .chain(
            trace
                .guide_picks
                .iter()
                .flat_map(|g| g.sections.iter().cloned()),
        )
        .take(STAGE_CAP)
        .collect();
    if complete {
        let mut stmt = conn.prepare("SELECT node FROM entities ORDER BY node LIMIT ?1")?;
        selected = stmt
            .query_map([NODE_CAP as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
    } else {
        add_ids(
            store,
            &mut selected,
            trace
                .seeds
                .iter()
                .map(|s| s.id.clone())
                .chain(trace.anchors.iter().map(|s| s.id.clone()))
                .chain(trace.routing.lead_entities.iter().cloned()),
        )?;
        // Keep authentic event ids (especially hop additions) ahead of context.
        for frame in &capture.frames {
            let data = &frame["data"];
            let mut ids = Vec::new();
            match frame["step"].as_str().unwrap_or("") {
                "seeds" => {
                    for key in ["ids", "anchors", "leadIds"] {
                        ids.extend(strings(&data[key]).map(str::to_owned));
                    }
                }
                "hop" => ids.extend(strings(&data["added"]).map(str::to_owned)),
                "descent" => ids.extend(strings(&data["pickedOrder"]).map(str::to_owned)),
                "expansion" => ids.extend(strings(&data["entities"]).map(str::to_owned)),
                "bm25" => ids.extend(
                    data["entityHits"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|h| h["id"].as_str())
                        .map(str::to_owned),
                ),
                _ => {}
            }
            add_ids(store, &mut selected, ids)?;
        }
        for title in &titles {
            if selected.len() == NODE_CAP {
                break;
            }
            let taken = json!(selected).to_string();
            let mut stmt = conn.prepare("SELECT e.node FROM section_entities se JOIN sections s ON s.row=se.section_row JOIN entities e ON e.entity_key=se.entity_key WHERE s.title=?1 AND e.node NOT IN (SELECT value FROM json_each(?3)) ORDER BY se.ord LIMIT ?2")?;
            let rows = stmt
                .query_map((title, (NODE_CAP - selected.len()) as i64, &taken), |r| {
                    r.get::<_, i64>(0)
                })?;
            for row in rows {
                selected.insert(row?);
            }
        }
        // A bounded one-hop context walk; never materialize a hub's full adjacency.
        let focus: Vec<i64> = selected.iter().copied().collect();
        for node in focus {
            if selected.len() == NODE_CAP {
                break;
            }
            let taken = json!(selected).to_string();
            let mut stmt = conn.prepare("SELECT node FROM (SELECT dst AS node FROM relations WHERE src=?1 UNION SELECT src AS node FROM relations WHERE dst=?1) WHERE node NOT IN (SELECT value FROM json_each(?3)) ORDER BY node LIMIT ?2")?;
            let rows = stmt.query_map((node, (NODE_CAP - selected.len()) as i64, &taken), |r| {
                r.get::<_, i64>(0)
            })?;
            for row in rows {
                selected.insert(row?);
            }
        }
        if selected.len() < NODE_CAP {
            // Context for disconnected collections / queries without lexical seeds.
            let taken = json!(selected).to_string();
            let mut stmt = conn.prepare("SELECT node FROM (SELECT src AS node FROM relations UNION ALL SELECT dst AS node FROM relations) WHERE node NOT IN (SELECT value FROM json_each(?2)) GROUP BY node ORDER BY COUNT(*) DESC, node LIMIT ?1")?;
            for row in stmt.query_map((12.min(NODE_CAP - selected.len()) as i64, &taken), |r| {
                r.get::<_, i64>(0)
            })? {
                selected.insert(row?);
            }
            // Isolated nodes are genuine entities too.
            let taken = json!(selected).to_string();
            let mut stmt = conn.prepare("SELECT node FROM entities WHERE node NOT IN (SELECT value FROM json_each(?2)) ORDER BY node LIMIT ?1")?;
            for row in stmt.query_map(((NODE_CAP - selected.len()) as i64, &taken), |r| {
                r.get::<_, i64>(0)
            })? {
                selected.insert(row?);
            }
        }
    }
    let selected_json = json!(selected).to_string();
    let mut stmt = conn.prepare("SELECT e.entity_key,e.name,e.type,(SELECT COUNT(*) FROM relations WHERE src=e.node)+(SELECT COUNT(*) FROM relations WHERE dst=e.node) FROM entities e WHERE node IN (SELECT value FROM json_each(?1)) ORDER BY entity_key")?;
    let nodes: Vec<Value> = stmt.query_map([&selected_json], |r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?.chars().take(200).collect::<String>(),"type":r.get::<_,String>(2)?.chars().take(80).collect::<String>(),"degree":r.get::<_,i64>(3)?})))?.collect::<Result<_, _>>()?;
    let mut stmt = conn.prepare("SELECT s.entity_key,t.entity_key,r.rel FROM relations r JOIN entities s ON s.node=r.src JOIN entities t ON t.node=r.dst WHERE r.src IN (SELECT value FROM json_each(?1)) AND r.dst IN (SELECT value FROM json_each(?1)) ORDER BY r.rid LIMIT ?2")?;
    let edges: Vec<Value> = stmt.query_map((&selected_json, EDGE_CAP as i64), |r| Ok(json!({"source":r.get::<_,String>(0)?,"target":r.get::<_,String>(1)?,"rel":r.get::<_,String>(2)?.chars().take(200).collect::<String>()})))?.collect::<Result<_, _>>()?;
    let mut sections = Vec::new();
    for title in titles {
        let mut stmt = conn.prepare("SELECT e.entity_key FROM section_entities se JOIN sections s ON s.row=se.section_row JOIN entities e ON e.entity_key=se.entity_key WHERE s.title=?1 AND e.node IN (SELECT value FROM json_each(?2)) ORDER BY se.ord LIMIT ?3")?;
        let ids: Vec<String> = stmt
            .query_map((&title, &selected_json, STAGE_CAP as i64), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        sections.push(json!({"title":title,"ids":ids}));
    }
    Ok(
        json!({"schema":SCHEMA,"scope":scope,"buildUid":store.build_uid()?,
        "totalNodes":counts.entities,"totalEdges":counts.relations,
        "omittedNodes":counts.entities.saturating_sub(nodes.len()),"omittedEdges":counts.relations.saturating_sub(edges.len()),
        "nodes":nodes,"edges":edges,"sections":sections,"frames":capture.frames,
        "traceTruncated":capture.truncated,"eventListCap":STAGE_CAP}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfoo_kg::graph::{KnowledgeGraph, SectionInfo};
    use docfoo_kg::store::OpenMode;

    #[test]
    fn capture_retains_real_order_and_marks_overflow_once() {
        let mut capture = Capture::default();
        for _ in 0..FRAME_CAP + 10 {
            capture.push(1, &StageEvent::Depth { depth: "deep" });
        }
        assert_eq!(capture.frames.len(), FRAME_CAP + 1);
        assert_eq!(capture.frames[0]["seq"], 1);
        assert_eq!(capture.frames[FRAME_CAP]["step"], "truncated");
        assert!(capture.truncated);
    }

    #[test]
    fn snapshot_stays_on_the_queried_version_during_wal_updates() {
        let temp = tempfile::tempdir().unwrap();
        let mut graph = KnowledgeGraph::default();
        graph
            .sections
            .insert("Evidence".into(), SectionInfo::default());
        graph.add_entity("A", "Original name", "CONCEPT", "", "Evidence", None);
        let file = temp.path().join("graph.sqlite");
        KgStore::write_full(&graph, &file).unwrap();
        let writer = KgStore::open(&file, OpenMode::ReadWrite).unwrap();
        let reader = KgStore::open(&file, OpenMode::ReadOnly).unwrap();
        let transaction = reader.connection().unchecked_transaction().unwrap();
        let uid = reader.build_uid().unwrap();
        writer
            .connection()
            .execute("UPDATE entities SET name='Rebuilt name'", [])
            .unwrap();
        writer
            .connection()
            .execute(
                "UPDATE meta SET value='new-build' WHERE key='build_uid'",
                [],
            )
            .unwrap();
        let value = build(&reader, "", &empty_trace(), &Capture::default()).unwrap();
        assert_eq!(value["nodes"][0]["name"], "Original name");
        assert_eq!(value["buildUid"], uid);
        drop(transaction);
        assert_eq!(reader.build_uid().unwrap(), "new-build");
    }

    #[test]
    fn bounded_selection_is_deterministic_and_preserves_hop_ids() {
        let temp = tempfile::tempdir().unwrap();
        let mut graph = KnowledgeGraph::default();
        graph.sections.insert(
            "Evidence".into(),
            SectionInfo {
                entity_ids: vec!["N1199".into()],
                ..Default::default()
            },
        );
        for i in 0..1200 {
            graph.add_entity(
                &format!("N{i:04}"),
                &format!("Node {i}"),
                "CONCEPT",
                "",
                "Evidence",
                None,
            );
        }
        for i in 0..1199 {
            for hop in 1..=6 {
                graph.add_relation(
                    &format!("N{i:04}"),
                    &format!("N{:04}", (i + hop) % 1200),
                    "NEXT",
                    "Evidence",
                    None,
                );
            }
        }
        let file = temp.path().join("graph.sqlite");
        KgStore::write_full(&graph, &file).unwrap();
        let store = KgStore::open(&file, OpenMode::ReadOnly).unwrap();
        let trace = empty_trace();
        let mut capture = Capture::default();
        capture.push(
            1,
            &StageEvent::Hop {
                hop: 1,
                added: vec!["N1199".into()],
                visited_so_far: 1,
            },
        );
        let a = build(&store, "papers/ml", &trace, &capture).unwrap();
        let b = build(&store, "papers/ml", &trace, &capture).unwrap();
        assert_eq!(a, b);
        assert_eq!(a["scope"], "papers/ml");
        assert_eq!(a["nodes"].as_array().unwrap().len(), NODE_CAP);
        assert!(a["edges"].as_array().unwrap().len() <= EDGE_CAP);
        assert!(a["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == "N1199"));
        let ids: BTreeSet<_> = a["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        for edge in a["edges"].as_array().unwrap() {
            assert!(
                ids.contains(edge["source"].as_str().unwrap())
                    && ids.contains(edge["target"].as_str().unwrap())
            );
        }
        assert!(a["omittedNodes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn text_and_artifact_envelope_budgets_are_shared_with_consumer() {
        assert!(payload_fits(&json!({"title":"x".repeat(TEXT_CAP)})));
        assert!(!payload_fits(&json!({"title":"x".repeat(TEXT_CAP+1)})));
        assert!(!payload_fits(&json!({"title":"🦉".repeat(TEXT_CAP)})));
        assert!(!payload_fits(&json!(vec!["x".repeat(12000); 400])));
    }

    fn empty_trace() -> Trace {
        Trace {
            query: String::new(),
            depth: "simple",
            seeds: vec![],
            anchors: vec![],
            topic_votes: vec![],
            guides: vec![],
            guide_picks: vec![],
            visited_count: 0,
            hop_depth: 0,
            evidence_sections: vec![],
            budget_dropped: 0,
            triples_used: 0,
            routing: docfoo_kg::query::RoutingInfo {
                used: false,
                gate: docfoo_kg::query::GateInfo {
                    s1: 0.0,
                    floor: None,
                    ratio: None,
                    n: 0,
                    rule: String::new(),
                },
                candidates: 0,
                picks: vec![],
                lead_entities: vec![],
                sections: vec![],
                fallback: None,
                escalated: false,
                terms: vec![],
                model: String::new(),
                secs: 0.0,
            },
            timings: vec![],
            total_seconds: 0.0,
        }
    }
}
