//! End-to-end KG query against a fixture graph using the fake sidecar.
//!
//! This is the deterministic counterpart to a live query: the fake answers the
//! synthesis call with `[S1]`, so the test covers graph load → retrieval →
//! evidence delivery → tag expansion → citation parsing → JSON envelope.

use std::path::Path;
use std::process::Command;

fn docfoo_with_fake_sidecar() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_docfoo"));
    command.env("DOCFOO_SIDECAR_BIN", env!("CARGO_BIN_EXE_fake-sidecar"));
    command
}

fn write_fixture_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("resources/doc/assets")).unwrap();
    std::fs::write(
        root.join("resources/doc/content.md"),
        "Apples are a tropical fruit.\nMangoes are also tropical fruit.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("graphs")).unwrap();
    let graph = serde_json::json!({
        "entities": {
            "CONCEPT_apple": {
                "id": "CONCEPT_apple",
                "name": "Apple",
                "type": "CONCEPT",
                "desc": "A tropical fruit.",
                "sections": ["[DOC] Apple facts"],
                "source_doc": ["doc/content.md"]
            }
        },
        "relations": [
            {
                "source": "CONCEPT_apple",
                "target": "CONCEPT_apple",
                "rel": "PART_OF",
                "section": "[DOC] Apple facts",
                "source_doc": "doc/content.md"
            }
        ],
        "sections": {
            "[DOC] Apple facts": {
                "topic": null,
                "entity_ids": ["CONCEPT_apple"],
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
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    let data = &value["data"];

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
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    let saved = value["data"]["saved"].as_str().expect("saved chat id");
    assert!(saved.starts_with("kg-"));
    let chat_dir = temp.path().join("kg-chats").join(saved);
    assert!(chat_dir.join("meta.json").is_file());
    assert!(chat_dir.join("kg-history.json").is_file());
    assert!(temp.path().join(".agent/current-kg-chat.json").is_file());
}
