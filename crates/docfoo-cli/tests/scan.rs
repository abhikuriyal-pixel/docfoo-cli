//! End-to-end `docfoo scan` tests: one command, multiple documents.
//!
//! These tests need the real native scan dependencies (`docfoo setup`). When
//! they are not installed the test prints a skip note and passes, so offline
//! CI stays green.

use std::path::{Path, PathBuf};
use std::process::Command;

/// 1x1 RGBA PNG (valid input for the `image` decoder).
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn docfoo_with_fake_sidecar() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_docfoo"));
    command.env("DOCFOO_SIDECAR_BIN", env!("CARGO_BIN_EXE_fake-sidecar"));
    command
}

/// The models directory to scan with: `DOCFOO_MODELS_DIR` when set,
/// otherwise the CLI's default workspace (`~/.docfoo/models`).
fn real_models_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = std::env::var_os("DOCFOO_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".docfoo").join("models"));
    let present = [
        docfoo_ocr::layout_model_file_name(),
        docfoo_ocr::ort_library_file_name(),
        docfoo_ocr::pdfium_library_file_name(),
    ]
    .iter()
    .all(|name| dir.join(name).is_file());
    present.then_some(dir)
}

fn write_png(path: &Path) {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1)
        .expect("valid png base64");
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn scan_multiple_files_with_jobs_emits_results_array() {
    let Some(models_dir) = real_models_dir() else {
        eprintln!(
            "skipping scan_multiple_files_with_jobs_emits_results_array: \
             real scan dependencies not found (run `docfoo setup`)"
        );
        return;
    };

    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let first = temp.path().join("first.png");
    let second = temp.path().join("second.png");
    write_png(&first);
    write_png(&second);

    let output = docfoo_with_fake_sidecar()
        .args(["scan", "--jobs", "2", "--quiet", "--json", "--workspace"])
        .arg(&workspace)
        .args(["--text_model", "fake-provider/fake-model"])
        .args(["--no-figures"])
        .arg(&first)
        .arg(&second)
        .env("DOCFOO_MODELS_DIR", &models_dir)
        .output()
        .expect("run docfoo scan");

    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["ok"], true, "{value:#?}");
    assert_eq!(value["command"], "scan");
    let results = value["data"]["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "{value:#?}");
    assert!(
        results.iter().all(|result| result["ok"] == true),
        "{value:#?}"
    );

    let rels: Vec<String> = results
        .iter()
        .map(|result| result["rel"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(rels.iter().all(|rel| !rel.is_empty()));
    assert_ne!(rels[0], rels[1]);
    for rel in &rels {
        assert!(
            workspace
                .join("resources")
                .join(rel)
                .join("content.md")
                .is_file(),
            "missing card {rel}"
        );
    }
}
