//! End-to-end tests for `docfoo resources --vis`.
//!
//! Each test starts the real binary on an ephemeral loopback port, then
//! exercises the HTTP API the browser page uses: library listings with cover
//! figures, markdown reads, asset serving, traversal refusal and the note
//! create/edit/delete round-trip through `notes.json`.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn write_fixture_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("resources/Root/assets")).unwrap();
    std::fs::write(
        root.join("resources/Root/content.md"),
        "# Chapter One\n\nHello **world**.\n\n![Figure](assets/f1.png)\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n> A quoted note.\n\n$$\nE = mc^2\n$$\n",
    )
    .unwrap();
    std::fs::write(root.join("resources/Root/assets/f1.png"), b"fake-png").unwrap();
    std::fs::write(root.join("resources/Root/assets/f2.png"), b"fake-png").unwrap();
    std::fs::write(root.join("resources/Root/notes.txt"), b"plain text").unwrap();
    // Hidden entries mirror the desktop browser.
    std::fs::create_dir_all(root.join("resources/Root/notebooks")).unwrap();
    std::fs::write(root.join("resources/Root/notebooks/demo.py"), b"print(1)").unwrap();
    std::fs::write(root.join("resources/Root/ocr_cache.json"), b"{}").unwrap();

    std::fs::create_dir_all(root.join("resources/Shelf/Book/assets")).unwrap();
    std::fs::write(root.join("resources/Shelf/Book/content.md"), "# Book\n").unwrap();
    std::fs::write(root.join("resources/Shelf/Book/assets/cover.png"), b"fake").unwrap();
    std::fs::write(root.join("resources/loose.md"), "# Loose\n").unwrap();

    std::fs::write(root.join("secret.txt"), b"do not serve").unwrap();
    std::fs::write(
        root.join("notes.json"),
        r#"{"Other/content.md":[{"id":"keep","text":"keep me"}]}"#,
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

#[allow(clippy::zombie_processes)]
fn start_vis(root: &Path, rel: &str) -> VisProcess {
    let log_path = root.join("resources-vis-stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut args = vec![
        "resources",
        "--vis",
        "--no-open",
        "--port",
        "0",
    ];
    if !rel.is_empty() {
        args.extend(["--rel", rel]);
    }
    args.push("--workspace");
    let child = Command::new(env!("CARGO_BIN_EXE_docfoo"))
        .args(&args)
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn docfoo resources --vis");

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
            panic!("resources vis did not print a URL within 20s; stderr:\n{log}");
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

fn get_text(base: &str, path: &str) -> (u16, String) {
    let response = agent().get(format!("{base}{path}")).call().expect("GET");
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().unwrap_or_default();
    (status, text)
}

fn get_bytes(base: &str, path: &str) -> (u16, Vec<u8>) {
    let response = agent().get(format!("{base}{path}")).call().expect("GET");
    let status = response.status().as_u16();
    let bytes = response.into_body().read_to_vec().unwrap_or_default();
    (status, bytes)
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

fn names(level: &Value) -> Vec<String> {
    level["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn browser_serves_levels_covers_and_markdown() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    let (status, state) = get_json(&vis.base, "/api/state");
    assert_eq!(status, 200, "state: {state}");
    assert_eq!(state["rel"], "");
    assert!(!state["workspace"].as_str().unwrap().is_empty());

    // The page and both front-end bundles are served.
    let (status, html) = get_text(&vis.base, "/");
    assert_eq!(status, 200);
    assert!(html.contains("DocFoo"), "page: {html}");
    assert_eq!(get_text(&vis.base, "/app.css").0, 200);
    assert_eq!(get_text(&vis.base, "/base.css").0, 200);
    assert_eq!(get_text(&vis.base, "/js/app.js").0, 200);
    assert_eq!(get_text(&vis.base, "/js/markdown.js").0, 200);
    assert_eq!(get_text(&vis.base, "/katex/katex.min.js").0, 200);
    assert_eq!(get_text(&vis.base, "/nope.js").0, 404);

    // Root level: folders first, hidden entries filtered, covers sampled from
    // a folder's own assets.
    let (status, level) = get_json(&vis.base, "/api/resources?rel=");
    assert_eq!(status, 200, "level: {level}");
    assert_eq!(names(&level), vec!["Root", "Shelf", "loose.md"]);
    let root = &level["entries"][0];
    let covers: Vec<&str> = root["covers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cover| cover.as_str().unwrap())
        .collect();
    assert!(covers.contains(&"Root/assets/f1.png"), "covers: {covers:?}");
    assert!(covers.contains(&"Root/assets/f2.png"));
    assert!(!covers.iter().any(|cover| cover.contains("notebooks")));

    // A folder of resources samples its children's assets.
    let (_, shelf) = get_json(&vis.base, "/api/resources?rel=Shelf");
    assert_eq!(names(&shelf), vec!["Book"]);
    assert_eq!(
        shelf["entries"][0]["covers"].as_array().unwrap()[0].as_str().unwrap(),
        "Shelf/Book/assets/cover.png"
    );

    // Traversal and unknown folders are refused.
    assert_eq!(get_json(&vis.base, "/api/resources?rel=..%2F").0, 404);
    assert_eq!(get_json(&vis.base, "/api/resources?rel=Missing").0, 404);

    // Markdown comes back as source for the reader.
    let (status, resource) = get_json(&vis.base, "/api/resource?path=Root/content.md");
    assert_eq!(status, 200, "resource: {resource}");
    assert_eq!(resource["kind"], "md");
    assert!(resource["text"].as_str().unwrap().contains("# Chapter One"));
    assert_eq!(
        get_json(&vis.base, "/api/resource?path=..%2Fsecret.txt").0,
        404
    );
    assert_eq!(get_json(&vis.base, "/api/resource?path=Missing%2Fx.md").0, 404);

    // Figures stream through /api/asset; escapes are refused.
    let (status, bytes) = get_bytes(&vis.base, "/api/asset?path=Root%2Fassets%2Ff1.png");
    assert_eq!(status, 200);
    assert_eq!(bytes, b"fake-png");
    assert_eq!(get_bytes(&vis.base, "/api/asset?path=..%2Fsecret.txt").0, 404);
}

#[test]
fn notes_create_edit_and_delete_through_the_shared_file() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "");

    let (status, empty) = get_json(&vis.base, "/api/notes?path=Root/content.md");
    assert_eq!(status, 200);
    assert_eq!(empty["notes"].as_array().unwrap().len(), 0);

    let note = json!({
        "id": "ann-1",
        "text": "first thought",
        "originalText": "Hello world",
        "sourceLine": 3,
        "anchor": { "startLine": 2, "startCol": 0, "endLine": 2, "endCol": 6 }
    });
    let (status, created) = post_json(
        &vis.base,
        "/api/notes",
        &json!({ "rel": "Root/content.md", "note": note }),
    );
    assert_eq!(status, 200, "create: {created}");
    let saved = &created["notes"][0];
    assert_eq!(saved["type"], "NOTE");
    assert_eq!(saved["filePath"], "Root/content.md");
    assert_eq!(saved["text"], "first thought");
    let created_stamp = saved["created"].as_i64().unwrap();
    assert!(created_stamp > 0);

    // The desktop file is the source of truth and other resources survive.
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(temp.path().join("notes.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["Other/content.md"][0]["text"], "keep me");
    assert_eq!(on_disk["Root/content.md"][0]["id"], "ann-1");

    // Editing keeps the id and creation stamp.
    let (status, edited) = post_json(
        &vis.base,
        "/api/notes",
        &json!({
            "rel": "Root/content.md",
            "note": { "id": "ann-1", "text": "second thought", "originalText": "Hello world", "sourceLine": 3 }
        }),
    );
    assert_eq!(status, 200, "edit: {edited}");
    assert_eq!(edited["notes"].as_array().unwrap().len(), 1);
    assert_eq!(edited["notes"][0]["text"], "second thought");
    assert_eq!(edited["notes"][0]["created"].as_i64().unwrap(), created_stamp);

    // Bad payloads are refused without touching the file.
    assert_eq!(
        post_json(
            &vis.base,
            "/api/notes",
            &json!({ "rel": "Missing/content.md", "note": note })
        )
        .0,
        404
    );
    assert_eq!(
        post_json(
            &vis.base,
            "/api/notes",
            &json!({ "rel": "Root/content.md", "note": { "id": "", "text": "x" } })
        )
        .0,
        400
    );

    let (status, removed) = post_json(
        &vis.base,
        "/api/notes/delete",
        &json!({ "rel": "Root/content.md", "id": "ann-1" }),
    );
    assert_eq!(status, 200, "delete: {removed}");
    assert_eq!(removed["notes"].as_array().unwrap().len(), 0);
    assert_eq!(
        post_json(
            &vis.base,
            "/api/notes/delete",
            &json!({ "rel": "Root/content.md", "id": "ann-1" })
        )
        .0,
        404
    );
}

#[test]
fn vis_starts_inside_a_folder_and_rejects_bad_arguments() {
    let temp = tempfile::tempdir().unwrap();
    write_fixture_workspace(temp.path());
    let vis = start_vis(temp.path(), "Shelf");

    let (status, state) = get_json(&vis.base, "/api/state");
    assert_eq!(status, 200);
    assert_eq!(state["rel"], "Shelf");

    let (_, level) = get_json(&vis.base, "/api/resources?rel=Shelf");
    assert_eq!(names(&level), vec!["Book"]);

    // A missing --rel folder fails fast with a clear message.
    let output = Command::new(env!("CARGO_BIN_EXE_docfoo"))
        .args([
            "resources",
            "--vis",
            "--no-open",
            "--rel",
            "Nope",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("resource folder not found"), "stderr: {stderr}");

    // --vis is exclusive with the other resource actions.
    let output = Command::new(env!("CARGO_BIN_EXE_docfoo"))
        .args(["resources", "--vis", "--list", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--vis") && stderr.contains("--list"),
        "stderr: {stderr}"
    );
}
