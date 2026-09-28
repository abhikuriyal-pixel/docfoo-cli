//! CLI driver for the OCR pipeline (dev/testing only):
//! `cargo run -p docfoo-ocr --example scan -- <input> [outdir]`
//! API key via DOCFOO_API_KEY or OPENCODE_API_KEY.

use std::path::PathBuf;
use std::sync::Arc;

use docfoo_ocr::{run_import, HttpCompletionClient, ImportOptions, Progress};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: scan <input> [outdir]");
        std::process::exit(2);
    }
    let input = PathBuf::from(&args[1]);
    let outdir = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("db/resources"));

    let api_key = std::env::var("DOCFOO_API_KEY")
        .or_else(|_| std::env::var("OPENCODE_API_KEY"))
        .expect("DOCFOO_API_KEY or OPENCODE_API_KEY");
    let endpoint = std::env::var("DOCFOO_OCR_ENDPOINT").unwrap_or_else(|_| docfoo_ocr::ocr::ENDPOINT.to_string());
    let mut opts = ImportOptions::new(Arc::new(HttpCompletionClient::new(endpoint, api_key)));
    opts.ocr_model = std::env::var("DOCFOO_OCR_MODEL").unwrap_or_else(|_| docfoo_ocr::ocr::MODEL.to_string());
    opts.models_dir = PathBuf::from("models");
    opts.output_dir = outdir.clone();
    // optional --pages 1 2 3
    if let Some(pos) = args.iter().position(|a| a == "--pages") {
        opts.pages = Some(
            args[pos + 1..]
                .iter()
                .take_while(|a| !a.starts_with('-'))
                .filter_map(|a| a.parse().ok())
                .collect(),
        );
    }

    let mut r = 0u32;
    let result = run_import(&input, &opts, &mut |p: Progress| {
        r += 1;
        if r % 2 == 0 {
            return; // only print every other event to keep output readable
        }
        match &p {
            Progress::Started { pages, cached } => println!("started: {pages} pages ({cached} cached)"),
            Progress::RenderedPage { page, total } => println!("  rendered {page}/{total}"),
            Progress::LayoutPage { page, regions, figures, cached } => {
                println!("  layout p{page}: {regions} regions, {figures} figures{}", if *cached { " (cached)" } else { "" })
            }
            Progress::LayoutUnavailable { reason } => println!("  NO LAYOUT: {reason}"),
            Progress::OcrBatch { start, end } => println!("  batch pages {start}-{end}"),
            Progress::OcrPageDone { page, chars, cached } => println!("  p{page} OCR ok ({chars} chars){}", if *cached { " (cached)" } else { "" }),
            Progress::Assembling => println!("assembling…"),
            Progress::Done { out_dir, pages, figures, chars, partial, failed_page, cancelled } => {
                println!("DONE -> {out_dir} ({pages} pages, {figures} figures, {chars} chars) partial={partial} failed_page={failed_page:?} cancelled={cancelled}")
            }
            _ => {}
        }
    });
    match result {
        Ok(out) => println!("md: {}", out.md_path.display()),
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
