//! End-to-end tests for `docfoo kg --vis`.
//!
//! Each test starts the real binary on an ephemeral loopback port with the
//! fake sidecar, then exercises the HTTP API the browser page uses: state,
//! graph projection, resource assets, the SSE query stream and traversal
//! refusal. The process is killed when the guard drops.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn write_fixture_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("resources/doc/assets")).unwrap();
    std::fs::write(
        root.join("resources/doc/content.md"),
        "Apples are a tropical fruit.\nMangoes are also tropical fruit.\n",
    )
    .unwrap();
    std::fs::write(root.join("resources/doc/assets/f.png"), b"fake-png-bytes").unwrap();
    std::fs::write(root.join("secret.txt"), b"do not serve").unwrap();

    std::fs::create_dir_all(root.join("graphs/papers/ml")).unwrap();
    let graph = json!({
        "entities": {
            "CONCEPT_apple": {
                "id": "CONCEPT_apple",
                "name": "Apple",
                "type": "CONCEPT",
                "desc": "A tropical fruit.",
                "sections": ["[DOC] Apple facts"],
                "source_doc": ["doc/content.md"]
            },
            "CONCEPT_mango": {
                "id": "CONCEPT_mango",
                "name": "Mango",
                "type": "CONCEPT",
                "desc": "Another tropical fruit.",
                "sections": ["[DOC] Apple facts"],
                "source_doc": ["doc/content.md"]
            }
        },
        "relations": [
            {
                "source": "CONCEPT_apple",
                "target": "CONCEPT_mango",
                "rel": "RELATED_TO",
                "section": "[DOC] Apple facts",
                "source_doc": "doc/content.md"
            }
        ],
        "sections": {
            "[DOC] Apple facts": {
                "topic": null,
                "entity_ids": ["CONCEPT_apple", "CONCEPT_mango"],
                "text": "Apples are a tropical fruit. Mangoes are also tropical fruit.",
                "source_doc": "doc/content.md",
                "start_line": 1,
                "end_line": 2,
                "content_hash": "",
                "retry_pending": false
            }
        },
        "topics": {},
        "noise_floor": null,
        "source_hashes": {}
    });
    std::fs::write(
        root.join("graphs/top-level.json"),
        serde_json::to_string_pretty(&graph).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("graphs/papers/ml/graph.json"),
        serde_json::to_string_pretty(&graph).unwrap(),
    )
    .unwrap();
}

struct VisProcess {
    child: Child,
    base: String,
}

impl Drop for VisProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// The returned guard kills and waits on the child in `Drop`, so the spawn
// inside the URL-poll loop cannot leak a process (clippy cannot see through
// the custom Drop impl).
#[allow(clippy::zombie_processes)]
fn start_vis(root: &Path, scope: &str) -> VisProcess {
    let log_path = root.join("vis-stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_docfoo"));
    command
        .env("DOCFOO_SIDECAR_BIN", env!("CARGO_BIN_EXE_fake-sidecar"))
        .args([
            "kg",
            "--vis",
            "--no-open",
            "--port",
            "0",
            "--model",
            "fake-provider/fake-model",
            "--scope",
            scope,
            "--workspace",
        ])
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    let child = command.spawn().expect("spawn docfoo --vis");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(&log_path) {
            if let Some(start) = text.find("http://127.0.0.1:") {
                let url = text[start..]
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .trim_end_matches('.')
                    .to_string();
                return VisProcess { child, base: url };
            }
        }
        if Instant::now() > deadline {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!("vis server did not print a URL within 20s; stderr:\n{log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .new_agent()
}

fn get_json(base: &str, path: &str) -> (u16, Value) {
    let response = agent().get(format!("{base}{path}")).call().expect("GET");
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().expect("body");
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value)
}

