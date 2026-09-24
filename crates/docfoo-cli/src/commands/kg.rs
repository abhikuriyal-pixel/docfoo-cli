//! `docfoo kg` — knowledge-graph index, query and status.
//!
//! `--query` is a one-shot final answer: retrieval + exactly one synthesis
//! call, no agent loop. `--index` builds or refreshes a graph. `--status`
//! reports what is built.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use docfoo_kg::build::Progress;
use docfoo_kg::graph::KnowledgeGraph;
use docfoo_kg::llm::Reasoning;
use serde_json::{json, Value};

use crate::citations;
use crate::cli::{Cli, KgArgs};
use crate::config::ModelPreferences;
use crate::error::{CliError, Result};
use crate::kg::{self, paths};
use crate::output::{self, OutputFormat};
use crate::render::{self, SlackOptions};
use crate::sidecar::Sidecar;
use crate::workspace::Workspace;

pub fn run(cli: &Cli, format: OutputFormat, workspace: &Workspace, args: &KgArgs) -> Result<()> {
    let scope = paths::normalize_scope(&args.scope)?;
    if args.index {
        return run_index(cli, format, workspace, args, &scope);
    }
    if args.status {
        return run_status(format, workspace, &scope);
    }
    if let Some(query) = &args.query {
        return run_query(cli, format, workspace, args, &scope, query);
    }
    Err(CliError::Usage(
        "kg needs --query, --index or --status".to_string(),
    ))
}

fn run_index(
    cli: &Cli,
    format: OutputFormat,
    workspace: &Workspace,
    args: &KgArgs,
    scope: &str,
) -> Result<()> {
    let model = resolve_model(workspace, args)?;
    let settings = kg::settings::load_settings(workspace);
    let reasoning = resolve_reasoning(args, settings.query.reasoning);
    let cancel = cancel_flag();
    let mut sidecar = Sidecar::locate(workspace)?;
    let client = sidecar.client()?;
    workspace.ensure_scaffold()?;

    let quiet = cli.quiet;
    let progress = move |event: Progress| {
        if quiet {
            return;
        }
        match event {
            Progress::Started {
                docs,
                total_sections,
            } => eprintln!("indexing {docs} document(s), {total_sections} section(s)…"),
            Progress::Sync {
                added,
                changed,
                removed,
                unchanged,
            } => eprintln!("sync: +{added} ~{changed} -{removed} ={unchanged}"),
            Progress::SectionDone {
                done,
                total,
                title,
                entities,
                relations,
            } => eprintln!("[{done}/{total}] {title} — {entities} entities, {relations} relations"),
            Progress::Warn(message) => eprintln!("warning: {message}"),
            Progress::Phase(phase) => eprintln!("{phase}"),
        }
    };

    let report = kg::index::run_index(
        workspace, scope, &model, reasoning, args.fresh, client, &cancel, progress,
    )?;

    let target = if scope.is_empty() {
        "Resources — top level".to_string()
    } else {
        scope.to_string()
    };
    let message = format!(
        "indexed {} document(s) into {target}: {} entities, {} relations, {} sections in {:.1}s",
        report.docs,
        report.stats.entities,
        report.stats.relations,
        report.stats.sections,
        report.stats.elapsed_secs
    );
    let data = json!({
        "scope": scope,
        "graphPath": report.graph_path.display().to_string(),
        "docs": report.docs,
        "entities": report.stats.entities,
        "relations": report.stats.relations,
        "sections": report.stats.sections,
        "topics": report.stats.topics,
        "extracted": report.stats.extracted_this_run,
        "skipped": report.stats.skipped_resume,
        "crossDocMerges": report.stats.cross_doc_merges,
        "typeMerges": report.stats.type_merges,
        "noiseFloor": report.stats.noise_floor,
        "elapsedSecs": report.stats.elapsed_secs,
        "fresh": report.fresh,
        "vocabChanged": report.vocab_changed,
        "message": message,
    });
    output::success(format, "kg.index", &workspace.root.display().to_string(), data)
}

fn run_status(format: OutputFormat, workspace: &Workspace, scope: &str) -> Result<()> {
    let graph_path = paths::graph_path(workspace, scope);
    let exists = graph_path.is_file();
    let built = paths::list_built(workspace);
    let (entities, relations, sections) = if exists {
        let graph = KnowledgeGraph::load(&graph_path)
            .map_err(|error| CliError::Message(format!("could not load the knowledge graph: {error}")))?;
        (graph.entities.len(), graph.relations.len(), graph.sections.len())
    } else {
        (0, 0, 0)
    };
    let target = if scope.is_empty() {
        "Resources — top level".to_string()
    } else {
        format!("\"{scope}\"")
    };
    let message = if exists {
        format!("{target}: {entities} entities, {relations} relations, {sections} sections")
    } else {
        format!("{target} is not indexed — run `docfoo kg --index --scope {scope}`")
    };
    let data = json!({
        "scope": scope,
        "exists": exists,
        "graphPath": graph_path.display().to_string(),
        "built": built,
        "entities": entities,
        "relations": relations,
        "sections": sections,
        "message": message,
    });
    output::success(format, "kg.status", &workspace.root.display().to_string(), data)
}

