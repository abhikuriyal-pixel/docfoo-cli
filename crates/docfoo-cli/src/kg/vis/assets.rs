//! Embedded KG-visualizer bundle.
//!
//! Every page asset is compiled into the binary with `include_str!`, so
//! `docfoo kg --vis` needs no installation step and no Node/TypeScript
//! toolchain. The JS is plain ES modules shared between the browser and the
//! Node test suite (see `crates/docfoo-cli/tests/js/`).
//!
//! Page-specific files live here; anything shared with the resource browser
//! (theme base CSS, markdown renderer, theme picker, KaTeX) is delegated to
//! [`crate::web::assets`].

use tiny_http::{Header, Response, ResponseBox, StatusCode};

struct TextAsset {
    path: &'static str,
    mime: &'static str,
    body: &'static str,
}

static TEXT_ASSETS: &[TextAsset] = &[
    TextAsset {
        path: "index.html",
        mime: "text/html; charset=utf-8",
        body: include_str!("assets/index.html"),
    },
    TextAsset {
        path: "app.css",
        mime: "text/css; charset=utf-8",
        body: include_str!("assets/app.css"),
    },
    TextAsset {
        path: "js/graph.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/graph.js"),
    },
    TextAsset {
        path: "js/layout.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/layout.js"),
    },
    TextAsset {
        path: "js/state.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/state.js"),
    },
    TextAsset {
        path: "js/choreo.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/choreo.js"),
    },
    TextAsset {
        path: "js/renderer.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/renderer.js"),
    },
    TextAsset {
        path: "js/models.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/models.js"),
    },
    TextAsset {
        path: "js/app.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/app.js"),
    },
];

/// Serve one embedded KG asset, or a shared asset, or 404.
pub fn serve(path: &str) -> ResponseBox {
    if let Some(asset) = TEXT_ASSETS.iter().find(|asset| asset.path == path) {
        return Response::from_string(asset.body)
            .with_status_code(StatusCode(200))
            .with_header(header("Content-Type", asset.mime))
            .with_header(header("Cache-Control", "no-cache"))
            .boxed();
    }
    crate::web::assets::serve(path)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bundle_and_shared_assets_are_served() {
        assert_eq!(serve("index.html").status_code().0, 200);
        assert_eq!(serve("app.css").status_code().0, 200);
        assert_eq!(serve("nope.js").status_code().0, 404);

        // Shared assets and KaTeX resolve through the delegation.
        assert_eq!(serve("base.css").status_code().0, 200);
        assert_eq!(serve("js/markdown.js").status_code().0, 200);
        let katex = serve("katex/katex.min.js");
        assert_eq!(katex.status_code().0, 200);
        assert!(katex.data_length().unwrap_or(0) > 100_000);
    }
}
