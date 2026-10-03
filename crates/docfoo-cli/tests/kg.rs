//! End-to-end KG query against a fixture graph using the fake sidecar.
//!
//! This is the deterministic counterpart to a live query: the fake answers the
//! synthesis call with `[S1]`, so the test covers graph load → retrieval →
//! evidence delivery → tag expansion → citation parsing → JSON envelope.

use std::path::Path;
use std::process::Command;

use docfoo_kg::graph::{KnowledgeGraph, SectionInfo};
use docfoo_kg::store::KgStore;

fn docfoo_with_fake_sidecar() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_docfoo"));
    command.env("DOCFOO_SIDECAR_BIN", env!("CARGO_BIN_EXE_fake-sidecar"));
    command
}

fn fixture_graph() -> KnowledgeGraph {
    let mut graph = KnowledgeGraph::default();
    graph.add_entity(
        "CONCEPT_apple",
        "Apple",
        "CONCEPT",
        "A tropical fruit.",
        "[DOC] Apple facts",
        Some("doc/content.md"),
    );
    graph.add_relation(
        "CONCEPT_apple",
        "CONCEPT_apple",
        "PART_OF",
        "[DOC] Apple facts",
        Some("doc/content.md"),
    );
    graph.sections.insert(
        "[DOC] Apple facts".into(),
        SectionInfo {
            topic: None,
            entity_ids: vec!["CONCEPT_apple".into()],
            text: "Apples are a tropical fruit. Mangoes are also tropical fruit.".into(),
            source_doc: "doc/content.md".into(),
            start_line: 1,
            end_line: 2,
            ..Default::default()
        },
    );
    graph
}

fn write_fixture_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("resources/doc/assets")).unwrap();
    std::fs::write(
        root.join("resources/doc/content.md"),
        "Apples are a tropical fruit.\nMangoes are also tropical fruit.\n",
    )
    .unwrap();
    let graph = fixture_graph();
    std::fs::create_dir_all(root.join("graphs/papers/ml")).unwrap();
    KgStore::write_full(&graph, &root.join("graphs/top-level.sqlite")).unwrap();
    KgStore::write_full(&graph, &root.join("graphs/papers/ml/graph.sqlite")).unwrap();
}

