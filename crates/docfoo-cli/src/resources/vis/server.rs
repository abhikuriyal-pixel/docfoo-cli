//! HTTP routing for `resources --vis`.
//!
//! The server binds loopback only. Routes:
//!
//! | Method | Path               | Purpose                                        |
//! |--------|--------------------|------------------------------------------------|
//! | GET    | `/`                | embedded resource browser page                 |
//! | GET    | `/app.css`         | embedded stylesheet                            |
//! | GET    | `/js/*`, `/base.css`, `/katex/*` | embedded modules / shared assets  |
//! | GET    | `/api/state`       | workspace root and starting folder             |
//! | GET    | `/api/resources`   | one folder level for `?rel=`                   |
//! | GET    | `/api/resource`    | markdown/text for `?path=`                     |
//! | GET    | `/api/asset`       | image bytes under `resources/` for `?path=`    |
//! | GET    | `/api/notes`       | notes for `?path=`                             |
//! | POST   | `/api/notes`       | create or edit a note `{rel, note}`            |
//! | POST   | `/api/notes/delete`| delete a note `{rel, id}`                      |
//!
//! Every request runs on its own thread, exactly like `kg --vis`, and all
//! writes go through the desktop app's `notes.json` schema.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Method, Request, Response, ResponseBox, StatusCode};

use crate::error::CliError;
use crate::resources::read;
use crate::web::http::{error_response, header, json_response, query_param, read_body, split_url};
use crate::workspace::Workspace;

use super::assets;

/// Per-request body ceiling (notes are small).
const MAX_BODY_BYTES: u64 = 256 * 1024;
/// Markdown larger than this is not sent to the browser.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
/// Upper bound on cover figures sent per folder card.
const MAX_COVER_FIGURES: usize = 64;

pub struct ServerState {
    pub workspace: Workspace,
    /// Folder the page opens on; empty = the library root.
    pub start_rel: String,
    /// Serializes read-modify-write cycles on `notes.json`.
    notes_lock: Mutex<()>,
}

impl ServerState {
    pub fn new(workspace: Workspace, start_rel: String) -> Self {
        Self {
            workspace,
            start_rel,
            notes_lock: Mutex::new(()),
        }
    }
}

/// Accept loop. Each request gets its own thread so a slow read never blocks
/// the rest of the page.
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
        (Method::Get, other)
            if other.starts_with("/js/")
                || other.starts_with("/katex/")
                || other.starts_with("/vendor/")
                || other == "/base.css" =>
        {
            assets::serve(other.trim_start_matches('/'))
        }

        (Method::Get, "/api/state") => api_state(state),
        (Method::Get, "/api/resources") => api_resources(query_string, state),
        (Method::Get, "/api/resource") => api_resource(query_string, state),
        (Method::Get, "/api/asset") => api_asset(query_string, state),
        (Method::Get, "/api/notes") => api_notes_get(query_string, state),

        (Method::Post, "/api/notes") => api_notes_post(request, state),
        (Method::Post, "/api/notes/delete") => api_notes_delete(request, state),

        _ => error_response(404, "not found"),
    }
}

// ---------------------------------------------------------------------------
// API handlers

fn api_state(state: &Arc<ServerState>) -> ResponseBox {
    json_response(
        200,
        &json!({
            "workspace": state.workspace.root.display().to_string(),
            "resources": state.workspace.resources_dir().display().to_string(),
            "rel": state.start_rel,
        }),
    )
}

/// One folder level: folders first, then files, with cover figures for folders
/// and the same hidden-entry rules as the desktop browser.
fn api_resources(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let rel = query_param(query_string, "rel").unwrap_or_default();
    let root = state.workspace.resources_dir();
    let listing = match read::list_level(&root, &rel, false) {
        Ok(listing) => listing,
        Err(error) => return error_response(404, &error.to_string()),
    };
    let entries: Vec<Value> = listing
        .entries
        .iter()
        .filter(|entry| visible(&entry.name))
        .map(|entry| {
            let covers = if entry.kind == "dir" {
                cover_figures(&root, &entry.rel)
            } else {
                Vec::new()
            };
            json!({
                "name": entry.name,
                "rel": entry.rel,
                "kind": entry.kind,
                "size": entry.size,
                "files": entry.files,
                "md": entry.md,
                "figures": entry.figures,
                "lines": entry.lines,
                "covers": covers,
            })
        })
        .collect();
    json_response(
        200,
        &json!({
            "rel": rel,
            "entries": entries,
            "total": entries.len(),
            "truncated": listing.truncated,
        }),
    )
}

