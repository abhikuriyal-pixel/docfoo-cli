use std::process::Command;

fn docfoo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docfoo"))
}

fn docfoo_with_fake_sidecar() -> Command {
    let mut command = docfoo();
    command.env("DOCFOO_SIDECAR_BIN", env!("CARGO_BIN_EXE_fake-sidecar"));
    command
}

#[test]
fn version_prints_the_package_version() {
    let output = docfoo().arg("version").output().expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "stdout: {stdout}"
    );
}

#[test]
fn version_json_envelope_is_stable() {
    let output = docfoo()
        .args(["version", "--json"])
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema"], "docfoo.cli/1");
    assert_eq!(value["command"], "version");
    assert!(value["data"]["cliVersion"].is_string());
    assert!(value["data"]["sidecarVersion"].is_null());
}

#[test]
fn help_lists_core_commands() {
    let output = docfoo().arg("help").output().expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "kg",
        "scan",
        "resources",
        "notes",
        "backup",
        "restore",
        "collections",
        "model",
        "auth",
        "setup",
        "version",
        "update",
    ] {
        assert!(
            stdout.contains(command),
            "help is missing {command}: {stdout}"
        );
    }
}

#[test]
fn kg_query_without_graph_reports_the_index_hint() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["kg", "--query", "anything", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("kg --index"), "stderr: {stderr}");
}

#[test]
fn kg_query_json_error_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["--json", "kg", "--query", "anything", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["schema"], "docfoo.cli/1");
    assert_eq!(value["command"], "kg");
    assert_eq!(value["error"]["code"], "not_found");
}

#[test]
fn kg_status_on_an_empty_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["kg", "--status", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["command"], "kg.status");
    assert_eq!(value["data"]["exists"], false);
    assert_eq!(value["data"]["built"].as_array().unwrap().len(), 0);
}

#[test]
fn kg_index_with_no_resources_reports_nothing_to_index() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args(["kg", "--index", "--model", "fake-provider/fake-model", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nothing to index"), "stderr: {stderr}");
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let output = docfoo()
        .args(["version", "--nope"])
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn workspace_flag_is_accepted_after_the_subcommand() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["version", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn version_verbose_shows_paths() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["version", "--verbose", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("workspace:"));
    assert!(stdout.contains("models dir:"));
    assert!(stdout.contains("platform:"));
}

// ---------------------------------------------------------------------------
// model / auth (Stage 1.2)
// ---------------------------------------------------------------------------

#[test]
fn model_list_uses_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args(["model", "--list", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fake-provider"), "stdout: {stdout}");
    assert!(stdout.contains("fake-model"), "stdout: {stdout}");
}

#[test]
fn model_list_json_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args(["model", "--list", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["command"], "model");
    assert_eq!(value["data"]["providers"][0]["id"], "fake-provider");
    assert_eq!(value["data"]["providers"][0]["models"][1]["id"], "other/model");
}

#[test]
fn model_get_reads_the_workspace_file() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("model-selection.json"),
        r#"{"kgModelKey":"fake-provider/fake-model"}"#,
    )
    .unwrap();
    let output = docfoo()
        .args(["model", "--get", "kg", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["data"]["key"], "fake-provider/fake-model");
    assert_eq!(value["data"]["configured"], true);
}

#[test]
fn model_get_defaults_to_kg_slot() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args(["model", "--get", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kg = (not set)"), "stdout: {stdout}");
}

#[test]
fn model_set_validates_and_saves() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args([
            "model",
            "--set",
            "kg",
            "fake-provider/fake-model",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let saved = std::fs::read_to_string(temp.path().join("model-selection.json")).unwrap();
    assert!(saved.contains("fake-provider/fake-model"));
}

#[test]
fn model_set_rejects_unknown_model() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args([
            "model",
            "--set",
            "kg",
            "fake-provider/nope",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown model"), "stderr: {stderr}");
    assert!(!temp.path().join("model-selection.json").exists());
}

#[test]
fn model_set_rejects_unknown_slot() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .args([
            "model",
            "--set",
            "nope",
            "fake-provider/fake-model",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown model slot"), "stderr: {stderr}");
}

#[test]
fn auth_status_never_prints_keys() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args(["auth", "--status", "--json", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("json envelope");
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["providers"][0]["id"], "fake-provider");
    assert!(value["data"]["providers"][0].get("key").is_none());
    assert!(!text.contains("apiKey"), "status must not include key fields: {text}");
}

#[test]
fn auth_set_reports_success_without_echoing_the_key() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args([
            "auth",
            "--set",
            "fake-provider",
            "--key",
            "super-secret",
            "--workspace",
        ])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("super-secret"), "output leaked the key: {combined}");
}

#[test]
fn auth_logout_reports_success() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo_with_fake_sidecar()
        .args(["auth", "--logout", "fake-provider", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("removed credential for fake-provider"));
}

#[test]
fn missing_sidecar_reports_a_build_hint() {
    let temp = tempfile::tempdir().unwrap();
    let output = docfoo()
        .env_remove("DOCFOO_SIDECAR_BIN")
        .env_remove("DOCFOO_SIDECAR_TS")
        .args(["model", "--list", "--workspace"])
        .arg(temp.path())
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sidecar is not built"), "stderr: {stderr}");
}
