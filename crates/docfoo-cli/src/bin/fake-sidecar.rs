//! Test fixture: a minimal stand-in for `docfoo-agent`.
//!
//! Speaks the same JSONL protocol with canned responses. Integration tests
//! spawn it through `DOCFOO_SIDECAR_BIN` / `CARGO_BIN_EXE_fake-sidecar`.
//!
//! Env switches:
//!   FAKE_SIDECAR_DELAY_MS    sleep before answering (timeout tests)
//!   FAKE_SIDECAR_EXIT_AFTER  exit after N request frames (crash tests)
//!   FAKE_SIDECAR_FAIL        answer `complete` with success:false

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let delay_ms: u64 = env_number("FAKE_SIDECAR_DELAY_MS");
    let exit_after: u64 = env_number("FAKE_SIDECAR_EXIT_AFTER");
    let fail = std::env::var("FAKE_SIDECAR_FAIL").is_ok();

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{}", json!({ "type": "ready", "version": "fake" }));
    let _ = out.flush();

    let stdin = std::io::stdin();
    let mut count = 0u64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        count += 1;
        if exit_after > 0 && count >= exit_after {
            std::process::exit(0);
        }
        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        let request_id = value.get("requestId").cloned().unwrap_or(Value::Null);
        match value.get("type").and_then(Value::as_str).unwrap_or_default() {
            "ping" => emit(&mut out, json!({ "type": "pong", "requestId": request_id })),
            "cancel" => {}
            "models" => emit(
                &mut out,
                json!({ "type": "models_response", "requestId": request_id, "success": true, "providers": providers() }),
            ),
            "auth_status" => {
                let wanted = value.get("provider").and_then(Value::as_str);
                let list: Vec<Value> = providers()
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter(|item| wanted.map_or(true, |w| item["id"] == w))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                emit(
                    &mut out,
                    json!({ "type": "auth_status_response", "requestId": request_id, "success": true, "providers": list }),
                )
            }
            "auth_set" => emit(
                &mut out,
                json!({ "type": "auth_set_response", "requestId": request_id, "success": true }),
            ),
            "auth_logout" => emit(
                &mut out,
                json!({ "type": "auth_logout_response", "requestId": request_id, "success": true }),
            ),
            "complete" => {
                // KG synthesis requests carry the evidence block; answer with
                // an [S1] tag so the crate's tag expansion is exercised.
                let text = if request_contains(&value, "EVIDENCE:") {
                    "Synthesized answer. [S1]"
                } else {
                    "fake completion"
                };
                if value.get("stream").and_then(Value::as_bool) == Some(true) {
                    emit(
                        &mut out,
                        json!({ "type": "stream_delta", "requestId": request_id, "text": text }),
                    );
                }
                if fail {
                    emit(
                        &mut out,
                        json!({ "type": "complete_response", "requestId": request_id, "success": false, "error": "fake failure" }),
                    );
                } else {
                    emit(
                        &mut out,
                        json!({ "type": "complete_response", "requestId": request_id, "success": true, "text": text }),
                    );
                }
            }
            _ => {}
        }
    }
}

fn env_number(name: &str) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// True when any string message content contains `needle`.
fn request_contains(value: &Value, needle: &str) -> bool {
    value
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|content| content.contains(needle))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn emit(out: &mut impl Write, value: Value) {
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn providers() -> Value {
    json!([
        {
            "id": "fake-provider",
            "name": "Fake Provider",
            "configured": true,
            "source": "stored",
            "modelCount": 2,
            "models": [
                {
                    "id": "fake-model",
                    "name": "Fake Model",
                    "contextWindow": 128000,
                    "maxTokens": 8192,
                    "api": "openai-completions"
                },
                { "id": "other/model", "name": "Slashed Model" }
            ]
        },
        {
            "id": "unconfigured",
            "name": "Unconfigured",
            "configured": false,
            "modelCount": 1,
            "models": [{ "id": "lonely" }]
        }
    ])
}
