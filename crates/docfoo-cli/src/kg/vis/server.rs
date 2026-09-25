//! HTTP routing, the query worker and static/asset serving for `kg --vis`.
//!
//! The server binds loopback only. Routes:
//!
//! | Method | Path            | Purpose                                        |
//! |--------|-----------------|------------------------------------------------|
//! | GET    | `/`             | embedded visualizer page                       |
//! | GET    | `/app.css`      | embedded stylesheet                            |
//! | GET    | `/js/*`         | embedded ES modules                            |
//! | GET    | `/api/state`    | workspace, built scopes, default model/reasoning |
//! | GET    | `/api/graph`    | graph projection for `?scope=`                  |
//! | GET    | `/api/models`   | provider/model catalog (sidecar)               |
//! | GET    | `/api/asset`    | image bytes under `resources/` for `?path=`     |
//! | GET    | `/api/events`   | frames since `?since=N` (the page polls)      |
//! | POST   | `/api/query`    | start a query                                   |
//! | POST   | `/api/cancel`   | cancel the running query                        |
//!
//! Frames are polled rather than pushed because `tiny_http` buffers chunked
//! responses (8 KB) and cannot stream SSE; the cursor endpoint is the same
//! event log with a tiny poll loop on the page.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use docfoo_kg::llm::Reasoning;
use docfoo_kg::query::StageEvent;
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, ResponseBox, StatusCode};

use crate::error::Result;
use crate::kg::{paths, query};
use crate::sidecar::{ProviderInfo, Sidecar, SidecarClient};
use crate::workspace::Workspace;

use super::assets;
use super::hub::EventHub;
use super::projection::Projector;

/// Per-request body ceiling (queries are tiny).
const MAX_BODY_BYTES: u64 = 256 * 1024;

pub struct ServerState {
    pub workspace: Workspace,
    pub default_scope: String,
    pub default_model: Option<String>,
    /// `off` | `default` | a pi thinking level.
    pub default_reasoning: String,
    pub hub: Arc<EventHub>,
    pub projector: Projector,
    sidecar: Mutex<Option<(Sidecar, Arc<SidecarClient>)>>,
    models: Mutex<Option<Vec<ProviderInfo>>>,
}

impl ServerState {
    pub fn new(
        workspace: Workspace,
        default_scope: String,
        default_model: Option<String>,
        default_reasoning: String,
    ) -> Self {
        Self {
            workspace,
            default_scope,
            default_model,
            default_reasoning,
            hub: Arc::new(EventHub::new()),
            projector: Projector::new(),
            sidecar: Mutex::new(None),
            models: Mutex::new(None),
        }
    }

    /// Lazily spawn the sidecar and keep both the client and its owner alive.
    fn sidecar_client(&self) -> Result<Arc<SidecarClient>> {
        let mut guard = self.sidecar.lock().expect("sidecar lock");
        if let Some((_, client)) = guard.as_ref() {
            return Ok(Arc::clone(client));
        }
        let mut sidecar = Sidecar::locate(&self.workspace)?;
        let client = sidecar.client()?;
        guard.replace((sidecar, Arc::clone(&client)));
        Ok(client)
    }

    fn list_models(&self, refresh: bool) -> Result<Vec<ProviderInfo>> {
        if !refresh {
            if let Some(cached) = self.models.lock().expect("models lock").as_ref() {
                return Ok(cached.clone());
            }
        }
        let providers = self.sidecar_client()?.models(refresh)?;
        self.models
            .lock()
            .expect("models lock")
            .replace(providers.clone());
        Ok(providers)
    }
}