#[test]
fn kg_index_builds_a_sqlite_store_and_status_reports_it() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("resources/doc")).unwrap();
    std::fs::write(
        temp.path().join("resources/doc/content.md"),
        "## Apple facts\n\nApples are a tropical fruit grown in orchards around the world.\n\n\
         Mangoes are also tropical fruit, and both appear in many recipes that pair\n\n\
         their sweetness with citrus and spice for balance. Orchards harvest apples\n\n\
         in autumn, while mangoes ripen through the warm summer months.\n",
    )
    .unwrap();

    let output = docfoo_with_fake_sidecar()
        .args([
            "kg",
            "--index",
            "--model",
            "fake-provider/fake-model",
            "--json",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["command"], "kg.index");
    let graph_path = value["data"]["graphPath"].as_str().expect("graphPath");
    assert!(
        std::path::Path::new(graph_path).ends_with("graphs/top-level.sqlite"),
        "graphPath: {graph_path}"
    );
    assert!(temp.path().join("graphs/top-level.sqlite").is_file());
    assert!(
        !temp.path().join("graphs/top-level.json").exists(),
        "the retired JSON graph must not be written"
    );

    let output = docfoo_with_fake_sidecar()
        .args(["kg", "--status", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["command"], "kg.status");
    assert_eq!(value["data"]["exists"], true);
    assert!(value["data"]["graphPath"]
        .as_str()
        .map(|path| std::path::Path::new(path).ends_with("graphs/top-level.sqlite"))
        .unwrap_or(false));
    assert_eq!(value["data"]["built"], serde_json::json!([""]));
}

#[test]
fn kg_query_qualifies_scoped_graph_sources() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let output = docfoo_with_fake_sidecar()
        .args([
            "kg",
            "--query",
            "What are apples?",
            "--scope",
            "papers/ml",
            "--model",
            "fake-provider/fake-model",
            "--json",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    let data = &value["data"];

    // The store holds provenance relative to the graph root; the query must
    // surface it workspace-relative (papers/ml/…) for citations and sources.
    let answer = data["answer_markdown"].as_str().unwrap();
    assert!(
        answer.contains("[papers/ml/doc/content.md:"),
        "scoped citation was not qualified: {answer}"
    );
    let sources = data["sources"].as_array().unwrap();
    assert_eq!(sources[0]["doc"], "papers/ml/doc/content.md");
}

#[test]
fn kg_query_expands_citations_and_reports_sources() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let output = docfoo_with_fake_sidecar()
        .args([
            "kg",
            "--query",
            "What are apples?",
            "--model",
            "fake-provider/fake-model",
            "--json",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    let data = &value["data"];

    assert!(
        data.get("replay").is_none(),
        "ordinary output must stay unchanged"
    );
    let answer = data["answer_markdown"].as_str().unwrap();
    assert!(
        answer.contains("[doc/content.md:"),
        "citation tag was not expanded: {answer}"
    );
    assert_eq!(value["command"], "kg.query");

    let citations = data["citations"].as_array().unwrap();
    assert!(!citations.is_empty(), "expected parsed citations");
    assert_eq!(citations[0]["file"], "doc/content.md");
    assert_eq!(citations[0]["line_start"], 1);
    assert_eq!(citations[0]["line_end"], 2);

    let sources = data["sources"].as_array().unwrap();
    assert!(!sources.is_empty(), "expected evidence sources");
    assert_eq!(sources[0]["doc"], "doc/content.md");
}

#[test]
fn kg_query_slack_output_has_sentinel_and_sources() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let output = docfoo_with_fake_sidecar()
        .args([
            "kg",
            "--query",
            "What are apples?",
            "--model",
            "fake-provider/fake-model",
            "--format",
            "slack",
            "--hermes-final",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("[[hermes:final]]\n"), "stdout: {stdout}");
    assert!(stdout.contains("Sources:"), "stdout: {stdout}");
    assert!(stdout.contains("• doc/content.md:1-2"), "stdout: {stdout}");
}

#[test]
fn kg_query_save_persists_a_kg_chat() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let output = docfoo_with_fake_sidecar()
        .args([
            "kg",
            "--query",
            "What is an apple?",
            "--model",
            "fake-provider/fake-model",
            "--save",
            "--json",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    let saved = value["data"]["saved"].as_str().expect("saved chat id");
    assert!(saved.starts_with("kg-"));
    let chat_dir = temp.path().join("kg-chats").join(saved);
    assert!(chat_dir.join("meta.json").is_file());
    assert!(chat_dir.join("kg-history.json").is_file());
    assert!(temp.path().join(".agent/current-kg-chat.json").is_file());
}

#[test]
fn kg_snapshot_exports_original_entities_and_authentic_stages() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    for scope in ["", "papers/ml"] {
        let output = docfoo_with_fake_sidecar()
            .args([
                "kg",
                "--query",
                "What are apples?",
                "--scope",
                scope,
                "--snapshot",
                "--model",
                "fake-provider/fake-model",
                "--json",
                "--workspace",
            ])
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let replay = &value["data"]["replay"];
        assert_eq!(replay["schema"], "docfoo.kg.replay/1");
        assert_eq!(replay["scope"], scope);
        assert!(!replay["buildUid"].as_str().unwrap().is_empty());
        assert_eq!(replay["nodes"][0]["id"], "CONCEPT_apple");
        assert_eq!(replay["edges"][0]["source"], "CONCEPT_apple");
        assert_eq!(replay["omittedNodes"], 0);
        let frames = replay["frames"].as_array().unwrap();
        assert_eq!(frames.first().unwrap()["step"], "depth");
        assert_eq!(frames.last().unwrap()["step"], "done");
        assert!(frames.iter().any(|f| f["step"] == "seeds"
            && f["data"]["ids"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("CONCEPT_apple"))));
        assert!(frames
            .windows(2)
            .all(|f| f[0]["seq"].as_u64() < f[1]["seq"].as_u64()));
        assert!(
            replay.get("graphPath").is_none(),
            "portable snapshots contain no native paths"
        );
    }
    let output = docfoo_with_fake_sidecar()
        .args(["kg", "--status", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["data"]["capabilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("kg.query.snapshot.v1")));
}

#[test]
fn kg_snapshot_requires_query_and_json_before_any_provider_work() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["kg", "--query", "question", "--snapshot"],
        vec!["kg", "--status", "--snapshot", "--json"],
    ] {
        let output = docfoo_with_fake_sidecar()
            .args(args)
            .arg("--workspace")
            .arg(temp.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}
