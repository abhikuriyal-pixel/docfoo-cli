//! KG indexing driver — Tauri-free port of `src-tauri/src/kg/mod.rs::kg_index`.
//!
//! Walks the selected resource folder, runs `docfoo_kg::build::run` with the
//! sidecar-backed `ChatClient`, and saves the vocabulary fingerprint on
//! success. Progress is reported through the caller's closure.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use docfoo_kg::build::{self, IndexOptions, IndexStats, Progress};
use docfoo_kg::llm::{ChatClient, Reasoning};

use crate::error::{CliError, Result};
use crate::sidecar::SidecarClient;
use crate::workspace::Workspace;

use super::bridge::AgentChatClient;
use super::{paths, settings};

pub struct IndexReport {
    pub stats: IndexStats,
    pub graph_path: PathBuf,
    pub scope: String,
    pub docs: usize,
    pub fresh: bool,
    pub vocab_changed: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_index(
    workspace: &Workspace,
    scope: &str,
    model_key: &str,
    reasoning: Reasoning,
    fresh: bool,
    sidecar: Arc<SidecarClient>,
    cancel: &AtomicBool,
    progress: impl FnMut(Progress) + Send,
) -> Result<IndexReport> {
    let resources = workspace.resources_dir();
    let corpus_root = if scope.is_empty() {
        resources
    } else {
        resources.join(scope)
    };
    let docs = docfoo_kg::corpus::load_documents(&corpus_root);
    if docs.is_empty() {
        return Err(CliError::Message(
            "nothing to index — scan a document or add files to resources/ first".to_string(),
        ));
    }

    let settings = settings::load_settings(workspace);
    let graph_path = paths::graph_path(workspace, scope);
    let current_fingerprint = settings::vocab_fingerprint(&settings.index);
    // Vocabulary changed since the last build ⇒ a fresh rebuild is required
    // (entity/relation ids are schema-locked to the vocab).
    let vocab_changed = !settings.index.vocab_fingerprint.is_empty()
        && settings.index.vocab_fingerprint != current_fingerprint
        && graph_path.exists();
    let fresh = fresh || vocab_changed;
    if let Some(parent) = graph_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let llm: Arc<dyn ChatClient> =
        Arc::new(AgentChatClient::new(sidecar, model_key, reasoning));
    let opts = IndexOptions {
        llm,
        workers: docfoo_kg::config::BUILD_CONCURRENCY,
        fresh,
        graph_path: graph_path.clone(),
        settings: settings.index,
    };
    let docs_len = docs.len();
    let stats = build::run(&docs, &opts, progress, cancel).map_err(|error| match error {
        docfoo_kg::KgError::Cancelled => CliError::Message(
            "indexing cancelled — everything completed so far was saved".to_string(),
        ),
        other => CliError::Message(other.to_string()),
    })?;
    settings::save_vocab_fingerprint(workspace, &current_fingerprint);

    Ok(IndexReport {
        stats,
        graph_path,
        scope: scope.to_string(),
        docs: docs_len,
        fresh,
        vocab_changed,
    })
}
