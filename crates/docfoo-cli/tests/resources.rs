//! Read-only resources and notes against a fixture workspace.

use std::path::Path;
use std::process::Command;

fn docfoo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docfoo"))
}

fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("resources/Book/assets")).unwrap();
    std::fs::write(
        root.join("resources/Book/content.md"),
        "# Alpha\nalpha text line\n![](assets/figure_1.png)\nFigure 1. A chart\n| A | B |\n|---|---|\n| 1 | 2 |\n## Beta\nbeta line\n",
    )
    .unwrap();
    std::fs::write(root.join("resources/Book/assets/figure_1.png"), b"png").unwrap();
    std::fs::write(root.join("resources/notes.md"), "a loose note\n").unwrap();
    std::fs::write(
        root.join("notes.json"),
        r#"{
            "Book/content.md": [
                {"id": "n1", "text": "remember this", "sourceLine": 2, "originalText": "alpha text line"}
            ]
        }"#,
    )
    .unwrap();
}

fn run_json(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = docfoo()
        .args(args)
        .arg("--json")
        .arg("--workspace")
        .arg(root)
        .output()
        .expect("run docfoo");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json envelope")
}

#[test]
fn list_summarizes_folders() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["resources", "--list"]);
    assert_eq!(value["command"], "resources.list");
    let entries = value["data"]["entries"].as_array().unwrap();
    let book = entries
        .iter()
        .find(|entry| entry["name"] == "Book")
        .unwrap();
    assert_eq!(book["kind"], "dir");
    assert_eq!(book["files"], 2);
    assert_eq!(book["md"], 1);
    assert_eq!(book["figures"], 1);
    assert!(entries.iter().any(|entry| entry["name"] == "notes.md"));
}

#[test]
fn tree_preserves_children_and_figure_paths() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["resources", "--list", "--tree"]);
    let tree = value["data"]["tree"].as_array().unwrap();
    let book = tree.iter().find(|entry| entry["name"] == "Book").unwrap();
    assert_eq!(book["figures"][0], "Book/assets/figure_1.png");
    let names: Vec<&str> = book["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| child["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"content.md"));
    assert!(names.contains(&"assets"));
}

#[test]
fn read_numbers_lines_and_reports_figures() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["resources", "--read", "Book/content.md"]);
    assert_eq!(value["command"], "resources.read");
    let data = &value["data"];
    assert_eq!(data["totalLines"], 10);
    assert!(data["numberedText"]
        .as_str()
        .unwrap()
        .contains("    2 | alpha text line"));
    assert!(data["text"].as_str().unwrap().starts_with("# Alpha"));
    assert_eq!(data["figures"][0]["line"], 3);
    assert_eq!(data["figures"][0]["markdown"], "![](Book/assets/figure_1.png)");
}

#[test]
fn read_window_reports_next_offset() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(
        temp.path(),
        &[
            "resources",
            "--read",
            "Book/content.md",
            "--offset",
            "2",
            "--limit",
            "3",
        ],
    );
    let data = &value["data"];
    assert_eq!(data["startLine"], 2);
    assert_eq!(data["endLine"], 4);
    assert_eq!(data["nextOffset"], 5);
}

#[test]
fn read_figures_prints_markdown_only() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let output = docfoo()
        .args(["resources", "--read", "Book/content.md", "--figures", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "![](Book/assets/figure_1.png)");
}

#[test]
fn outline_lists_headings_with_counts() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["resources", "--outline", "Book/content.md"]);
    let data = &value["data"];
    assert_eq!(data["headingCount"], 2);
    assert_eq!(data["figureCount"], 1);
    assert_eq!(data["tableCount"], 1);
    let sections = data["sections"].as_array().unwrap();
    assert_eq!(sections[0]["line"], 1);
    assert_eq!(sections[0]["heading"], "Alpha");
    assert_eq!(sections[0]["figures"], 1);
    assert_eq!(sections[0]["tables"], 1);
}

#[test]
fn search_returns_hits_with_context_and_images() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(
        temp.path(),
        &["resources", "--search", "alpha", "--context", "1"],
    );
    let data = &value["data"];
    assert_eq!(data["total"], 2);
    let hit = data["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["line"] == 2)
        .expect("hit on line 2");
    assert_eq!(hit["file"], "Book/content.md");
    assert_eq!(hit["text"], "alpha text line");
    assert_eq!(hit["image"], "Book/assets/figure_1.png");
}

#[test]
fn search_with_no_hits_includes_a_hint() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["resources", "--search", "zzzzz"]);
    assert_eq!(value["data"]["total"], 0);
    assert!(value["data"]["hint"].as_str().unwrap().contains("zzzzz"));
}

#[test]
fn notes_list_and_read() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let value = run_json(temp.path(), &["notes", "--list"]);
    assert_eq!(value["command"], "notes.list");
    assert_eq!(value["data"]["total"], 1);
    assert_eq!(value["data"]["notes"][0]["resource"], "Book/content.md");
    assert_eq!(value["data"]["notes"][0]["id"], "n1");

    let value = run_json(temp.path(), &["notes", "--read", "n1"]);
    assert_eq!(value["command"], "notes.read");
    assert_eq!(value["data"]["resource"], "Book/content.md");
    assert_eq!(value["data"]["note"]["text"], "remember this");
}

#[test]
fn list_on_a_fresh_workspace_is_empty() {
    let temp = tempfile::tempdir().unwrap();
    let value = run_json(temp.path(), &["resources", "--list"]);
    assert_eq!(value["data"]["entries"].as_array().unwrap().len(), 0);
}

#[test]
fn path_escapes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let output = docfoo()
        .args(["resources", "--read", "../secret.md", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid resource path"), "stderr: {stderr}");
}

#[test]
fn missing_resource_is_not_found() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture(temp.path());
    let output = docfoo()
        .args(["resources", "--read", "Book/nope.md", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "stderr: {stderr}");
}
