//! Setup provisioning and scan preconditions.

use std::path::Path;
use std::process::Command;

fn docfoo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docfoo"))
}

fn write_models_dir(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("PP-DocLayoutV3.onnx"), b"fake model").unwrap();
    std::fs::write(dir.join("onnxruntime.dll"), b"fake ort").unwrap();
    std::fs::write(dir.join("pdfium.dll"), b"fake pdfium").unwrap();
}

#[test]
fn setup_check_reports_missing_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .env("DOCFOO_MODELS_DIR", temp.path().join("models"))
        .args(["setup", "--check", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["command"], "setup");
    assert_eq!(value["data"]["ready"], false);
    assert_eq!(value["data"]["missing"].as_array().unwrap().len(), 3);
}

#[test]
fn setup_from_a_local_models_dir_becomes_ready() {
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("docfoo-models");
    write_models_dir(&from);
    let ort = from.join("libonnxruntime.so");
    let pdfium = from.join("libpdfium.so");
    std::fs::write(&ort, b"fake ort").unwrap();
    std::fs::write(&pdfium, b"fake pdfium").unwrap();
    let workspace = temp.path().join("ws");
    let models = temp.path().join("models");

    let output = docfoo()
        .env("DOCFOO_MODELS_DIR", &models)
        .env("DOCFOO_ORT_DLL", &ort)
        .env("DOCFOO_PDFIUM_DLL", &pdfium)
        .args(["setup", "--from"])
        .arg(&from)
        .arg("--json")
        .arg("--workspace")
        .arg(&workspace)
        .output()
        .expect("run docfoo setup");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["data"]["ready"], true);
    assert!(models.join("PP-DocLayoutV3.onnx").is_file());
}

#[test]
fn scan_without_dependencies_reports_setup_hint() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("page.pdf");
    std::fs::write(&input, b"%PDF-1.4 fake").unwrap();
    let output = docfoo()
        .env("DOCFOO_MODELS_DIR", temp.path().join("models"))
        .args(["scan"])
        .arg(&input)
        .arg("--text_model")
        .arg("fake-provider/fake-model")
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dependencies are missing"), "stderr: {stderr}");
    assert!(stderr.contains("docfoo setup"), "stderr: {stderr}");
}

#[test]
fn scan_missing_file_is_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["scan", "nope.pdf", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("file not found"), "stderr: {stderr}");
}

#[test]
fn scan_without_a_model_reports_the_model_hint() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("page.pdf");
    std::fs::write(&input, b"%PDF-1.4 fake").unwrap();
    let output = docfoo()
        .args(["scan"])
        .arg(&input)
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no scan model selected"), "stderr: {stderr}");
}