fn run_query(
    cli: &Cli,
    format: OutputFormat,
    workspace: &Workspace,
    args: &KgArgs,
    scope: &str,
    query: &str,
) -> Result<()> {
    let query = query.trim();
    if query.is_empty() {
        return Err(CliError::Usage("--query needs a question".to_string()));
    }
    // Report a missing graph before model selection: indexing is the real
    // prerequisite and the hint is more useful than "no model selected".
    kg::query::ensure_graph(workspace, scope)?;
    let model = resolve_model(workspace, args)?;
    let settings = kg::settings::load_settings(workspace);
    let reasoning = resolve_reasoning(args, settings.query.reasoning);
    let cancel = cancel_flag();
    let mut sidecar = Sidecar::locate(workspace)?;
    let client = sidecar.client()?;

    let stream = args.stream;
    let mut on_delta = |delta: &str| {
        if stream {
            eprint!("{delta}");
        }
    };
    let report = kg::query::run_query(
        workspace, scope, query, &model, reasoning, client, &cancel, &mut on_delta,
    )?;
    if stream {
        eprintln!();
    }

    let answer = report.answer;
    let citations = citations::tokenize(&answer);
    let sources: Vec<Value> = serde_json::to_value(&report.trace.evidence_sections)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let figures = render::extract_figures(&answer, &workspace.resources_dir());
    let tables = render::extract_tables(&answer);
    let routing = serde_json::to_value(&report.trace.routing).unwrap_or_else(|_| json!({}));

    let saved = if args.save {
        let saved = kg::chats::save_query(
            workspace,
            query,
            &answer,
            &Value::Array(sources.clone()),
            &routing,
            report.trace.total_seconds,
        )?;
        Some(saved.id)
    } else {
        None
    };

    let data = json!({
        "query": query,
        "scope": scope,
        "model": model,
        "answer_markdown": answer,
        "citations": citations,
        "figures": figures,
        "tables": tables,
        "sources": sources,
        "routing": routing,
        "depth": report.trace.depth,
        "guides": report.trace.guides,
        "droppedCount": report.trace.budget_dropped,
        "triples": report.trace.triples_used,
        "timings": report.trace.timings,
        "totalSecs": report.trace.total_seconds,
        "saved": saved,
    });

    match format {
        OutputFormat::Json => {
            output::success(format, "kg.query", &workspace.root.display().to_string(), data)
        }
        OutputFormat::Markdown => {
            println!("{answer}");
            Ok(())
        }
        OutputFormat::Slack => {
            let options = SlackOptions {
                plain_tables: args.plain_tables,
                include_sources: !args.no_sources,
                quote_sources: args.quote_sources,
                max_chars: args.max_chars,
                hermes_final: args.hermes_final,
                resources_dir: &workspace.resources_dir(),
            };
            let rendered = render::render_slack(&answer, &sources, &options);
            print!("{rendered}");
            if !rendered.ends_with('\n') {
                println!();
            }
            let _ = cli; // reserved for future verbosity flags
            Ok(())
        }
    }
}

fn resolve_model(workspace: &Workspace, args: &KgArgs) -> Result<String> {
    if let Some(model) = &args.model {
        if !model.trim().is_empty() {
            return Ok(model.trim().to_string());
        }
    }
    let prefs = ModelPreferences::load(workspace);
    if let Some(key) = prefs.get("kg")? {
        return Ok(key.to_string());
    }
    if let Some(key) = prefs.get("chat")? {
        return Ok(key.to_string());
    }
    Err(CliError::Message(
        "no model selected — pass --model provider/model or run `docfoo model --set kg provider/model`"
            .to_string(),
    ))
}

fn resolve_reasoning(args: &KgArgs, tunables_reasoning: bool) -> Reasoning {
    match args.reasoning.as_deref() {
        Some("off") => Reasoning::Off,
        Some("default") => Reasoning::Default,
        Some(level) if !level.trim().is_empty() => Reasoning::Level(level.trim().to_string()),
        _ => {
            if tunables_reasoning {
                Reasoning::Default
            } else {
                Reasoning::Off
            }
        }
    }
}

/// Ctrl-C flips the pipeline's cancel flag; the KG crate saves partial work on
/// cancellation and the sidecar aborts in-flight completions.
fn cancel_flag() -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    });
    cancel
}
