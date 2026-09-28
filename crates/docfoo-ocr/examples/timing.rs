//! Timing probe: prints every progress event with elapsed ms so stage
//! gaps are visible. `cargo run -p docfoo-ocr --example timing -- <pdf> [pages...]`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use docfoo_ocr::{run_import, HttpCompletionClient, ImportOptions, Progress};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: timing <input> [page...]");
        std::process::exit(2);
    }
    let input = PathBuf::from(&args[1]);
    let client = HttpCompletionClient::new(
        docfoo_ocr::ocr::ENDPOINT,
        std::env::var("DOCFOO_API_KEY").expect("DOCFOO_API_KEY"),
    );
    let mut opts = ImportOptions::new(Arc::new(client));
    opts.models_dir = PathBuf::from("models");
    opts.output_dir = PathBuf::from("db/resources-timing");
    let pages: Vec<u32> = args[2..].iter().filter_map(|a| a.parse().ok()).collect();
    if !pages.is_empty() {
        opts.pages = Some(pages);
    }

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
            Progress::LayoutUnavailable { reason } => {
                println!("{ms:>7} ms  LayoutUnavailable {{ reason: {reason} }}")
            }
            Progress::OcrBatch { start, end } => println!("{ms:>7} ms  OcrBatch {{ {start}-{end} }}"),
            Progress::OcrRegionDone { page, done, total } => {
                println!("{ms:>7} ms  OcrRegionDone {{ page: {page}, {done}/{total} }}")
            }
            Progress::OcrPageDone { page, chars, .. } => {
                println!("{ms:>7} ms  OcrPageDone {{ page: {page}, chars: {chars} }}")
            }
            Progress::Analyzing { page, assets } => {
                println!("{ms:>7} ms  Analyzing {{ page: {page}, assets: {assets} }}")
            }
            Progress::AnalysisFailed { page, assets, message } => {
                println!("{ms:>7} ms  AnalysisFailed {{ page: {page}, assets: {assets}, message: {message} }}")
            }
            Progress::Assembling => println!("{ms:>7} ms  Assembling"),
            Progress::Done { out_dir, pages, figures, chars, partial, failed_page, .. } => {
                println!("{ms:>7} ms  Done {{ {out_dir} pages={pages} figs={figures} chars={chars} partial={partial} failed={failed_page:?} }}")
            }
        }
    });
    println!("result: {:?}", result.as_ref().map(|o| o.md_path.to_string_lossy().to_string()));
    if let Err(e) = result {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}
