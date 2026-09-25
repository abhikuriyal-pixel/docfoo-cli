//! KG query driver — Tauri-free port of `src-tauri/src/kg/query.rs::run_query`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use docfoo_kg::decision::DecisionClient;
use docfoo_kg::graph::KnowledgeGraph;
use docfoo_kg::llm::{ChatClient, Reasoning};
use docfoo_kg::query::{self, QueryError, StageEvent, Trace};
use serde_json::{json, Value};

use crate::error::{CliError, Result};
use crate::sidecar::SidecarClient;
use crate::workspace::Workspace;

use super::bridge::AgentChatClient;
use super::decision::JevDecisionClient;
use super::{paths, settings};

pub struct QueryReport {
    pub answer: String,
    pub trace: Trace,
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
    on_stage: &mut dyn FnMut(u8, StageEvent),
    on_delta: &mut dyn FnMut(&str),
) -> Result<QueryReport> {
    let graph_path = ensure_graph(workspace, scope)?;

    let settings = settings::load_settings(workspace);
    let tunables = settings.query;
    let mut kg = KnowledgeGraph::load(&graph_path)
        .map_err(|error| CliError::Message(format!("could not load the knowledge graph: {error}")))?;
    qualify_source_docs(&mut kg, scope);

    // Jev concept routing is best-effort: when disabled or without a key the
    // query proceeds with lexical seeds and the gate's fallback.
    let decision: Option<Arc<dyn DecisionClient>> =
        JevDecisionClient::from_settings(&tunables)
            .map(|client| Arc::new(client) as Arc<dyn DecisionClient>);
    let llm: Arc<dyn ChatClient> = Arc::new(AgentChatClient::new(sidecar, model_key, reasoning));

    let outcome = query::retrieve(
        query_text,
        &kg,
        &tunables,
        llm.as_ref(),
        decision.as_deref(),
        on_stage,
        cancel,
        on_delta,
    )
    .map_err(|error| match error {
        QueryError::Cancelled => CliError::Message("the query was cancelled".to_string()),
        QueryError::Llm(error) => CliError::Message(error.to_string()),
    })?;

    Ok(QueryReport {
        answer: outcome.answer,
        trace: outcome.trace,
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

/// The `data` payload shared by `docfoo kg --query` and the visualizer.
///
/// Keeping one builder here means the interactive page and the scriptable
/// command can never drift apart in shape, citations or figure resolution.
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
    json!({
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
    })
}

/// Subdirectory graphs store section provenance relative to the GRAPH root,
/// while everything downstream expects workspace-relative rels. Re-qualify at
/// query time; the stored graph file is left untouched and the operation is
/// idempotent. Port of `src-tauri/src/kg/query.rs::qualify_source_docs`.
pub fn qualify_source_docs(kg: &mut KnowledgeGraph, scope: &str) {
    if scope.is_empty() {
        return;
    }
    let prefix = format!("{scope}/");
    for info in kg.sections.values_mut() {
        if !info.source_doc.is_empty() && !info.source_doc.starts_with(&prefix) {
            info.source_doc = format!("{prefix}{}", info.source_doc);
        }
    }
}
