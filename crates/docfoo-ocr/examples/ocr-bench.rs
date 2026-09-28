//! OCR bench: run the import pipeline against any OpenAI-compatible endpoint
//! (e.g. a local llama.cpp server) and print per-stage timings + region counts,
//! so the OCR-per-region cost vs a single whole-image request is visible.
//!
//! usage: cargo run -p docfoo-ocr --example ocr-bench -- <input> [concurrency]
//!   DOCFOO_OCR_ENDPOINT / DOCFOO_OCR_MODEL  override the OCR endpoint/model

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use docfoo_ocr::{run_import, HttpCompletionClient, ImportOptions, Progress};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: ocr-bench <input> [concurrency]");
        std::process::exit(2);
    }
    let input = PathBuf::from(&args[1]);
    let concurrency: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let endpoint = env::var("DOCFOO_OCR_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/v1/chat/completions".to_string());
    let model = env::var("DOCFOO_OCR_MODEL")
        .unwrap_or_else(|_| "nopesadly/GLM-OCR-Q4_K_M.gguf:Q4_K_M".to_string());
    let api_key = env::var("DOCFOO_API_KEY").unwrap_or_else(|_| "local".to_string());

    let mut opts = ImportOptions::new(Arc::new(HttpCompletionClient::new(endpoint.clone(), api_key)));
    opts.ocr_model = model.clone();
    opts.concurrency = concurrency;
    opts.models_dir = PathBuf::from("models");
    opts.output_dir = PathBuf::from("db/resources-bench");

    println!(
        "OCR bench: {input:?} | endpoint={endpoint} model={model} concurrency={concurrency}"
    );

    let t0 = Instant::now();
    let result = run_import(&input, &opts, &mut |p: Progress| {
        let ms = t0.elapsed().as_millis();
        match &p {
            Progress::Started { pages, .. } => println!("{ms:>7} ms  Started {{ pages: {pages} }}"),
            Progress::RenderedPage { page, total } => {
                println!("{ms:>7} ms  RenderedPage {{ page: {page}/{total} }}")
            }
            Progress::LayoutPage { page, regions, figures, .. } => {
                println!("{ms:>7} ms  LayoutPage {{ page: {page}, regions: {regions}, figures: {figures} }}")
            }
            Progress::OcrBatch { start, end } => {
                println!("{ms:>7} ms  OcrBatch {{ {start}-{end} }}")
            }
            Progress::OcrPageDone { page, chars, .. } => {
                println!("{ms:>7} ms  OcrPageDone {{ page: {page}, chars: {chars} }}")
            }
            Progress::Assembling => println!("{ms:>7} ms  Assembling"),
            Progress::Done { pages, figures, chars, .. } => {
                println!("{ms:>7} ms  Done {{ pages={pages} figs={figures} chars={chars} }}")
            }
            _ => {}
        }
    });
    if let Err(e) = result {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}
