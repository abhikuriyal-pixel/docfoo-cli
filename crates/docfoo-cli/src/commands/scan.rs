//! `docfoo scan` — OCR one or more PDFs/images into resource cards.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use docfoo_ocr::{ImportOutput, Progress};
use serde_json::{json, Value};

use crate::cli::{Cli, ScanArgs};
use crate::config::ModelPreferences;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::scan::{self, ScanRequest};
use crate::sidecar::Sidecar;
use crate::workspace::Workspace;

pub fn run(cli: &Cli, format: OutputFormat, workspace: &Workspace, args: &ScanArgs) -> Result<()> {
    let files: Vec<PathBuf> = args
        .files
        .iter()
        .map(|file| crate::workspace::absolutize(file))
        .collect::<Result<Vec<_>>>()?;
    for file in &files {
        if !file.is_file() {
            return Err(CliError::NotFound(format!(
                "file not found: {}",
                file.display()
            )));
        }
    }

    let preferences = ModelPreferences::load(workspace);
    let text_model = match &args.text_model {
        Some(model) if !model.trim().is_empty() => model.trim().to_string(),
        _ => preferences
            .get("scan")?
            .map(str::to_string)
            .ok_or_else(|| {
                CliError::Message(
                    "no scan model selected — pass --text_model or run `docfoo model --set scan provider/model`"
                        .to_string(),
                )
            })?,
    };
    // The figure model defaults to the text model (per the CLI contract).
    let figure_model = if args.no_figures {
        None
    } else {
        Some(
            args.figure_model
                .clone()
                .filter(|model| !model.trim().is_empty())
                .unwrap_or_else(|| text_model.clone()),
        )
    };

    let status = crate::setup::check(workspace);
    if !status.ready() {
        return Err(CliError::Message(format!(
            "scan dependencies are missing ({}). Run `docfoo setup` first.",
            status.missing().join(", ")
        )));
    }

    let cancel = cancel_flag();
    let mut sidecar = Sidecar::locate(workspace)?;
    let client = sidecar.client()?;
    let destination = args
        .output
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let jobs = args.jobs.max(1);
    let multi = files.len() > 1;

    // Documents run `jobs` at a time; each file is an independent import, so
    // the per-document thread needs no shared state beyond the thread-safe
    // sidecar client and the shared cancel flag.
    let mut results: Vec<(PathBuf, Result<ImportOutput>)> = Vec::with_capacity(files.len());
    for chunk in files.chunks(jobs) {
        let chunk_results: Vec<(PathBuf, Result<ImportOutput>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|file| {
                    let client = Arc::clone(&client);
                    let cancel = Arc::clone(&cancel);
                    let text_model = text_model.as_str();
                    let figure_model = figure_model.as_deref();
                    let destination = destination.as_str();
                    let analysis_prompt = args.analysis_prompt.as_deref();
                    let prompt = args.prompt.as_deref();
                    let concurrency = args.parallel.unwrap_or(4).clamp(1, 40);
                    scope.spawn(move || {
                        let request = ScanRequest {
                            file,
                            text_model,
                            figure_model,
                            analysis_prompt,
                            prompt,
                            concurrency,
                            destination,
                            pages: args.pages.clone(),
                            cancel,
                        };
                        let mut progress =
                            |event: Progress| emit_progress(file, multi, cli.quiet, event);
                        (
                            file.clone(),
                            scan::run_scan(workspace, request, client, &mut progress),
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        (
                            PathBuf::new(),
                            Err(CliError::Message("scan worker panicked".to_string())),
                        )
                    })
                })
                .collect()
        });
        results.extend(chunk_results);
    }

    // A single document keeps the historical envelope exactly (errors still
    // propagate as command failures).
    if !multi {
        let (_, result) = results.pop().expect("one result per file");
        let output = result?;
        let data = scan_data(workspace, &output);
        return output::success(format, "scan", &workspace.root.display().to_string(), data);
    }

    let succeeded = results.iter().filter(|(_, result)| result.is_ok()).count();
    if succeeded == 0 {
        return Err(CliError::Message(format!(
            "all {} scans failed",
            results.len()
        )));
    }
    let entries: Vec<Value> = results
        .iter()
        .map(|(file, result)| match result {
            Ok(output) => {
                let mut data = scan_data(workspace, output);
                data["file"] = json!(file.display().to_string());
                data["ok"] = json!(true);
                data
            }
            Err(error) => json!({
                "file": file.display().to_string(),
                "ok": false,
                "message": error.to_string(),
            }),
        })
        .collect();
    let data = json!({
        "message": format!(
            "scanned {} file(s): {} succeeded, {} failed",
            results.len(),
            succeeded,
            results.len() - succeeded
        ),
        "results": entries,
    });
    output::success(format, "scan", &workspace.root.display().to_string(), data)
}

/// The `data` object for one finished scan (shared by the single- and
/// multi-document envelopes).
fn scan_data(workspace: &Workspace, output: &ImportOutput) -> Value {
    let rel = output
        .out_dir
        .strip_prefix(workspace.resources_dir())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    json!({
        "outDir": output.out_dir.display().to_string(),
        "rel": rel,
        "md": output.md_path.display().to_string(),
        "pages": output.pages,
        "figures": output.figures,
        "chars": output.chars,
        "partial": output.partial,
        "failedPage": output.failed_page,
        "cancelled": output.cancelled,
        "message": output.error,
    })
}

/// Print one progress event, prefixed with the file name when more than one
/// document shares the terminal.
fn emit_progress(file: &Path, multi: bool, quiet: bool, event: Progress) {
    if quiet {
        return;
    }
    let prefix = if multi {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        format!("[{name}] ")
    } else {
        String::new()
    };
    match event {
        Progress::Started { pages, cached } => {
            eprintln!("{prefix}scan: {pages} page(s), {cached} cached")
        }
        Progress::RenderedPage { page, total } => eprintln!("{prefix}  rendered {page}/{total}"),
        Progress::LayoutPage {
            page,
            regions,
            figures,
            cached,
        } => eprintln!(
            "{prefix}  layout p{page}: {regions} regions, {figures} figures{}",
            if cached { " (cached)" } else { "" }
        ),
        Progress::LayoutUnavailable { reason } => {
            eprintln!("{prefix}  layout unavailable: {reason}")
        }
        Progress::OcrBatch { start, end } => eprintln!("{prefix}  OCR batch pages {start}-{end}"),
        Progress::OcrRegionDone { .. } => {}
        Progress::OcrPageDone { page, chars, cached } => eprintln!(
            "{prefix}  p{page} OCR ok ({chars} chars){}",
            if cached { " (cached)" } else { "" }
        ),
        Progress::Analyzing { page, assets } => {
            eprintln!("{prefix}  analyzing p{page}: {assets} asset(s)")
        }
        Progress::AnalysisFailed {
            page,
            assets,
            message,
        } => eprintln!("{prefix}  analysis failed p{page} ({assets} assets): {message}"),
        Progress::Assembling => eprintln!("{prefix}  assembling…"),
        Progress::Done {
            out_dir,
            pages,
            figures,
            chars,
            partial,
            failed_page,
            cancelled,
        } => eprintln!(
            "{prefix}  done → {out_dir} ({pages} pages, {figures} figures, {chars} chars, partial={partial}, failed_page={failed_page:?}, cancelled={cancelled})"
        ),
    }
}

fn cancel_flag() -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    });
    cancel
}