/// Accept loop. Each request gets its own thread so an open SSE stream never
/// blocks static traffic.
pub fn serve_loop(server: tiny_http::Server, state: Arc<ServerState>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(request)) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let mut request = request;
                    let response = route(&mut request, &state);
                    let _ = request.respond(response);
                });
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn route(request: &mut Request, state: &Arc<ServerState>) -> ResponseBox {
    let method = request.method().clone();
    let (path, query_string) = split_url(request.url());

    match (&method, path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => assets::serve("index.html"),
        (Method::Get, "/app.css") => assets::serve("app.css"),
        (Method::Get, other) if other.starts_with("/js/") || other.starts_with("/katex/") => {
            assets::serve(other.trim_start_matches('/'))
        }

        (Method::Get, "/api/state") => api_state(state),
        (Method::Get, "/api/graph") => api_graph(query_string, state),
        (Method::Get, "/api/models") => api_models(query_string, state),
        (Method::Get, "/api/asset") => api_asset(query_string, state),
        (Method::Get, "/api/events") => api_events(query_string, state),

        (Method::Post, "/api/query") => api_query(request, state),
        (Method::Post, "/api/cancel") => {
            state.hub.cancel();
            json_response(200, &json!({ "cancelled": true }))
        }

        _ => error_response(404, "not found"),
    }
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

fn api_state(state: &Arc<ServerState>) -> ResponseBox {
    let built = paths::list_built(&state.workspace);
    let graph_path = paths::graph_path(&state.workspace, &state.default_scope);
    json_response(
        200,
        &json!({
            "workspace": state.workspace.root.display().to_string(),
            "scope": state.default_scope,
            "built": built,
            "graphExists": graph_path.is_file(),
            "model": state.default_model,
            "reasoning": state.default_reasoning,
            "busy": state.hub.is_busy(),
        }),
    )
}

fn api_graph(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let scope = match requested_scope(query_string, state) {
        Ok(scope) => scope,
        Err(message) => return error_response(400, &message),
    };
    let graph_path = paths::graph_path(&state.workspace, &scope);
    if !graph_path.is_file() {
        return json_response(
            404,
            &json!({
                "error": missing_graph_message(&scope),
                "scope": scope,
                "built": paths::list_built(&state.workspace),
            }),
        );
    }
    match state.projector.project(&graph_path) {
        Ok(projection) => json_response(200, &projection),
        Err(error) => error_response(500, &error),
    }
}

fn api_models(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let refresh = query_param(query_string, "refresh")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    match state.list_models(refresh) {
        Ok(providers) => json_response(200, &json!({ "providers": providers })),
        Err(error) => error_response(502, &error.to_string()),
    }
}

fn api_asset(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let Some(rel) = query_param(query_string, "path") else {
        return error_response(400, "missing path");
    };
    match assets::resolve_resource(&state.workspace, &rel) {
        Some((path, mime)) => match std::fs::read(&path) {
            Ok(bytes) => Response::from_data(bytes)
                .with_status_code(StatusCode(200))
                .with_header(header("Content-Type", mime))
                .with_header(header("Cache-Control", "no-cache"))
                .with_header(header("X-Content-Type-Options", "nosniff"))
                .boxed(),
            Err(error) => error_response(404, &format!("could not read asset: {error}")),
        },
        None => error_response(404, "asset not found under resources/"),
    }
}

fn api_events(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let cursor = query_param(query_string, "since")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let events: Vec<Value> = state
        .hub
        .since(cursor)
        .into_iter()
        .filter_map(|event| {
            let data = serde_json::from_str::<Value>(&event.data).ok()?;
            Some(json!({ "id": event.id, "event": event.event, "data": data }))
        })
        .collect();
    let next = events
        .iter()
        .filter_map(|event| event["id"].as_u64())
        .max()
        .unwrap_or(cursor);
    json_response(200, &json!({ "cursor": next, "events": events }))
}

#[derive(Deserialize)]
struct QueryBody {
    query: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// `off` | `default` | a pi thinking level.
    #[serde(default)]
    reasoning: Option<String>,
}

fn api_query(request: &mut Request, state: &Arc<ServerState>) -> ResponseBox {
    let body = match read_body(request) {
        Ok(body) => body,
        Err(message) => return error_response(400, &message),
    };
    let parsed: QueryBody = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(error) => return error_response(400, &format!("invalid query body: {error}")),
    };
    let text = parsed.query.trim();
    if text.is_empty() {
        return error_response(400, "query must not be empty");
    }
    let scope = match parsed
        .scope
        .as_deref()
        .map(paths::normalize_scope)
        .transpose()
    {
        Ok(scope) => scope.unwrap_or_else(|| state.default_scope.clone()),
        Err(error) => return error_response(400, &error.to_string()),
    };
    let model = parsed
        .model
        .clone()
        .or_else(|| state.default_model.clone());
    let Some(model) = model else {
        return error_response(
            409,
            "no model selected — pass --model provider/model or run `docfoo model --set kg provider/model`",
        );
    };
    let reasoning = parsed
        .reasoning
        .clone()
        .unwrap_or_else(|| state.default_reasoning.clone());

    let cancel = Arc::new(AtomicBool::new(false));
    if !state.hub.begin_query(Arc::clone(&cancel)) {
        return error_response(409, "a query is already running");
    }
    state.hub.emit(
        "start",
        json!({
            "type": "start",
            "query": text,
            "scope": scope,
            "model": model,
            "reasoning": reasoning,
        })
        .to_string(),
    );

    let state = Arc::clone(state);
    let query_text = text.to_string();
    std::thread::spawn(move || {
        run_query_worker(state, scope, query_text, model, reasoning, cancel);
    });

    json_response(202, &json!({ "started": true }))
}

