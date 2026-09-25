//! Embedded shared frontend bundle and resource-file serving.
//!
//! Every asset is compiled into the binary with `include_str!` /
//! `include_bytes!`, so the local browsers need no installation step and no
//! Node/TypeScript toolchain. The JS is plain ES modules shared between the
//! browser and the Node test suite (see `crates/docfoo-cli/tests/js/`).
//!
//! Served by both `kg --vis` and `resources --vis`:
//! `base.css` (fonts, reset, kinetic/art-deco tokens), `js/markdown.js`,
//! `js/theme.js` and the vendored KaTeX bundle (`katex.min.js`,
//! `katex.min.css`, 20 `.woff2` faces).

use std::path::{Component, Path, PathBuf};

use tiny_http::{Header, Response, ResponseBox, StatusCode};

use crate::workspace::Workspace;

struct TextAsset {
    path: &'static str,
    mime: &'static str,
    body: &'static str,
}

struct BinaryAsset {
    path: &'static str,
    mime: &'static str,
    body: &'static [u8],
}

static TEXT_ASSETS: &[TextAsset] = &[
    TextAsset {
        path: "base.css",
        mime: "text/css; charset=utf-8",
        body: include_str!("assets/css/base.css"),
    },
    TextAsset {
        path: "js/markdown.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/markdown.js"),
    },
    TextAsset {
        path: "js/md-repair.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/md-repair.js"),
    },
    TextAsset {
        path: "js/md-doc.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/md-doc.js"),
    },
    TextAsset {
        path: "vendor/marked.esm.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/vendor/marked.esm.js"),
    },
    TextAsset {
        path: "vendor/purify.min.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/vendor/purify.min.js"),
    },
    TextAsset {
        path: "js/theme.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/theme.js"),
    },
    TextAsset {
        path: "katex/katex.min.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/katex/katex.min.js"),
    },
    TextAsset {
        path: "katex/katex.min.css",
        mime: "text/css; charset=utf-8",
        body: include_str!("assets/katex/katex.min.css"),
    },
    TextAsset {
        path: "katex/contrib/auto-render.min.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/katex/contrib/auto-render.min.js"),
    },
];

static BINARY_ASSETS: &[BinaryAsset] = &[
    BinaryAsset {
        path: "katex/fonts/KaTeX_AMS-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_AMS-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Caligraphic-Bold.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Caligraphic-Bold.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Caligraphic-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Caligraphic-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Fraktur-Bold.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Fraktur-Bold.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Fraktur-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Fraktur-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Main-Bold.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Main-Bold.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Main-BoldItalic.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Main-BoldItalic.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Main-Italic.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Main-Italic.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Main-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Main-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Math-BoldItalic.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Math-BoldItalic.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Math-Italic.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Math-Italic.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_SansSerif-Bold.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_SansSerif-Bold.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_SansSerif-Italic.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_SansSerif-Italic.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_SansSerif-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_SansSerif-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Script-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Script-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Size1-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Size1-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Size2-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Size2-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Size3-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Size3-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Size4-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Size4-Regular.woff2"),
    },
    BinaryAsset {
        path: "katex/fonts/KaTeX_Typewriter-Regular.woff2",
        mime: "font/woff2",
        body: include_bytes!("assets/katex/fonts/KaTeX_Typewriter-Regular.woff2"),
    },
];

/// Serve one shared asset (text or font), or 404.
pub fn serve(path: &str) -> ResponseBox {
    if let Some(asset) = TEXT_ASSETS.iter().find(|asset| asset.path == path) {
        return Response::from_string(asset.body)
            .with_status_code(StatusCode(200))
            .with_header(header("Content-Type", asset.mime))
            .with_header(header("Cache-Control", "no-cache"))
            .boxed();
    }
    if let Some(asset) = BINARY_ASSETS.iter().find(|asset| asset.path == path) {
        return Response::from_data(asset.body)
            .with_status_code(StatusCode(200))
            .with_header(header("Content-Type", asset.mime))
            .with_header(header("Cache-Control", "no-cache"))
            .boxed();
    }
    Response::from_string("{\"error\":\"asset not found\"}")
        .with_status_code(StatusCode(404))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .boxed()
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

/// Content type for a served resource file.
pub fn mime_for(path: &Path) -> &'static str {
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
    fn shared_bundle_and_katex_are_served() {
        assert_eq!(serve("base.css").status_code().0, 200);
        assert_eq!(serve("js/markdown.js").status_code().0, 200);
        assert_eq!(serve("js/theme.js").status_code().0, 200);
        assert_eq!(serve("js/md-repair.js").status_code().0, 200);
        assert_eq!(serve("js/md-doc.js").status_code().0, 200);
        assert_eq!(serve("vendor/marked.esm.js").status_code().0, 200);
        assert_eq!(serve("vendor/purify.min.js").status_code().0, 200);
        assert_eq!(serve("nope.js").status_code().0, 404);

        let katex = serve("katex/katex.min.js");
        assert_eq!(katex.status_code().0, 200);
        assert!(katex.data_length().unwrap_or(0) > 100_000);

        let auto_render = serve("katex/contrib/auto-render.min.js");
        assert_eq!(auto_render.status_code().0, 200);
        assert!(auto_render.data_length().unwrap_or(0) > 1_000);

        let font = serve("katex/fonts/KaTeX_Main-Regular.woff2");
        assert_eq!(font.status_code().0, 200);
        assert!(font.data_length().unwrap_or(0) > 0);
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
