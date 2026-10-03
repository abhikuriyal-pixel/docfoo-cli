//! KG query driver — Tauri-free port of `src-tauri/src/kg/query.rs::run_query`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use docfoo_kg::decision::DecisionClient;
use docfoo_kg::llm::{ChatClient, Reasoning};
use docfoo_kg::query::{self, QueryError, StageEvent, Trace};
use docfoo_kg::store::{KgStore, OpenMode};
use serde_json::{json, Value};

use crate::error::{CliError, Result};
use crate::sidecar::SidecarClient;
use crate::workspace::Workspace;

use super::bridge::AgentChatClient;
use super::decision::JevDecisionClient;
use super::{paths, settings, snapshot};

pub struct QueryReport {
    pub answer: String,
    pub trace: Trace,
    pub replay: Option<Value>,
    pub replay_error: Option<String>,
}

/// Fail early with an actionable message when the scope has no graph.
pub fn ensure_graph(workspace: &Workspace, scope: &str) -> Result<std::path::PathBuf> {
    let graph_path = paths::graph_path(workspace, scope);
    if !graph_path.is_file() {
        let target = if scope.is_empty() {
            "the whole library".to_string()
        } else {
            format!("\"{scope}\"")
        };
        return Err(CliError::NotFound(format!(
            "no knowledge graph for {target} — run `docfoo kg --index` first"
        )));
    }
    Ok(graph_path)
}

#[allow(clippy::too_many_arguments)]
pub fn run_query(
    workspace: &Workspace,
    scope: &str,
    query_text: &str,
    model_key: &str,
    reasoning: Reasoning,
    sidecar: Arc<SidecarClient>,
    cancel: &AtomicBool,
    snapshot: bool,
    on_stage: &mut dyn FnMut(u8, StageEvent),
    on_delta: &mut dyn FnMut(&str),
) -> Result<QueryReport> {
    let graph_path = ensure_graph(workspace, scope)?;

    let settings = settings::load_settings(workspace);
    let tunables = settings.query;
    // Section provenance is stored relative to the graph root; the store
    // re-qualifies it to a workspace-relative path on read, which is what the
    // citation/evidence layer downstream expects.
    let store = KgStore::open(&graph_path, OpenMode::ReadOnly)
        .map_err(|error| CliError::Message(format!("could not open the knowledge graph: {error}")))?
        .with_source_prefix(scope);
    // Pin one read snapshot throughout retrieval and export, including in-place
    // WAL rebuilds by another application. Drop rolls back on query errors.
    let _read =
        if snapshot {
            Some(store.connection().unchecked_transaction().map_err(|e| {
                CliError::Message(format!("could not pin the knowledge graph: {e}"))
            })?)
        } else {
            None
        };
    if !store.derived_ready().map_err(|error| {
        CliError::Message(format!("could not read the knowledge graph: {error}"))
    })? {
        return Err(CliError::Message(
            "The knowledge graph is still being built — finish indexing before asking questions."
                .to_string(),
        ));
    }

    // Jev concept routing is best-effort: when disabled or without a key the
    // query proceeds with lexical seeds and the gate's fallback.
    let decision: Option<Arc<dyn DecisionClient>> = JevDecisionClient::from_settings(&tunables)
        .map(|client| Arc::new(client) as Arc<dyn DecisionClient>);
    let llm: Arc<dyn ChatClient> = Arc::new(AgentChatClient::new(sidecar, model_key, reasoning));

    let mut capture = snapshot::Capture::default();
    let mut sink = |pass, event: StageEvent| {
        if snapshot {
            capture.push(pass, &event);
        }
        on_stage(pass, event);
    };
    let outcome = query::retrieve(
        query_text,
        &store,
        &tunables,
        llm.as_ref(),
        decision.as_deref(),
        &mut sink,
        cancel,
        on_delta,
    )
    .map_err(|error| match error {
        QueryError::Cancelled => CliError::Message("the query was cancelled".to_string()),
        QueryError::Llm(error) => CliError::Message(error.to_string()),
        QueryError::Store(error) => {
            CliError::Message(format!("could not read the knowledge graph: {error}"))
        }
    })?;

    // Visualization is best-effort; never discard a successfully synthesized
    // answer because export failed or exceeded its separate byte budget.
    let (replay, replay_error) = if snapshot {
        match snapshot::build(&store, scope, &outcome.trace, &capture) {
            Ok(value) if snapshot::payload_fits(&value) => (Some(value), None),
            Ok(_) => (
                None,
                Some("Graph replay exceeded its text or 4 MB safety limit.".into()),
            ),
            Err(error) => (
                None,
                Some(format!("Graph replay could not be exported: {error}")),
            ),
        }
    } else {
        (None, None)
    };
    Ok(QueryReport {
        answer: outcome.answer,
        trace: outcome.trace,
        replay,
        replay_error,
    })
}

/// Evidence sources from a trace, as the `sources` envelope array.
pub fn sources_value(trace: &Trace) -> Vec<Value> {
    serde_json::to_value(&trace.evidence_sections)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

/// Jev/System One routing decision from a trace, as a JSON object.
pub fn routing_value(trace: &Trace) -> Value {
    serde_json::to_value(&trace.routing).unwrap_or_else(|_| json!({}))
}

/// The `data` payload for `docfoo kg --query`.
///
/// One builder keeps the command's shape, citations and figure resolution in
/// one place.
pub fn result_data(
    workspace: &Workspace,
    scope: &str,
    query: &str,
    model: &str,
    report: &QueryReport,
    saved: Option<&str>,
) -> Value {
    let answer = &report.answer;
    let citations = crate::citations::tokenize(answer);
    let figures = crate::render::extract_figures(answer, &workspace.resources_dir());
    let tables = crate::render::extract_tables(answer);
    let mut data = json!({
        "query": query,
        "scope": scope,
        "model": model,
        "answer_markdown": answer,
        "citations": citations,
        "figures": figures,
        "tables": tables,
        "sources": sources_value(&report.trace),
        "routing": routing_value(&report.trace),
        "depth": report.trace.depth,
        "guides": report.trace.guides,
        "droppedCount": report.trace.budget_dropped,
        "triples": report.trace.triples_used,
        "timings": report.trace.timings,
        "totalSecs": report.trace.total_seconds,
        "saved": saved,
    });
    if let Some(replay) = &report.replay {
        data["replay"] = replay.clone();
    }
    if let Some(error) = &report.replay_error {
        data["replayError"] = json!(error);
    }
    data
}
