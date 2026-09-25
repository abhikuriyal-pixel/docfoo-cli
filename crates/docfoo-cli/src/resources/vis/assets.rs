//! Embedded resource-browser bundle.
//!
//! Every page asset is compiled into the binary with `include_str!`, so
//! `docfoo resources --vis` needs no installation step and no Node/TypeScript
//! toolchain. Shared files (theme base CSS, markdown renderer, theme picker,
//! KaTeX) are delegated to [`crate::web::assets`], exactly like `kg --vis`.

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
        path: "js/api.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/api.js"),
    },
    TextAsset {
        path: "js/covers.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/covers.js"),
    },
    TextAsset {
        path: "js/anchor.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/anchor.js"),
    },
    TextAsset {
        path: "js/browser.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/browser.js"),
    },
    TextAsset {
        path: "js/tabs.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/tabs.js"),
    },
    TextAsset {
        path: "js/reader.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/reader.js"),
    },
    TextAsset {
        path: "js/notes.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/notes.js"),
    },
    TextAsset {
        path: "js/app.js",
        mime: "text/javascript; charset=utf-8",
        body: include_str!("assets/js/app.js"),
    },
];

/// Serve one embedded page asset, or a shared asset, or 404.
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
    fn page_and_shared_assets_are_served() {
        assert_eq!(serve("index.html").status_code().0, 200);
        assert_eq!(serve("app.css").status_code().0, 200);
        assert_eq!(serve("js/app.js").status_code().0, 200);
        assert_eq!(serve("nope.js").status_code().0, 404);

        // Shared assets resolve through the delegation.
        assert_eq!(serve("base.css").status_code().0, 200);
        assert_eq!(serve("js/markdown.js").status_code().0, 200);
        assert_eq!(serve("js/theme.js").status_code().0, 200);
        let katex = serve("katex/katex.min.js");
        assert_eq!(katex.status_code().0, 200);
        assert!(katex.data_length().unwrap_or(0) > 100_000);
    }
}
