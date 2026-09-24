//! Sidecar client tests against the `fake-sidecar` fixture binary.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use docfoo_cli::sidecar::{CompletionRequest, SidecarClient, SidecarError, SidecarLaunch};
use serde_json::json;

fn fake_launch() -> SidecarLaunch {
    SidecarLaunch::Binary(PathBuf::from(env!("CARGO_BIN_EXE_fake-sidecar")))
}

fn agent_dir() -> &'static Path {
    Path::new(".")
}

fn spawn_with_env(extra: &[(&str, &str)]) -> SidecarClient {
    SidecarClient::spawn_with_env(&fake_launch(), agent_dir(), extra).expect("spawn fake sidecar")
}

fn request() -> CompletionRequest {
    CompletionRequest::new(
        "fake-provider/fake-model",
        vec![json!({"role":"user","content":"hi"})],
    )
}

#[test]
fn ping_round_trip() {
    let mut client = spawn_with_env(&[]);
    client.ping().expect("ping");
}

#[test]
fn complete_returns_text() {
    let mut client = spawn_with_env(&[]);
    let text = client
        .complete(request(), None, None, Duration::from_secs(10))
        .expect("complete");
    assert_eq!(text, "fake completion");
}

#[test]
fn complete_streams_deltas() {
    let mut client = spawn_with_env(&[]);
    let mut request = request();
    request.stream = true;
    let mut deltas = String::new();
    let text = client
        .complete(
            request,
            None,
            Some(&mut |delta| deltas.push_str(delta)),
            Duration::from_secs(10),
        )
        .expect("complete");
    assert_eq!(text, "fake completion");
    assert_eq!(deltas, "streamed ");
}

#[test]
fn models_and_auth_status_parse() {
    let mut client = spawn_with_env(&[]);
    let providers = client.models().expect("models");
    assert_eq!(providers[0].id, "fake-provider");
    assert_eq!(providers[0].models.len(), 2);
    assert_eq!(providers[0].models[1].id, "other/model");

    let status = client.auth_status(Some("fake-provider")).expect("status");
    assert_eq!(status.len(), 1);
    assert!(status[0].configured);
    assert_eq!(status[0].source.as_deref(), Some("stored"));
}

#[test]
fn auth_set_and_logout_succeed() {
    let mut client = spawn_with_env(&[]);
    client.auth_set("fake-provider", "secret-key").expect("auth set");
    client.auth_logout("fake-provider").expect("auth logout");
}

#[test]
fn model_failure_surfaces_the_message() {
    let mut client = spawn_with_env(&[("FAKE_SIDECAR_FAIL", "1")]);
    let error = client
        .complete(request(), None, None, Duration::from_secs(10))
        .unwrap_err();
    assert!(matches!(error, SidecarError::Model(_)));
    assert!(error.to_string().contains("fake failure"));
}

#[test]
fn timeout_is_reported() {
    let mut client = spawn_with_env(&[("FAKE_SIDECAR_DELAY_MS", "5000")]);
    let error = client
        .complete(request(), None, None, Duration::from_millis(300))
        .unwrap_err();
    assert!(matches!(error, SidecarError::Transport(_)));
    assert!(error.to_string().contains("in time"));
}

#[test]
fn crash_fails_the_request() {
    let mut client = spawn_with_env(&[("FAKE_SIDECAR_EXIT_AFTER", "1")]);
    let error = client
        .complete(request(), None, None, Duration::from_secs(10))
        .unwrap_err();
    assert!(matches!(error, SidecarError::Transport(_)));
}

#[test]
fn cancel_flag_stops_a_request() {
    let flag = AtomicBool::new(true);
    let mut client = spawn_with_env(&[("FAKE_SIDECAR_DELAY_MS", "5000")]);
    let error = client
        .complete(request(), Some(&flag), None, Duration::from_secs(10))
        .unwrap_err();
    assert!(matches!(error, SidecarError::Cancelled));
}
