//! Backup and restore round-trips against fixture workspaces.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn docfoo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docfoo"))
}

fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("resources/Book/assets")).unwrap();
    std::fs::write(root.join("resources/Book/content.md"), "book content\n").unwrap();
    std::fs::write(root.join("resources/Book/assets/figure_1.png"), b"png").unwrap();
    std::fs::write(root.join("notes.json"), r#"{"Book/content.md":[]}"#).unwrap();
    std::fs::create_dir_all(root.join(".agent/sessions")).unwrap();
    let session = format!(
        "{{\"type\":\"meta\",\"cwd\":\"{}\"}}\n",
        root.display().to_string().replace('\\', "\\\\")
    );
    std::fs::write(root.join(".agent/sessions/s1.jsonl"), session).unwrap();
}

#[test]
fn backup_then_restore_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    let backup_file = temp.path().join("backup.zip");
    write_fixture(&source);

    let output = docfoo()
        .args(["backup", "--json", "--out"])
        .arg(&backup_file)
        .arg("--workspace")
        .arg(&source)
        .output()
        .expect("run docfoo backup");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(backup_file.is_file(), "backup zip was not created");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("backup json envelope");
    assert_eq!(value["command"], "backup");
    assert_eq!(value["data"]["resources"], 2);

    let output = docfoo()
        .args(["restore", "--json", "--yes"])
        .arg(&backup_file)
        .arg("--workspace")
        .arg(&target)
        .output()
        .expect("run docfoo restore");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("restore json envelope");
    assert_eq!(value["command"], "restore");
    assert_eq!(value["data"]["resources"], 2);
    assert!(target.join("resources/Book/content.md").is_file());
    assert!(target.join("resources/Book/assets/figure_1.png").is_file());
    assert!(target.join("notes.json").is_file());
    assert!(target.join(".agent/sessions/s1.jsonl").is_file());

    // Workspace paths inside sessions are rewritten to the new root.
    let session = std::fs::read_to_string(target.join(".agent/sessions/s1.jsonl")).unwrap();
    let session_value: serde_json::Value = serde_json::from_str(session.lines().next().unwrap()).unwrap();
    assert_eq!(
        session_value["cwd"].as_str().unwrap(),
        target.display().to_string()
    );
}

#[test]
fn restore_without_yes_prompts_and_cancels_on_n() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    let backup_file = temp.path().join("backup.zip");
    write_fixture(&source);
    docfoo()
        .args(["backup", "--out"])
        .arg(&backup_file)
        .arg("--workspace")
        .arg(&source)
        .output()
        .expect("backup");

    let mut child = docfoo()
        .args(["restore"])
        .arg(&backup_file)
        .arg("--workspace")
        .arg(&target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn restore");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"n\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cancelled"), "stderr: {stderr}");
    assert!(!target.join("resources").exists());
}

#[test]
fn restore_rejects_a_non_backup_zip() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let not_a_backup = temp.path().join("random.zip");
    // A valid zip with no manifest inside.
    {
        let file = std::fs::File::create(&not_a_backup).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        use std::io::Write as _;
        zip.start_file("hello.txt", options).unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
    }
    let output = docfoo()
        .args(["restore"])
        .arg(&not_a_backup)
        .arg("--yes")
        .arg("--workspace")
        .arg(&target)
        .output()
        .expect("restore");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a DocFoo backup"), "stderr: {stderr}");
}

#[test]
fn backup_missing_workspace_is_empty_but_valid() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("fresh");
    let backup_file = temp.path().join("empty.zip");
    let output = docfoo()
        .args(["backup", "--out"])
        .arg(&backup_file)
        .arg("--workspace")
        .arg(&workspace)
        .output()
        .expect("backup");
    assert!(output.status.success());
    assert!(backup_file.is_file());
}
