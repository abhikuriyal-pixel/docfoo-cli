//! Community collections: list and download shared resources/graphs.
//!
//! Only the read path is ported (list, info, download + install). Uploads and
//! Koofr credential handling stay desktop-only.

pub mod client;
pub mod zip_util;

/// Built-in DocFoo Collections server (the "common platform"). The URL is
/// intentionally hardcoded, matching the desktop app.
pub const DEFAULT_COLLECTIONS_URL: &str = "https://docfoo-community.docfoo-work.workers.dev";

pub fn server_url() -> String {
    DEFAULT_COLLECTIONS_URL.trim_end_matches('/').to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CollectionItem {
    pub id: String,
    /// "resource" | "kg"
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub files: u64,
    #[serde(rename = "createdAt", default)]
    pub created_at: u64,
    #[serde(default)]
    pub uploader: String,
    #[serde(default)]
    pub downloads: u64,
}
