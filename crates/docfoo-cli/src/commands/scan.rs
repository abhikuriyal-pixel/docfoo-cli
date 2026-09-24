//! `docfoo scan` — OCR a PDF or image into a resource card.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use docfoo_ocr::Progress;
use serde_json::json;

use crate::cli::{Cli, ScanArgs};
use crate::config::ModelPreferences;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::scan::{self, ScanRequest};
use crate::sidecar::Sidecar;
use crate::workspace::Workspace;

pub fn run(cli: &Cli, format: OutputFormat, workspace: &Workspace, args: &ScanArgs) -> Result<()> {
    let file = crate::workspace::absolutize(&args.file)?;
    if !file.is_file() {
        return Err(CliError::NotFound(format!(
            "file not found: {}",
            file.display()
        )));
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
            "scan dependencies are missing ({}). Run `docfoo setup` (Linux) or `docfoo setup --from <DocFoo/models>` first.",
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
    let request = ScanRequest {
        file: &file,
        text_model: &text_model,
        figure_model: figure_model.as_deref(),
        analysis_prompt: args.analysis_prompt.as_deref(),
        prompt: args.prompt.as_deref(),
        concurrency: args.parallel.unwrap_or(4).clamp(1, 40),
        destination: &destination,
        pages: args.pages.clone(),
        cancel,
    };

    let quiet = cli.quiet;
    let mut progress = |event: Progress| {
        if quiet {
            return;
        }
        match event {
            Progress::Started { pages, cached } => {
                eprintln!("scan: {pages} page(s), {cached} cached")
            }
            Progress::RenderedPage { page, total } => eprintln!("  rendered {page}/{total}"),
            Progress::LayoutPage {
                page,
                regions,
                figures,
                cached,
            } => eprintln!(
                "  layout p{page}: {regions} regions, {figures} figures{}",
                if cached { " (cached)" } else { "" }
            ),
            Progress::LayoutUnavailable { reason } => eprintln!("  layout unavailable: {reason}"),
            Progress::OcrBatch { start, end } => eprintln!("  OCR batch pages {start}-{end}"),
            Progress::OcrRegionDone { .. } => {}
            Progress::OcrPageDone { page, chars, cached } => eprintln!(
                "  p{page} OCR ok ({chars} chars){}",
                if cached { " (cached)" } else { "" }
            ),
            Progress::Analyzing { page, assets } => {
                eprintln!("  analyzing p{page}: {assets} asset(s)")
            }
            Progress::AnalysisFailed {
                page,
                assets,
                message,
            } => eprintln!("  analysis failed p{page} ({assets} assets): {message}"),
            Progress::Assembling => eprintln!("  assembling…"),
            Progress::Done {
                out_dir,
                pages,
                figures,
                chars,
                partial,
                failed_page,
                cancelled,
            } => eprintln!(
                "  done → {out_dir} ({pages} pages, {figures} figures, {chars} chars, partial={partial}, failed_page={failed_page:?}, cancelled={cancelled})"
            ),
        }
    };

    let output = scan::run_scan(workspace, request, client, &mut progress)?;
    let rel = output
        .out_dir
        .strip_prefix(&workspace.resources_dir())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let data = json!({
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
    });
    output::success(format, "scan", &workspace.root.display().to_string(), data)
}

fn cancel_flag() -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    });
    cancel
}