// ---------------------------------------------------------------------------
// Query worker
// ---------------------------------------------------------------------------

fn run_query_worker(
    state: Arc<ServerState>,
    scope: String,
    query_text: String,
    model: String,
    reasoning: String,
    cancel: Arc<AtomicBool>,
) {
    let hub = Arc::clone(&state.hub);

    let stage_hub = Arc::clone(&hub);
    let mut seq = 0u64;
    let mut on_stage = move |pass: u8, event: StageEvent| {
        seq += 1;
        let mut frame = serde_json::to_value(&event).unwrap_or_else(|_| json!({}));
        frame["type"] = json!("stage");
        frame["pass"] = json!(pass);
        frame["seq"] = json!(seq);
        stage_hub.emit("stage", frame.to_string());
    };

    let delta_hub = Arc::clone(&hub);
    let mut on_delta = move |text: &str| {
        delta_hub.emit("delta", json!({ "type": "delta", "text": text }).to_string());
    };

    let result = (|| -> Result<query::QueryReport> {
        let client = state.sidecar_client()?;
        query::run_query(
            &state.workspace,
            &scope,
            &query_text,
            &model,
            parse_reasoning(&reasoning),
            client,
            &cancel,
            &mut on_stage,
            &mut on_delta,
        )
    })();

    match result {
        Ok(report) => {
            let mut payload =
                query::result_data(&state.workspace, &scope, &query_text, &model, &report, None);
            payload["type"] = json!("done");
            payload["ok"] = json!(true);
            hub.emit("done", payload.to_string());
        }
        Err(error) => {
            if cancel.load(Ordering::SeqCst) {
                hub.emit("cancelled", json!({ "type": "cancelled" }).to_string());
            } else {
                hub.emit(
                    "error",
                    json!({ "type": "error", "message": error.to_string() }).to_string(),
                );
            }
        }
    }
    hub.finish_query();
}

fn parse_reasoning(value: &str) -> Reasoning {
    match value.trim() {
        "" | "off" => Reasoning::Off,
        "default" => Reasoning::Default,
        level => Reasoning::Level(level.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn split_url(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

fn requested_scope(query_string: Option<&str>, state: &Arc<ServerState>) -> std::result::Result<String, String> {
    match query_param(query_string, "scope") {
        Some(value) => paths::normalize_scope(&value).map_err(|error| error.to_string()),
        None => Ok(state.default_scope.clone()),
    }
}

fn missing_graph_message(scope: &str) -> String {
    if scope.is_empty() {
        "no knowledge graph for the whole library — run `docfoo kg --index` first".to_string()
    } else {
        format!("no knowledge graph for \"{scope}\" — run `docfoo kg --index --scope {scope}` first")
    }
}

fn read_body(request: &mut Request) -> std::result::Result<String, String> {
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut body)
        .map_err(|error| format!("could not read the request body: {error}"))?;
    Ok(body)
}

fn query_param(query_string: Option<&str>, key: &str) -> Option<String> {
    let query_string = query_string?;
    for pair in query_string.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                if let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    out.push(high * 16 + low);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

fn json_response(status: u16, value: &Value) -> ResponseBox {
    Response::from_string(value.to_string())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
        .boxed()
}

fn error_response(status: u16, message: &str) -> ResponseBox {
    json_response(status, &json!({ "error": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_params_are_decoded() {
        let query = "scope=papers%2Fml&path=Book%2Fassets%2Ff%20x.png&flag";
        assert_eq!(query_param(Some(query), "scope").unwrap(), "papers/ml");
        assert_eq!(
            query_param(Some(query), "path").unwrap(),
            "Book/assets/f x.png"
        );
        assert_eq!(query_param(Some(query), "flag").unwrap(), "");
        assert!(query_param(Some(query), "missing").is_none());
    }

    #[test]
    fn reasoning_strings_map_to_the_shared_enum() {
        assert!(matches!(parse_reasoning("off"), Reasoning::Off));
        assert!(matches!(parse_reasoning(""), Reasoning::Off));
        assert!(matches!(parse_reasoning("default"), Reasoning::Default));
        assert!(matches!(
            parse_reasoning("high"),
            Reasoning::Level(level) if level == "high"
        ));
    }

    #[test]
    fn urls_split_cleanly() {
        assert_eq!(split_url("/api/graph?scope="), ("/api/graph", Some("scope=")));
        assert_eq!(split_url("/"), ("/", None));
    }
}
