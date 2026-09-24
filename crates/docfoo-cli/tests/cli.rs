use std::process::Command;

fn docfoo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_docfoo"))
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
fn kg_query_stub_fails_with_sidecar_hint() {
    let output = docfoo()
        .args(["kg", "--query", "anything"])
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sidecar"), "stderr: {stderr}");
}

#[test]
fn kg_query_stub_json_error_envelope() {
    let output = docfoo()
        .args(["--json", "kg", "--query", "anything"])
        .output()
        .expect("run docfoo");
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["schema"], "docfoo.cli/1");
    assert_eq!(value["command"], "kg");
    assert_eq!(value["error"]["code"], "not_implemented");
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