/// Markdown source for the reader. Bounded so a huge file cannot lock the tab.
fn api_resource(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let Some(rel) = query_param(query_string, "path") else {
        return error_response(400, "missing path");
    };
    let root = state.workspace.resources_dir();
    let (rel, path) = match read::resolve_rel(&root, &rel) {
        Ok(resolved) => resolved,
        Err(error) => return error_response(404, &error.to_string()),
    };
    if !path.is_file() {
        return error_response(404, "resource not found");
    }
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if size > MAX_TEXT_BYTES {
        return json_response(
            413,
            &json!({
                "error": "resource is too large to display in the browser",
                "rel": rel,
                "size": size,
                "tooLarge": true,
            }),
        );
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let kind = read::kind_of(&name);
    match read::read_text(&path) {
        Ok(text) => json_response(
            200,
            &json!({ "rel": rel, "name": name, "kind": kind, "size": size, "text": text }),
        ),
        Err(error) => error_response(415, &error.to_string()),
    }
}

fn api_asset(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let Some(rel) = query_param(query_string, "path") else {
        return error_response(400, "missing path");
    };
    match crate::web::assets::resolve_resource(&state.workspace, &rel) {
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

fn api_notes_get(query_string: Option<&str>, state: &Arc<ServerState>) -> ResponseBox {
    let Some(rel) = query_param(query_string, "path") else {
        return error_response(400, "missing path");
    };
    let (rel, path) = match read::resolve_rel(&state.workspace.resources_dir(), &rel) {
        Ok(resolved) => resolved,
        Err(error) => return error_response(404, &error.to_string()),
    };
    if !path.is_file() {
        return error_response(404, "resource not found");
    }
    match crate::notes::load(&state.workspace) {
        Ok(map) => json_response(
            200,
            &json!({
                "rel": rel,
                "notes": map.get(&rel).cloned().unwrap_or_else(|| Value::Array(Vec::new())),
            }),
        ),
        Err(error) => error_response(500, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct NoteBody {
    rel: String,
    note: Value,
}

fn api_notes_post(request: &mut Request, state: &Arc<ServerState>) -> ResponseBox {
    let body = match read_body(request, MAX_BODY_BYTES) {
        Ok(body) => body,
        Err(message) => return error_response(400, &message),
    };
    let parsed: NoteBody = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(error) => return error_response(400, &format!("invalid note body: {error}")),
    };
    let _guard = state.notes_lock.lock().expect("notes lock");
    match crate::notes::upsert(&state.workspace, &parsed.rel, &parsed.note) {
        Ok(notes) => json_response(200, &json!({ "ok": true, "rel": parsed.rel, "notes": notes })),
        Err(error) => error_response(status_for(&error), &error.to_string()),
    }
}

#[derive(Deserialize)]
struct NoteDeleteBody {
    rel: String,
    id: String,
}

fn api_notes_delete(request: &mut Request, state: &Arc<ServerState>) -> ResponseBox {
    let body = match read_body(request, MAX_BODY_BYTES) {
        Ok(body) => body,
        Err(message) => return error_response(400, &message),
    };
    let parsed: NoteDeleteBody = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(error) => return error_response(400, &format!("invalid note body: {error}")),
    };
    let _guard = state.notes_lock.lock().expect("notes lock");
    match crate::notes::delete(&state.workspace, &parsed.rel, &parsed.id) {
        Ok(notes) => json_response(200, &json!({ "ok": true, "rel": parsed.rel, "notes": notes })),
        Err(error) => error_response(status_for(&error), &error.to_string()),
    }
}

/// Same hidden-entry rules as the desktop browser.
fn visible(name: &str) -> bool {
    !name.eq_ignore_ascii_case("assets")
        && !name.eq_ignore_ascii_case("notebooks")
        && name != "ocr_cache.json"
        && !name.to_ascii_lowercase().ends_with(".jsonl")
}

/// Cover candidates for a folder card: images under its own `assets/`, or —
/// when it is a folder of resources — images from its children's `assets/`.
/// Capped so a huge subtree cannot inflate the listing response.
fn cover_figures(root: &Path, rel: &str) -> Vec<String> {
    let dir = root.join(rel);
    let mut out = Vec::new();
    collect_images(&dir.join("assets"), &format!("{rel}/assets"), &mut out);
    if out.is_empty() {
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                if out.len() >= MAX_COVER_FIGURES {
                    break;
                }
                if !entry.metadata().map(|meta| meta.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                collect_images(
                    &entry.path().join("assets"),
                    &format!("{rel}/{name}/assets"),
                    &mut out,
                );
            }
        }
    }
    out
}

fn collect_images(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((current, rel_prefix)) = stack.pop() {
        if out.len() >= MAX_COVER_FIGURES {
            return;
        }
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            if out.len() >= MAX_COVER_FIGURES {
                return;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, format!("{rel_prefix}/{name}")));
            } else if read::is_image(&name) {
                out.push(format!("{rel_prefix}/{name}"));
            }
        }
    }
}

fn status_for(error: &CliError) -> u16 {
    match error {
        CliError::NotFound(_) => 404,
        CliError::Usage(_) => 400,
        CliError::Message(_) | CliError::NotImplemented(_) => 400,
        CliError::Io(_) | CliError::Json(_) => 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_prefer_own_assets_then_children() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::create_dir_all(root.join("Book/assets")).unwrap();
        std::fs::write(root.join("Book/assets/f1.png"), b"png").unwrap();
        std::fs::write(root.join("Book/assets/note.txt"), b"txt").unwrap();
        // A child's figures must not leak into a folder that has its own.
        std::fs::create_dir_all(root.join("Book/Sub/assets")).unwrap();
        std::fs::write(root.join("Book/Sub/assets/f2.png"), b"png").unwrap();
        let own = cover_figures(root, "Book");
        assert_eq!(own, vec!["Book/assets/f1.png"]);

        std::fs::create_dir_all(root.join("Shelf/A/assets")).unwrap();
        std::fs::write(root.join("Shelf/A/assets/a.png"), b"png").unwrap();
        std::fs::create_dir_all(root.join("Shelf/B/assets")).unwrap();
        std::fs::write(root.join("Shelf/B/assets/b.jpg"), b"jpg").unwrap();
        let children = cover_figures(root, "Shelf");
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|rel| rel.ends_with("a.png")));
        assert!(children.iter().any(|rel| rel.ends_with("b.jpg")));
        assert!(cover_figures(root, "Empty").is_empty());
    }

    #[test]
    fn hidden_entries_match_the_desktop_browser() {
        assert!(!visible("assets"));
        assert!(!visible("Notebooks"));
        assert!(!visible("ocr_cache.json"));
        assert!(!visible("cache.jsonl"));
        assert!(visible("content.md"));
        assert!(visible("ocr.md"));
        assert!(visible("figure.png"));
    }
}
