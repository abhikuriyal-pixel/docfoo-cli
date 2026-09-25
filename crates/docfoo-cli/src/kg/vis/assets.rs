//! Embedded frontend bundle and resource-file serving.
//!
//! Every page asset is compiled into the binary with `include_str!`, so
//! `docfoo kg --vis` needs no installation step and no Node/TypeScript
//! toolchain. The JS is plain ES modules shared between the browser and the
//! Node test suite (see `crates/docfoo-cli/tests/js/`).

use std::path::{Component, Path, PathBuf};

use tiny_http::{Header, Response, ResponseBox, StatusCode};

use crate::workspace::Workspace;

struct Asset {
    path: &'static str,
    mime: &'static str,
    body: &'static str,
}

static ASSETS: &[Asset] = &[
    Asset {
        path: "index.html",
        mime: "text/html; charset=utf-8",
        body: include_str!("assets/index.html"),
    },
    Asset {
        path: "app.css",
        mime: "text/css; charset=utf-8",
        body: include_str!("assets/app.css"),
    },
    Asset {
        path: "js/graph.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/graph.js"),
    },
    Asset {
        path: "js/layout.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/layout.js"),
    },
    Asset {
        path: "js/state.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/state.js"),
    },
    Asset {
        path: "js/choreo.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/choreo.js"),
    },
    Asset {
        path: "js/renderer.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/renderer.js"),
    },
    Asset {
        path: "js/markdown.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/markdown.js"),
    },
    Asset {
        path: "js/models.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/models.js"),
    },
    Asset {
        path: "js/theme.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/theme.js"),
    },
    Asset {
        path: "js/app.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/app.js"),
    },
];

/// Serve one embedded asset, or 404.
pub fn serve(path: &str) -> ResponseBox {
    match ASSETS.iter().find(|asset| asset.path == path) {
        Some(asset) => Response::from_string(asset.body)
            .with_status_code(StatusCode(200))
            .with_header(header("Content-Type", asset.mime))
            .with_header(header("Cache-Control", "no-cache"))
            .boxed(),
        None => Response::from_string("{\"error\":\"asset not found\"}")
            .with_status_code(StatusCode(404))
            .with_header(header("Content-Type", "application/json; charset=utf-8"))
            .boxed(),
    }
}

/// Resolve `rel` inside `<workspace>/resources`, refusing anything that
/// escapes the library (absolute paths, drive prefixes, `..`, symlink
/// escapes). Returns the canonicalized path and its content type.
pub fn resolve_resource(workspace: &Workspace, rel: &str) -> Option<(PathBuf, &'static str)> {
    if rel.trim().is_empty() {
        return None;
    }
    let normalized = rel.replace('\\', "/");
    let relative = Path::new(&normalized);
    if relative.is_absolute()
        || normalized.contains(':')
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }

    let resources = workspace.resources_dir().canonicalize().ok()?;
    let candidate = resources.join(relative).canonicalize().ok()?;
    if !candidate.starts_with(&resources) || !candidate.is_file() {
        return None;
    }
    let mime = mime_for(&candidate);
    Some((candidate, mime))
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(root: &Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            agent_dir: root.join(".agent"),
            models_dir: root.join("models"),
        }
    }

    #[test]
    fn embedded_bundle_is_served() {
        let response = serve("index.html");
        assert_eq!(response.status_code().0, 200);
        assert!(serve("nope.js").status_code().0 == 404);
    }

    #[test]
    fn resources_resolve_but_escapes_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("resources");
        std::fs::create_dir_all(resources.join("Book/assets")).unwrap();
        std::fs::write(resources.join("Book/assets/f.png"), b"png").unwrap();
        std::fs::write(temp.path().join("secret.txt"), b"nope").unwrap();
        let workspace = workspace(temp.path());

        let (path, mime) = resolve_resource(&workspace, "Book/assets/f.png").unwrap();
        assert!(path.ends_with("f.png"));
        assert_eq!(mime, "image/png");

        assert!(resolve_resource(&workspace, "../secret.txt").is_none());
        assert!(resolve_resource(&workspace, "/etc/passwd").is_none());
        assert!(resolve_resource(&workspace, "Book/../../../secret.txt").is_none());
        assert!(resolve_resource(&workspace, "").is_none());
        assert!(resolve_resource(&workspace, "Book/missing.png").is_none());
    }
}