fn post_json(base: &str, path: &str, body: &Value) -> (u16, Value) {
    let response = agent()
        .post(format!("{base}{path}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .expect("POST");
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().expect("body");
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value)
}

/// Poll the event cursor until the `done` frame, returning every parsed frame.
fn poll_until_done(base: &str) -> Vec<Value> {
    let mut frames = Vec::new();
    let mut cursor = 0u64;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let (status, batch) = get_json(base, &format!("/api/events?since={cursor}"));
        assert_eq!(status, 200, "events: {batch}");
        for event in batch["events"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = event["id"].as_u64() {
                cursor = cursor.max(id);
            }
            let data = event["data"].clone();
            let done = data["type"] == "done";
            frames.push(data);
            if done {
                return frames;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("query did not finish within 30s; frames so far: {frames:?}");
}

#[test]
fn vis_serves_state_graph_and_assets() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    let (status, state) = get_json(&vis.base, "/api/state");
    assert_eq!(status, 200, "state: {state}");
    assert_eq!(state["scope"], "");
    assert_eq!(state["graphExists"], true);
    assert_eq!(state["model"], "fake-provider/fake-model");
    assert!(state["built"]
        .as_array()
        .unwrap()
        .iter()
        .any(|scope| scope == "papers/ml"));

    let (status, graph) = get_json(&vis.base, "/api/graph");
    assert_eq!(status, 200, "graph: {graph}");
    assert!(graph["hash"].is_string());
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"].as_array().unwrap().len(), 1);
    assert!(graph["sections"]["[DOC] Apple facts"]["e"].is_array());

    // Scoped graph resolves too.
    let (status, scoped) = get_json(&vis.base, "/api/graph?scope=papers%2Fml");
    assert_eq!(status, 200, "scoped: {scoped}");
    assert_eq!(scoped["nodes"].as_array().unwrap().len(), 2);

    // Missing scope is a 404 with a hint, and nothing crashes.
    let (status, missing) = get_json(&vis.base, "/api/graph?scope=nope%2Fmissing");
    assert_eq!(status, 404);
    assert!(missing["error"].as_str().unwrap().contains("kg --index"));

    // Figure bytes are served from resources/ only.
    let response = agent()
        .get(format!("{}/api/asset?path=doc%2Fassets%2Ff.png", vis.base))
        .call()
        .expect("asset");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap().to_str().unwrap(),
        "image/png"
    );
    let bytes = response.into_body().read_to_string().unwrap();
    assert_eq!(bytes, "fake-png-bytes");

    let (status, _) = get_json(&vis.base, "/api/asset?path=..%2Fsecret.txt");
    assert_eq!(status, 404, "path traversal must not serve files");
}

#[test]
fn vis_streams_a_query_with_stage_frames_and_a_cited_answer() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    let (status, accepted) = post_json(
        &vis.base,
        "/api/query",
        &json!({ "query": "What are apples?", "scope": "" }),
    );
    assert_eq!(status, 202, "query accepted: {accepted}");

    let frames = poll_until_done(&vis.base);
    assert!(!frames.is_empty(), "expected frames");
    assert_eq!(frames[0]["type"], "start");

    let steps: Vec<&str> = frames
        .iter()
        .filter(|frame| frame["type"] == "stage")
        .filter_map(|frame| frame["step"].as_str())
        .collect();
    assert!(steps.contains(&"seeds"), "stage steps: {steps:?}");
    assert!(steps.contains(&"traversal"), "stage steps: {steps:?}");
    // Pass and seq ride every stage frame.
    let stage = frames.iter().find(|frame| frame["type"] == "stage").unwrap();
    assert_eq!(stage["pass"], 1);
    assert!(stage["seq"].as_u64().unwrap() >= 1);

    assert!(
        frames.iter().any(|frame| frame["type"] == "delta"),
        "expected synthesis deltas"
    );

    let done = frames.last().unwrap();
    assert_eq!(done["ok"], true);
    let answer = done["answer_markdown"].as_str().unwrap();
    assert!(
        answer.contains("[doc/content.md:"),
        "citation tag was not expanded: {answer}"
    );
    assert!(!done["sources"].as_array().unwrap().is_empty());
    assert!(!done["citations"].as_array().unwrap().is_empty());
}

#[test]
fn a_new_query_never_replays_the_previous_turns_events() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    // Turn 1 runs to completion and its frames stay in the log.
    let (status, _) = post_json(&vis.base, "/api/query", &json!({ "query": "first", "scope": "" }));
    assert_eq!(status, 202);
    poll_until_done(&vis.base);
    let (_, after_first) = get_json(&vis.base, "/api/events?since=0");
    let first_max = after_first["cursor"].as_u64().expect("cursor after turn 1");

    // `begin_query` clears the log before answering 202, so a client that
    // polls from cursor 0 after the POST can only see the new turn. This is
    // the server invariant behind the page's two-turn regression (`startQuery`
    // polls only after the POST resolves).
    let (status, _) = post_json(&vis.base, "/api/query", &json!({ "query": "second", "scope": "" }));
    assert_eq!(status, 202);
    let (_, after_second) = get_json(&vis.base, "/api/events?since=0");
    let events = after_second["events"].as_array().expect("events");
    assert!(!events.is_empty(), "turn 2 must emit at least the start frame");
    assert_eq!(events[0]["event"], "start");
    for event in events {
        let id = event["id"].as_u64().expect("event id");
        assert!(id > first_max, "stale turn-1 event {id} leaked into turn 2 (max {first_max})");
    }

    // Let turn 2 finish so the guard's killed process is not mid-query.
    poll_until_done(&vis.base);
}

#[test]
fn vis_cancel_is_a_no_op_when_idle() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    let (status, value) = post_json(&vis.base, "/api/cancel", &json!({}));
    assert_eq!(status, 200);
    assert_eq!(value["cancelled"], true);
}
