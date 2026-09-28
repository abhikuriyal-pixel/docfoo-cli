//! Pipeline orchestrator — pdfium_probe architecture, streamed per page:
//! render a page, detect its layout, OCR every region individually
//! (batched in parallel), and assemble the markdown incrementally at each
//! region's position in the reading order. Only one page bitmap is alive
//! at any moment and each finished page is appended straight to content.md,
//! so memory stays flat for documents of any size.
//!
//! Fault tolerance: OCR results are cached in an append-only journal
//! (`ocr_cache.jsonl`), pages are assembled as soon as they complete, and
//! a failure mid-document writes the partial output — a re-run of the
//! same PDF resumes from the cache instead of restarting.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbaImage;

use crate::analyze::{omitted_regions, order_regions};
use crate::assemble::{
    clean_region_text, group_bullets, normalize_analysis_alt, png_bytes, prepare_asset_crops,
    region_image, render_page_fragments, AssetCrop, RegionJob, RenderCtx,
};
use crate::client::{image_mime, CompletionClient};
use crate::error::{OcrError, Result};
use crate::layout::{acquire_detector, ensure_onnx_runtime, Region};
use crate::raster::PdfiumRasterizer;

/// Process-wide budget of simultaneous OCR/analysis HTTP requests, shared by
/// every concurrent import so N workspaces × "Parallel" can never exceed the
/// provider budget.
#[derive(Debug)]
pub struct OcrPermitPool {
    max: std::sync::atomic::AtomicUsize,
    live: std::sync::Mutex<usize>,
    cv: std::sync::Condvar,
}

/// RAII permit: releasing on drop, one in-flight request each.
pub struct OcrPermit<'a> {
    pool: &'a OcrPermitPool,
}

impl Drop for OcrPermit<'_> {
    fn drop(&mut self) {
        let mut live = self.pool.live.lock().unwrap();
        *live -= 1;
        self.pool.cv.notify_one();
    }
}

impl OcrPermitPool {
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            max: std::sync::atomic::AtomicUsize::new(max),
            live: std::sync::Mutex::new(0),
            cv: std::sync::Condvar::new(),
        }
    }

    /// Adjust the budget at runtime (e.g. after the user changes the
    /// "Request limit" setting); wakes waiters when it grows.
    pub fn set_max(&self, max: usize) {
        self.max.store(max.max(1), std::sync::atomic::Ordering::Relaxed);
        self.cv.notify_all();
    }

    /// Blocks until a permit is free.
    pub fn acquire(&self) -> OcrPermit<'_> {
        let mut live = self.live.lock().unwrap();
        while *live >= self.max.load(std::sync::atomic::Ordering::Relaxed) {
            live = self.cv.wait(live).unwrap();
        }
        *live += 1;
        OcrPermit { pool: self }
    }
}

struct ScanDirLock(PathBuf);

impl Drop for ScanDirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Model file name inside the models dir.
///
/// On Android the model ships as a native-library-shaped file in the app's
/// `jniLibs` (extracted to the native library dir at install, the only
/// location where `std::fs` can read a real file without asset-manager JNI),
/// so it carries the `lib*.so` name there.
#[cfg(target_os = "android")]
const MODEL_FILE: &str = "libppdoclayoutv3.so";
#[cfg(not(target_os = "android"))]
const MODEL_FILE: &str = "PP-DocLayoutV3.onnx";

/// ONNX Runtime library file name inside the models dir.
#[cfg(target_os = "windows")]
const ORT_DLL_FILE: &str = "onnxruntime.dll";
#[cfg(not(target_os = "windows"))]
const ORT_DLL_FILE: &str = "libonnxruntime.so";

/// PDFium library file name inside the models dir.
#[cfg(target_os = "windows")]
const PDFIUM_DLL_FILE: &str = "pdfium.dll";
#[cfg(not(target_os = "windows"))]
const PDFIUM_DLL_FILE: &str = "libpdfium.so";

/// Platform file name of the layout model (see [`MODEL_FILE`]).
pub fn layout_model_file_name() -> &'static str {
    MODEL_FILE
}

/// Platform file name of the ONNX Runtime library (see [`ORT_DLL_FILE`]).
pub fn ort_library_file_name() -> &'static str {
    ORT_DLL_FILE
}

/// Platform file name of the PDFium library (see [`PDFIUM_DLL_FILE`]).
pub fn pdfium_library_file_name() -> &'static str {
    PDFIUM_DLL_FILE
}

/// Independent configuration for optional asset image analysis.
#[derive(Debug, Clone)]
pub struct ImageAnalysisOptions {
    /// Model key passed to the completion client (`provider/modelId`).
    pub model: String,
    /// Prompt sent for each detected figure, chart, image, or table.
    pub prompt: String,
}

/// Options for one import job.
#[derive(Clone)]
pub struct ImportOptions {
    /// One-image completion client. The app supplies the pi agent bridge;
    /// examples and benches supply [`crate::HttpCompletionClient`].
    pub client: Arc<dyn CompletionClient>,
    /// OCR model key passed to the client (`provider/modelId`, or a plain
    /// model id for direct HTTP endpoints).
    pub ocr_model: String,
    /// Prompt sent with each uncached region OCR request.
    pub ocr_prompt: String,
    /// Optional Figures & Tables analysis configuration. `None` disables all
    /// asset analysis requests.
    pub analysis: Option<ImageAnalysisOptions>,
    /// Render DPI for PDFs (300 like the reference).
    pub dpi: u32,
    /// Downscale pages to this max side in pixels (0 = keep full size).
    pub max_side: u32,
    /// Parallel region OCR requests. Cost is usage-based (subscription),
    /// not per request, so higher concurrency only trades rate-limit
    /// headroom for speed.
    pub concurrency: usize,
    /// Directory holding the runtime dependencies (model + dlls).
    pub models_dir: PathBuf,
    /// Directory where results are written (`db/resources`).
    pub output_dir: PathBuf,
    /// 1-based page numbers to process (None = all pages).
    pub pages: Option<Vec<u32>>,
    /// Optional cancellation flag: when set, the pipeline stops between
    /// pages (and between OCR batches) and writes the partial output.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Optional process-wide budget for OCR and analysis HTTP requests.
    pub permits: Option<std::sync::Arc<OcrPermitPool>>,
}

impl ImportOptions {
    /// Options for `client` with the built-in defaults.
    #[must_use]
    pub fn new(client: Arc<dyn CompletionClient>) -> Self {
        Self {
            client,
            ocr_model: crate::ocr::MODEL.to_string(),
            ocr_prompt: crate::ocr::REGION_PROMPT.to_string(),
            analysis: None,
            dpi: 300,
            max_side: 2048,
            concurrency: 4,
            models_dir: PathBuf::new(),
            output_dir: PathBuf::new(),
            pages: None,
            cancel: None,
            permits: None,
        }
    }
}

/// The result of a completed import.
#[derive(Debug, Clone)]
pub struct ImportOutput {
    /// The result directory (contains `content.md`, `assets/` and the cache).
    pub out_dir: PathBuf,
    /// The markdown file (partial when `partial` is set).
    pub md_path: PathBuf,
    /// Number of pages processed.
    pub pages: usize,
    /// Number of figure and table asset crops embedded (kept under the
    /// historical `figures` field name for event compatibility).
    pub figures: usize,
    /// Total characters OCR'd (cached + fresh).
    pub chars: usize,
    /// True when a page failed and the output is incomplete; the finished
    /// pages are still written, and re-running resumes from the cache.
    pub partial: bool,
    /// The 1-based page that failed, if any.
    pub failed_page: Option<u32>,
    /// True when the run was stopped by the user (partial output written).
    pub cancelled: bool,
    /// The failure message, if any.
    pub error: Option<String>,
}

/// Progress events emitted during an import.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Progress {
    /// The job started with `pages` pages to process, `cached` of which
    /// are already fully read (replayed from the cache, no rework).
    Started { pages: usize, cached: usize },
    /// A page was rendered (PDFs only).
    RenderedPage { page: usize, total: usize },
    /// Layout detection finished for a page. `figures` counts all emitted
    /// figure/table assets; `cached` is true when the page was replayed from
    /// the cache (no inference ran).
    LayoutPage {
        page: usize,
        regions: usize,
        figures: usize,
        cached: bool,
    },
    /// Layout detection is unavailable (missing model/runtime); the job
    /// continues without figures/tables.
    LayoutUnavailable { reason: String },
    /// An OCR batch started (pages are 1-based).
    OcrBatch { start: usize, end: usize },
    /// One region of a page finished OCR (or was served from the cache).
    /// `done`/`total` cover the page's OCR'd regions (cache hits
    /// included), giving the UI a live within-page progress readout.
    OcrRegionDone { page: usize, done: usize, total: usize },
    /// A page finished OCR. `cached` is true when the page was replayed
    /// from the cache (no API calls).
    OcrPageDone { page: usize, chars: usize, cached: bool },
    /// Figures & Tables analysis started for a page with `assets` crops.
    Analyzing { page: usize, assets: usize },
    /// Figures & Tables analysis finished with failures. The page still
    /// completes with generic fallback alt text; `message` is the provider
    /// error so the UI can show why (and which model to switch away from).
    AnalysisFailed {
        page: usize,
        assets: usize,
        message: String,
    },
    /// Assembling the final markdown.
    Assembling,
    /// Everything finished (or the partial result was written).
    Done {
        out_dir: String,
        pages: usize,
        figures: usize,
        chars: usize,
        partial: bool,
        failed_page: Option<u32>,
        cancelled: bool,
    },
}

/// True when the user requested a stop.
fn is_cancelled(opts: &ImportOptions) -> bool {
    opts.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
}

/// Everything needed to process one page, prepared one page ahead so
/// render + layout inference overlap the current page's OCR network
/// waits. Only two page bitmaps are ever alive (current + prefetch).
struct PageData {
    img: RgbaImage,
    ordered: Vec<Region>,
    omitted: std::collections::HashSet<usize>,
}

/// A page prepared ahead of time: its 1-based ordinal plus its data (or
/// the error that processing it would have raised — surfaced at the same
/// point in the sequence as inline preparation would have).
type Prefetched = (usize, Result<(PageData, Option<String>)>);

/// The layout detector's lazy warm-up state. The process-wide detector is
/// acquired per inference at the detection site — it is never parked here,
/// so concurrent imports interleave layout passes instead of one import
/// holding the global detector mutex for its whole run.
struct DetectorState {
    /// True once the warm-up thread was joined / acquisition attempted.
    tried: bool,
    /// Why layout is unavailable (missing model/runtime), if so.
    failure: Option<String>,
    model_thread: Option<std::thread::JoinHandle<std::result::Result<(), String>>>,
    resolved_model: Option<PathBuf>,
}

/// Run the full import pipeline on `input` (a PDF or an image file).
/// `emit` receives progress events on the calling thread.
///
/// Pages are OCR'd and assembled incrementally: a failure on one page
/// stops the loop, writes the partial markdown and cache, and returns
/// `Ok` with `partial` set — re-running the same input resumes from the
/// per-region cache.
///
/// # Errors
///
/// Returns an [`OcrError`] when the input cannot be read, the layout
/// stage hard-fails, or the output cannot be written.
pub fn run_import(
    input: &Path,
    opts: &ImportOptions,
    emit: &mut dyn FnMut(Progress),
) -> Result<ImportOutput> {
    // ---- stage 0: out dir + cache — enables page-level resume ----
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_pdf = ext == "pdf";
    let base = sanitize_stem(input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let mut stem = base.clone();
    let mut n = 2;
    // A crashed run may leave the lock, so a later scan intentionally uses a suffixed directory.
    let out_dir = loop {
        let dir = opts.output_dir.join(&stem);
        std::fs::create_dir_all(&dir)
            .map_err(|e| OcrError::Output(format!("could not create {dir:?}: {e}")))?;
        let lock_path = dir.join(".scan_lock");
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
            Ok(_) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                stem = format!("{base}-{n}");
                n += 1;
            }
            Err(e) => {
                return Err(OcrError::Output(format!("could not reserve {dir:?}: {e}")));
            }
        }
    };
    let _dir_lock = ScanDirLock(out_dir.join(".scan_lock"));
    let assets_dir = out_dir.join("assets");
    std::fs::create_dir_all(&assets_dir)
        .map_err(|e| OcrError::Output(format!("could not create {out_dir:?}: {e}")))?;

    // per-region OCR cache: key = stable (page, label, box) signature;
    // cached regions are skipped on resume. Pages that completed in a
    // previous run also carry page-level keys. The page key is namespaced by
    // the optional analysis model/prompt so a changed analysis configuration
    // never replays stale alt text.
    //
    // Storage is an append-only journal (`ocr_cache.jsonl`, one
    // `["key","value"]` JSON record per line): appending per page is O(1)
    // instead of re-serializing the whole ever-growing map on every page
    // (O(pages²) cumulative work plus a large transient string each time).
    // The pre-journal single-JSON file is still loaded as the base layer.
    let analysis_namespace = opts
        .analysis
        .as_ref()
        .map(|analysis| analysis_cache_namespace(&analysis.model, effective_analysis_prompt(&analysis.prompt)));
    let cache_path = out_dir.join("ocr_cache.jsonl");
    let legacy_cache_path = out_dir.join("ocr_cache.json");
    let mut cache: HashMap<String, String> = load_cache(&cache_path, &legacy_cache_path);
    // records detected/OCR'd since the last journal flush (appended at
    // page boundaries and before every exit)
    let mut pending_cache: Vec<(String, String)> = Vec::new();

    let rasterizer: Option<PdfiumRasterizer> = if is_pdf {
        let bytes = std::fs::read(input)
            .map_err(|e| OcrError::Input(format!("Could not read the file: {e}")))?;
        let pdfium_dll = resolve_in_models(&opts.models_dir, PDFIUM_DLL_FILE, "DOCFOO_PDFIUM_DLL")?;
        Some(PdfiumRasterizer::new(bytes, opts.dpi, &pdfium_dll)?)
    } else {
        None
    };
    let total: usize = match &rasterizer {
        Some(r) => r.page_count() as usize,
        None => 1,
    };
    // wanted pages (1-based), resolved once — used before rendering so a
    // fully-cached run skips the heavy stages entirely
    let wanted: Vec<usize> = match &opts.pages {
        Some(list) => list
            .iter()
            .copied()
            .filter(|p| (1..=total as u32).contains(p))
            .map(|p| p as usize)
            .collect(),
        None => (1..=total).collect(),
    };
    let need_work = wanted
        .iter()
        .any(|p| !page_is_cached(&cache, *p as u32, analysis_namespace.as_deref()));
    emit(Progress::Started {
        pages: total,
        cached: wanted
            .iter()
            .filter(|p| page_is_cached(&cache, **p as u32, analysis_namespace.as_deref()))
            .count(),
    });

    // ---- streaming main loop setup ----
    // The layout model is loaded on a background thread NOW so the 130 MB
    // session creation overlaps the first page's render/OCR; the thread is
    // joined lazily right before the first page that needs detection (and
    // reaped after the loop when none does). On later imports the
    // process-wide detector cache makes this instant.
    let mut cancelled = false;
    let mut partial: Option<(u32, String)> = None;

    let model_res: std::result::Result<PathBuf, String> =
        resolve_in_models(&opts.models_dir, MODEL_FILE, "DOCFOO_LAYOUT_MODEL").map_err(|e| e.to_string());
    let resolved_model = model_res.clone().ok();
    let mut layout_failure: Option<String> = None;
    // skip loading the 130 MB model entirely when every wanted page is
    // already cached
    let model_thread = match (need_work, model_res) {
        (false, _) => None,
        (true, Ok(model_path)) => {
            let ort_dll = resolve_in_models(&opts.models_dir, ORT_DLL_FILE, "DOCFOO_ORT_DLL");
            let model_path_clone = model_path.clone();
            Some(std::thread::spawn(move || {
                // load + populate the process-wide cache, then drop the
                // guard (the mutex guard is !Send, so it never leaves
                // this thread)
                ensure_onnx_runtime(&ort_dll.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                acquire_detector(&model_path_clone).map(|_| ()).map_err(|e| e.to_string())
            }))
        }
        (true, Err(e)) => {
            layout_failure = Some(e);
            None
        }
    };
    let mut det = DetectorState {
        tried: false,
        failure: layout_failure,
        model_thread,
        resolved_model,
    };

    // ---- streaming main loop: one page in memory at a time ----
    //
    // Every page runs render -> layout -> region crops -> OCR -> assembly
    // on its own and the page bitmap is dropped before the next iteration,
    // so RAM stays flat no matter how many pages the document has (the
    // previous implementation buffered every rendered page until the end —
    // gigabytes for 500+ page PDFs). Markdown is appended to content.md as
    // pages finish and the cache journal is flushed per page, so even a
    // hard kill keeps completed pages resumable.
    let md_path = out_dir.join("content.md");
    let mut md_file = std::io::BufWriter::new(
        std::fs::File::create(&md_path)
            .map_err(|e| OcrError::Output(format!("could not create content.md: {e}")))?,
    );
    let mut md_pages_written = 0usize;

    let concurrency = opts.concurrency.max(1);
    let model = opts.ocr_model.clone();
    let prompt = opts.ocr_prompt.clone();
    let client = Arc::clone(&opts.client);

    let mut ctx = RenderCtx::default();
    let mut total_chars = 0usize;

    // The next uncached page is prepared (rendered + layout-detected)
    // while the current page's OCR requests are in flight; None means
    // "prepare on arrival" (first page / after cached stretches).
    let mut prefetched: Option<Prefetched> = None;

    'pages: for &p in &wanted {
        if is_cancelled(opts) {
            cancelled = true;
            partial = Some((p as u32, "Stopped by user".to_string()));
            break 'pages;
        }
        // fully cached page: replay from the cache — no render, no layout,
        // no region crops, no OCR, no assembly
        if page_is_cached(&cache, p as u32, analysis_namespace.as_deref()) {
            let md = cache
                .get(&page_md_key(p as u32, analysis_namespace.as_deref()))
                .cloned()
                .unwrap_or_default();
            if !md.is_empty() {
                append_page_md(&mut md_file, &mut md_pages_written, &md)
                    .map_err(|e| OcrError::Output(format!("could not write content.md: {e}")))?;
            }
            let chars: usize = cache
                .get(&page_chars_key(p as u32))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let regions: usize = cache
                .get(&page_regions_key(p as u32))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let figures: usize = cache
                .get(&page_layout_key(p as u32))
                .and_then(|serialized| serde_json::from_str::<Vec<Region>>(serialized).ok())
                .map(|found| {
                    let omitted = omitted_regions(&found);
                    count_figures(&found, &omitted)
                })
                .or_else(|| {
                    cache
                        .get(&page_figures_key(p as u32))
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(0);
            // keep the global figure/table counters aligned so fresh pages
            // keep the same asset numbering as the original run
            ctx.figure_n = cache
                .get(&page_figure_n_key(p as u32))
                .and_then(|s| s.parse().ok())
                .unwrap_or(ctx.figure_n);
            ctx.table_n = cache
                .get(&page_table_n_key(p as u32))
                .and_then(|s| s.parse().ok())
                .unwrap_or(ctx.table_n);
            total_chars += chars;
            emit(Progress::LayoutPage { page: p, regions, figures, cached: true });
            emit(Progress::OcrPageDone { page: p, chars, cached: true });
            continue;
        }
        // ---- this page's data: either prefetched while the previous
        // page's OCR was in flight, or prepared now (first / only page) --
        let (page_data, layout_entry) = match prefetched.take() {
            Some((q, ready)) if q == p => ready?,
            _ => prepare_page(p, total, &mut det, rasterizer.as_ref(), input, opts, &cache, emit)?,
        };
        if let Some(serialized) = layout_entry {
            record(&mut cache, &mut pending_cache, page_layout_key(p as u32), serialized);
        }
        let PageData { img: page_img, ordered, omitted } = page_data;

        // region index -> transcription for THIS page (dropped with it)
        let mut region_texts: HashMap<usize, String> = HashMap::new();
        let jobs: Vec<(usize, String, Arc<Vec<u8>>)> = ordered
            .iter()
            .enumerate()
            .filter(|(idx, r)| !omitted.contains(idx) && r.label != "reference" && r.label != "inline_formula")
            .map(|(idx, r)| {
                let key = region_cache_key(p as u32, r);
                let img = region_image(&RegionJob { region: r, page: &page_img });
                (idx, key, Arc::new(png_bytes(&img)))
            })
            .collect();
        emit(Progress::OcrBatch { start: p, end: p });
        let mut chars = 0usize;
        // live within-page progress: cache hits count immediately, each
        // finished request increments the tally as its result arrives
        let page_parts = jobs.len();
        let mut parts_done = 0usize;
        // serve cached regions without any API call
        let mut to_fetch: Vec<(usize, String, Arc<Vec<u8>>)> = Vec::new();
        for (idx, key, png) in jobs {
            if let Some(text) = cache.get(&key) {
                chars += text.len();
                parts_done += 1;
                region_texts.insert(idx, text.clone());
            } else {
                to_fetch.push((idx, key, png));
            }
        }
        if page_parts > 0 {
            emit(Progress::OcrRegionDone { page: p, done: parts_done, total: page_parts });
        }
        // ---- OCR all uncached regions through a bounded worker pool ----
        //
        // Workers pull from a shared queue, so all `concurrency` slots
        // stay busy until the page's work is drained and one slow region
        // only blocks its own slot (the previous fixed-size batches waited
        // for their slowest member before starting any more requests).
        let expected = to_fetch.len();
        let (result_tx, result_rx) = mpsc::channel();
        let mut handles = Vec::new();
        if expected > 0 {
            let queue: Arc<Mutex<VecDeque<_>>> = Arc::new(Mutex::new(to_fetch.into_iter().collect()));
            for _ in 0..concurrency.min(expected) {
                let queue = Arc::clone(&queue);
                let client = Arc::clone(&client);
                let model = model.clone();
                let prompt = prompt.clone();
                let cancel = opts.cancel.clone();
                let permits = opts.permits.clone();
                let sender = result_tx.clone();
                handles.push(std::thread::spawn(move || {
                    while let Some((idx, key, png)) =
                        queue.lock().ok().and_then(|mut q| q.pop_front())
                    {
                        // stop pulling new work once the user is stopping;
                        // in-flight requests still deliver their result
                        if cancel.as_deref().is_some_and(|f| f.load(Ordering::Relaxed)) {
                            break;
                        }
                        let result = {
                            let _permit = permits.as_ref().map(|p| p.acquire());
                            complete_with_retry(
                                client.as_ref(),
                                &model,
                                &prompt,
                                png.as_slice(),
                                "image/png",
                                cancel.as_deref(),
                            )
                        };
                        if sender.send((idx, key, result)).is_err() {
                            break; // collector gone (cancel/teardown)
                        }
                    }
                }));
            }

            // overlap: with this page's requests in flight, prepare the
            // NEXT uncached page (render + layout inference) on this thread
            let after = wanted
                .iter()
                .position(|&x| x == p)
                .map_or(wanted.len(), |i| i + 1);
            prefetched = wanted[after..]
                .iter()
                .copied()
                .find(|&q| !page_is_cached(&cache, q as u32, analysis_namespace.as_deref()))
                .filter(|_| !is_cancelled(opts))
                .map(|q| {
                    (
                        q,
                        prepare_page(q, total, &mut det, rasterizer.as_ref(), input, opts, &cache, emit),
                    )
                });
        }
        drop(result_tx);

        let mut outputs = Vec::with_capacity(expected);
        let mut worker_lost = false;
        while outputs.len() < expected {
            if is_cancelled(opts) {
                cancelled = true;
                partial = Some((p as u32, "Stopped by user".to_string()));
                drop(handles);
                break 'pages;
            }
            match result_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(output) => {
                    outputs.push(output);
                    // report immediately, before the batch is processed,
                    // so the UI shows movement even while later requests
                    // are still in flight
                    parts_done += 1;
                    emit(Progress::OcrRegionDone { page: p, done: parts_done, total: page_parts });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    worker_lost = true;
                    break;
                }
            }
        }
        if worker_lost {
            partial = Some((p as u32, "OCR worker stopped unexpectedly".to_string()));
            drop(handles);
            break 'pages;
        }
        for handle in handles {
            let _ = handle.join();
        }
        for (idx, key, res) in outputs {
            match res {
                Ok(text) => {
                    let text = clean_region_text(&text);
                    chars += text.len();
                    record(&mut cache, &mut pending_cache, key, text.clone());
                    region_texts.insert(idx, text);
                }
                Err(OcrError::Cancelled) => {
                    cancelled = true;
                    partial = Some((p as u32, "Stopped by user".to_string()));
                    break 'pages;
                }
                Err(e) => {
                    // stop here: write the partial output + cache.
                    // the error already names the region's page, so
                    // keep just the reason for the message
                    let msg = match &e {
                        OcrError::OcrPage { message, .. } => message.clone(),
                        other => other.to_string(),
                    };
                    partial = Some((p as u32, msg));
                    break 'pages;
                }
            }
        }
        // Prepare and save the exact asset crops that will be embedded. The
        // optional analysis stage consumes these same PNG bytes.
        let asset_crops = prepare_asset_crops(
            p as u32,
            &ordered,
            &omitted,
            &page_img,
            &mut ctx,
            &assets_dir,
        )?;
        let mut asset_descriptions: HashMap<usize, String> = HashMap::new();
        let mut analysis_failed = false;
        if let Some(analysis) = opts.analysis.as_ref() {
            let asset_total = asset_crops.len();
            if asset_total > 0 {
                emit(Progress::Analyzing { page: p, assets: asset_total });
            }
            let analysis_result = analyze_assets(
                &asset_crops,
                analysis,
                &cache,
                &client,
                concurrency,
                opts.cancel.clone(),
                opts.permits.clone(),
            );
            for (key, value) in &analysis_result.records {
                record(&mut cache, &mut pending_cache, key.clone(), value.clone());
            }
            asset_descriptions = analysis_result.descriptions;
            analysis_failed = analysis_result.failed;
            if analysis_result.cancelled {
                cancelled = true;
                partial = Some((p as u32, "Stopped by user".to_string()));
                break 'pages;
            }
            // failed assets keep their generic fallback alt text, but the
            // provider error must reach the UI instead of being swallowed
            if let Some(message) = analysis_result.error {
                emit(Progress::AnalysisFailed { page: p, assets: asset_total, message });
            }
        }

        // assemble this page immediately and append it to content.md
        let ocr = |idx: usize, _r: &Region| -> Result<String> {
            region_texts.get(&idx).cloned().ok_or_else(|| {
                OcrError::Other(format!("page {p}: region {idx} missing OCR result"))
            })
        };
        let frags = render_page_fragments(
            &ordered,
            &omitted,
            &ocr,
            &asset_crops,
            &asset_descriptions,
            "assets",
        )?;
        let frags = group_bullets(frags);
        if !frags.is_empty() {
            append_page_md(&mut md_file, &mut md_pages_written, &frags.join("\n\n"))
                .map_err(|e| OcrError::Output(format!("could not write content.md: {e}")))?;
        }
        // page-level cache keys: once this page is stored, a re-run
        // replays it without ANY rework (no render/layout/OCR/assembly)
        let page_figures = count_figures(&ordered, &omitted);
        // A generic fallback is intentionally not a completed analysis
        // cache entry. Leave the page uncached so a later resume can retry
        // failed assets, while the current scan still completes normally.
        if !analysis_failed {
            record(
                &mut cache,
                &mut pending_cache,
                page_md_key(p as u32, analysis_namespace.as_deref()),
                frags.join("\n\n"),
            );
        }
        record(&mut cache, &mut pending_cache, page_chars_key(p as u32), chars.to_string());
        record(&mut cache, &mut pending_cache, page_regions_key(p as u32), ordered.len().to_string());
        record(&mut cache, &mut pending_cache, page_figures_key(p as u32), page_figures.to_string());
        record(&mut cache, &mut pending_cache, page_figure_n_key(p as u32), ctx.figure_n.to_string());
        record(&mut cache, &mut pending_cache, page_table_n_key(p as u32), ctx.table_n.to_string());
        total_chars += chars;
        // persist the journal after every page so even a hard kill keeps
        // the completed pages resumable
        flush_cache(&cache_path, &mut pending_cache);
        emit(Progress::OcrPageDone { page: p, chars, cached: false });
    }

    // reap the warm-up thread when no page ended up needing detection
    if !det.tried {
        if let Some(handle) = det.model_thread.take() {
            let _ = handle.join();
        }
    }
    // persist anything not yet journaled (cancel/failure exits land here)
    flush_cache(&cache_path, &mut pending_cache);

    // synthetic final render event so the UI's render stage always
    // completes (cached pages emit no per-page render events)
    if !cancelled && rasterizer.is_some() {
        emit(Progress::RenderedPage { page: total, total });
    }

    // ---- finish content.md (pages were already streamed into it) ----
    emit(Progress::Assembling);
    if let Some((failed_page, message)) = &partial {
        let comment = if cancelled {
            format!(
                "\n\n<!-- SCAN STOPPED: page {failed_page} ({message}). \
                 Run the scan again to continue from where it stopped. -->\n"
            )
        } else {
            format!(
                "\n\n<!-- SCAN INCOMPLETE: page {failed_page} failed ({message}). \
                 Run the scan again to continue from where it stopped. -->\n"
            )
        };
        md_file
            .write_all(comment.as_bytes())
            .map_err(|e| OcrError::Output(format!("could not write content.md: {e}")))?;
    }
    md_file
        .flush()
        .map_err(|e| OcrError::Output(format!("could not write content.md: {e}")))?;

    let out = ImportOutput {
        out_dir: out_dir.clone(),
        md_path,
        pages: wanted.len(),
        figures: (ctx.figure_n + ctx.table_n) as usize,
        chars: total_chars,
        partial: partial.is_some(),
        failed_page: partial.as_ref().map(|(p, _)| *p),
        cancelled,
        error: partial.map(|(_, m)| m),
    };
    emit(Progress::Done {
        out_dir: out_dir.to_string_lossy().to_string(),
        pages: out.pages,
        figures: out.figures,
        chars: out.chars,
        partial: out.partial,
        failed_page: out.failed_page,
        cancelled,
    });
    Ok(out)
}

/// Attempts per completion (one retry). Only [`OcrError::Transport`]
/// failures are retried — model/provider errors fail immediately.
const COMPLETION_ATTEMPTS: usize = 2;
/// Pause between completion attempts (cancel-aware).
const COMPLETION_RETRY_DELAY: Duration = Duration::from_millis(500);

struct AnalysisBatch {
    descriptions: HashMap<usize, String>,
    records: Vec<(String, String)>,
    failed: bool,
    cancelled: bool,
    /// First provider error, when `failed` is set.
    error: Option<String>,
}

/// Analyze final saved asset crops through the same bounded worker shape as
/// Text OCR. Failed descriptions are deliberately omitted from `records`, so
/// a generic fallback can never make a future resume think analysis finished.
fn analyze_assets(
    assets: &[AssetCrop],
    config: &ImageAnalysisOptions,
    cache: &HashMap<String, String>,
    client: &Arc<dyn CompletionClient>,
    concurrency: usize,
    cancel: Option<Arc<AtomicBool>>,
    permits: Option<Arc<OcrPermitPool>>,
) -> AnalysisBatch {
    let mut descriptions = HashMap::new();
    let mut jobs = Vec::new();
    let prompt = effective_analysis_prompt(&config.prompt);
    for asset in assets {
        let key = analysis_cache_key(asset, &config.model, prompt);
        if let Some(description) = cache
            .get(&key)
            .filter(|description| !description.trim().is_empty())
        {
            descriptions.insert(asset.region_index, description.clone());
        } else {
            jobs.push((asset.region_index, key, Arc::clone(&asset.bytes)));
        }
    }
    if jobs.is_empty() {
        return AnalysisBatch {
            descriptions,
            records: Vec::new(),
            failed: false,
            cancelled: false,
            error: None,
        };
    }

    let expected = jobs.len();
    let queue: Arc<Mutex<VecDeque<(usize, String, Arc<Vec<u8>>)>>>
        = Arc::new(Mutex::new(jobs.into_iter().collect()));
    let (result_tx, result_rx) = mpsc::channel();
    let mut handles = Vec::new();
    for _ in 0..concurrency.min(expected).max(1) {
        let queue = Arc::clone(&queue);
        let client = Arc::clone(client);
        let model = config.model.clone();
        let prompt = prompt.to_string();
        let cancel_for_worker = cancel.clone();
        let permits = permits.clone();
        let sender = result_tx.clone();
        handles.push(std::thread::spawn(move || {
            while let Some((idx, key, png)) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
                if cancel_for_worker.as_deref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    break;
                }
                let result = {
                    let _permit = permits.as_ref().map(|p| p.acquire());
                    complete_with_retry(
                        client.as_ref(),
                        &model,
                        &prompt,
                        png.as_slice(),
                        image_mime(&png),
                        cancel_for_worker.as_deref(),
                    )
                    .map(|raw| normalize_analysis_alt(&raw))
                };
                if sender.send((idx, key, result)).is_err() { break; }
            }
        }));
    }
    drop(result_tx);

    let mut outputs = Vec::with_capacity(expected);
    let mut cancelled = false;
    let mut worker_lost = false;
    while outputs.len() < expected {
        if cancel.as_deref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            cancelled = true;
            break;
        }
        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(output) => outputs.push(output),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => { worker_lost = true; break; }
        }
    }
    if cancelled {
        // Keep results that completed before cancellation visible to the
        // caller, then let in-flight workers stop without delaying cancel.
        while let Ok(output) = result_rx.try_recv() { outputs.push(output); }
        drop(handles);
    } else {
        for handle in handles { let _ = handle.join(); }
        if cancel.as_deref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            cancelled = true;
        }
    }

    let received = outputs.len();
    let mut records = Vec::new();
    let mut failed = worker_lost || received < expected;
    let mut error: Option<String> = None;
    if worker_lost {
        error = Some("the analysis worker stopped unexpectedly".to_string());
    } else if !cancelled && received < expected {
        error = Some(format!("{} of {expected} analysis requests did not finish", expected - received));
    }
    for (idx, key, result) in outputs {
        match result {
            Ok(description) => {
                descriptions.insert(idx, description.clone());
                records.push((key, description));
            }
            Err(OcrError::Cancelled) => cancelled = true,
            Err(e) => {
                failed = true;
                if error.is_none() {
                    error = Some(e.to_string());
                }
            }
        }
    }
    AnalysisBatch { descriptions, records, failed, cancelled, error }
}

/// Complete one image request with at most one retry, and only for
/// [`OcrError::Transport`] failures. Model/provider errors and empty
/// completions fail immediately so the UI never waits on a retry storm.
fn complete_with_retry(
    client: &dyn CompletionClient,
    model: &str,
    prompt: &str,
    image: &[u8],
    mime: &str,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    for attempt in 0..COMPLETION_ATTEMPTS {
        if flag_set(cancel) {
            return Err(OcrError::Cancelled);
        }
        match client.complete_image(model, prompt, image, mime, cancel) {
            Ok(text) if !text.trim().is_empty() => return Ok(text),
            Ok(_) => {
                return Err(OcrError::Other("the model returned an empty response".to_string()));
            }
            Err(OcrError::Cancelled) => return Err(OcrError::Cancelled),
            Err(error) => {
                let retryable = matches!(error, OcrError::Transport(_));
                if !retryable || attempt + 1 >= COMPLETION_ATTEMPTS {
                    return Err(error);
                }
            }
        }
        if sleep_cancelable(COMPLETION_RETRY_DELAY, cancel) {
            return Err(OcrError::Cancelled);
        }
    }
    Err(OcrError::Other("the completion failed".to_string()))
}

fn flag_set(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// Sleep `delay`, waking early when the user stops the job.
/// Returns true when cancellation was observed.
fn sleep_cancelable(delay: Duration, cancel: Option<&AtomicBool>) -> bool {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if flag_set(cancel) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    flag_set(cancel)
}

fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn effective_analysis_prompt(prompt: &str) -> &str {
    let trimmed = prompt.trim();
    if trimmed.is_empty() { crate::ocr::IMAGE_ANALYSIS_PROMPT } else { trimmed }
}

fn analysis_cache_namespace(model: &str, prompt: &str) -> String {
    format!("{:016x}", fnv1a(&[model.as_bytes(), prompt.as_bytes()]))
}

fn analysis_cache_key(asset: &AssetCrop, model: &str, prompt: &str) -> String {
    let kind = match asset.kind {
        crate::assemble::AssetKind::Figure => b"figure".as_slice(),
        crate::assemble::AssetKind::Table => b"table".as_slice(),
    };
    format!("analysis_v1_{:016x}", fnv1a(&[kind, model.as_bytes(), prompt.as_bytes(), asset.bytes.as_slice()]))
}

/// Render one page and detect its layout, producing everything needed to
/// OCR it. Called inline for the first page and — via `prefetched` — one
/// page ahead while the previous page's OCR requests are in flight, so
/// render + inference no longer stall the pipeline between pages.
///
/// Returns the page data plus the serialized layout to journal (the caller
/// records it, keeping this function free of cache mutation).
///
/// # Errors
///
/// Propagates render/inference failures; on prefetch these surface when
/// the loop reaches that page, i.e. exactly where inline preparation would
/// have raised them.
#[allow(clippy::too_many_arguments)]
fn prepare_page(
    page: usize,
    total: usize,
    det: &mut DetectorState,
    rasterizer: Option<&PdfiumRasterizer>,
    input: &Path,
    opts: &ImportOptions,
    cache: &HashMap<String, String>,
    emit: &mut dyn FnMut(Progress),
) -> Result<(PageData, Option<String>)> {
    // ---- render (PDFs) or load the image once ----
    let img: RgbaImage = match rasterizer {
        Some(rasterizer) => {
            let rgba = rasterizer.render_page((page - 1) as u32)?;
            // downscale right away (fixed-point bilinear — the image-crate
            // filters are an order of magnitude slower in debug builds)
            let scaled = downscale_if_needed(rgba, opts.max_side);
            emit(Progress::RenderedPage { page, total });
            scaled
        }
        None => {
            let img = image::open(input)
                .map_err(|e| OcrError::Input(format!("Could not read the image: {e}")))?;
            downscale_if_needed(img.to_rgba8(), opts.max_side)
        }
    };

    // ---- layout: stored results first, then inference; degrades to a
    // region-less page when the model/runtime is unavailable ----
    let mut ordered: Option<Vec<Region>> = None;
    let mut omitted = std::collections::HashSet::new();
    let mut journal_entry: Option<String> = None;
    if let Some(stored) = cache.get(&page_layout_key(page as u32)) {
        // layout already detected in a previous (interrupted) run — reuse
        // it instead of running inference again
        if let Ok(found) = serde_json::from_str::<Vec<Region>>(stored) {
            omitted = omitted_regions(&found);
            emit(Progress::LayoutPage {
                page,
                regions: found.len(),
                figures: count_figures(&found, &omitted),
                cached: true,
            });
            ordered = Some(found);
        }
    }
    if ordered.is_none() {
        ensure_detector(det, emit);
        // acquire the process-wide detector for THIS inference only: the
        // loaded session is cached process-wide, so concurrent imports
        // merely queue here for the (fast) layout pass instead of one
        // import holding the mutex until its last page
        let acquired = if det.failure.is_none() {
            det.resolved_model.as_deref().map(acquire_detector)
        } else {
            None
        };
        match acquired {
            Some(Ok(mut guard)) => {
                // single RGB conversion straight off the RGBA pixels (no extra
                // full-page clone)
                let rgb = rgba_to_rgb(&img);
                let detected = guard.detect_image(&rgb).map_err(|e| match e {
                    OcrError::LayoutInference { page: _, message } => OcrError::LayoutInference {
                        page: page as u32,
                        message,
                    },
                    other => other,
                })?;
                let found = order_regions(detected);
                omitted = omitted_regions(&found);
                let serialized = serde_json::to_string(&found).unwrap_or_default();
                emit(Progress::LayoutPage {
                    page,
                    regions: found.len(),
                    figures: count_figures(&found, &omitted),
                    cached: false,
                });
                ordered = Some(found);
                journal_entry = Some(serialized);
            }
            Some(Err(e)) => {
                // the one-time model load failed mid-import: degrade to
                // layout-unavailable rather than failing the page
                det.failure = Some(e.to_string());
                emit(Progress::LayoutUnavailable { reason: e.to_string() });
            }
            None => {} // layout unavailable — the page proceeds without regions
        }
    }
    Ok((
        PageData {
            img,
            ordered: ordered.unwrap_or_default(),
            omitted,
        },
        journal_entry,
    ))
}

/// Join the warm-up thread once per import, recording why layout cannot be
/// used (missing model/runtime) in `det.failure`. The process-wide detector
/// itself is acquired per inference at the detection site — parking the
/// guard here would hold the global detector mutex for the whole import and
/// serialize concurrent scans.
fn ensure_detector(det: &mut DetectorState, emit: &mut dyn FnMut(Progress)) {
    if det.tried {
        return;
    }
    det.tried = true;
    if let Some(handle) = det.model_thread.take() {
        match handle.join().expect("layout thread panicked") {
            Ok(()) => {}
            Err(reason) => det.failure = Some(reason),
        }
    }
    if let Some(reason) = &det.failure {
        emit(Progress::LayoutUnavailable { reason: reason.clone() });
    }
}

/// Stable cache key for a region's OCR result (pdfium_probe `cache_key`:
/// page + label + box rounded to 1 decimal, half-to-even).
fn region_cache_key(page_no: u32, r: &Region) -> String {
    fn round1(v: f32) -> f32 {
        (v * 10.0).round_ties_even() / 10.0
    }
    format!(
        "[{}, \"{}\", {:.1}, {:.1}, {:.1}, {:.1}]",
        page_no,
        r.label,
        round1(r.bbox[0]),
        round1(r.bbox[1]),
        round1(r.bbox[2]),
        round1(r.bbox[3])
    )
}

/// Cache key for a fully-completed page's assembled markdown (its
/// presence marks the page as cached: a re-run replays it without any
/// rework).
fn page_md_key(page_no: u32, analysis_namespace: Option<&str>) -> String {
    match analysis_namespace {
        Some(namespace) => format!("page_md_{page_no}_analysis_{namespace}"),
        None => format!("page_md_{page_no}"),
    }
}

fn page_is_cached(
    cache: &HashMap<String, String>,
    page_no: u32,
    analysis_namespace: Option<&str>,
) -> bool {
    cache.contains_key(&page_md_key(page_no, analysis_namespace))
}

fn page_chars_key(page_no: u32) -> String {
    format!("page_chars_{page_no}")
}

fn page_regions_key(page_no: u32) -> String {
    format!("page_regions_{page_no}")
}

/// Serialized layout regions for a page (JSON array of `Region`). Written
/// right after inference so an interrupted run never re-detects a page
/// whose layout was already computed.
fn page_layout_key(page_no: u32) -> String {
    format!("page_layout_{page_no}")
}

fn page_figures_key(page_no: u32) -> String {
    format!("page_figures_{page_no}")
}

/// Global figure counter value after this page — keeps asset numbering
/// consistent when cached pages are replayed before fresh ones.
fn page_figure_n_key(page_no: u32) -> String {
    format!("page_figure_n_{page_no}")
}

fn page_table_n_key(page_no: u32) -> String {
    format!("page_table_n_{page_no}")
}

/// Load the per-region OCR cache: the legacy single-JSON file (pre-journal
/// runs) is the base layer, then every `["key","value"]` record from the
/// append-only journal is applied on top (last write wins). Missing or
/// corrupt inputs are skipped, never fatal.
fn load_cache(journal: &Path, legacy: &Path) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = std::fs::read_to_string(legacy)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default();
    if let Ok(content) = std::fs::read_to_string(journal) {
        for line in content.lines() {
            if let Ok((key, value)) = serde_json::from_str::<(String, String)>(line) {
                map.insert(key, value);
            }
        }
    }
    map
}

/// Append the pending records to the journal (one JSON line each,
/// best effort like the previous whole-file save) and clear the queue.
fn flush_cache(journal: &Path, pending: &mut Vec<(String, String)>) {
    if pending.is_empty() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(journal) {
        let mut buf = String::new();
        for (key, value) in pending.iter() {
            if let Ok(line) = serde_json::to_string(&(key, value)) {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        let _ = file.write_all(buf.as_bytes());
    }
    pending.clear();
}

/// Insert a record into the in-memory cache and queue it for the next
/// journal flush.
fn record(
    cache: &mut HashMap<String, String>,
    pending: &mut Vec<(String, String)>,
    key: String,
    value: String,
) {
    cache.insert(key.clone(), value.clone());
    pending.push((key, value));
}

/// Append one page's markdown to content.md, inserting the `\n\n---\n\n`
/// separator between pages (exactly what `pages_md.join(...)` produced).
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when the write fails.
fn append_page_md(
    file: &mut impl std::io::Write,
    pages_written: &mut usize,
    md: &str,
) -> std::io::Result<()> {
    if *pages_written > 0 {
        file.write_all(b"\n\n---\n\n")?;
    }
    file.write_all(md.as_bytes())?;
    *pages_written += 1;
    Ok(())
}

/// Non-omitted figure/table-kind assets of a page. The historical
/// `figures` field remains the event name, but its count includes tables.
fn count_figures(ordered: &[Region], omitted: &std::collections::HashSet<usize>) -> usize {
    ordered
        .iter()
        .enumerate()
        .filter(|(idx, r)| crate::assemble::is_asset_region(r) && !omitted.contains(idx))
        .count()
}

/// RGB copy of a page bitmap — one allocation read straight off the RGBA
/// pixels (the previous `DynamicImage::ImageRgba8(img.clone()).to_rgb8()`
/// made an avoidable full-page clone first).
fn rgba_to_rgb(rgba: &RgbaImage) -> image::RgbImage {
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, dst) in out.enumerate_pixels_mut() {
        let src = rgba.get_pixel(x, y);
        *dst = image::Rgb([src[0], src[1], src[2]]);
    }
    out
}

/// Downscale a page bitmap to the max side with the fixed-point bilinear
/// port (fast in debug, INTER_LINEAR semantics like the reference).
fn downscale_if_needed(rgba: RgbaImage, max_side: u32) -> RgbaImage {
    if max_side == 0 {
        return rgba;
    }
    let (w, h) = (rgba.width(), rgba.height());
    let max = w.max(h);
    if max <= max_side {
        return rgba;
    }
    let scale = f64::from(max_side) / f64::from(max);
    let nw = ((f64::from(w) * scale).round()).max(1.0) as u32;
    let nh = ((f64::from(h) * scale).round()).max(1.0) as u32;
    crate::preprocess::resize_fixed_point(&rgba, nw, nh)
}

/// Warm the process-wide layout-model cache (called by `scan_prepare` so
/// the first import never waits on the slow session creation).
///
/// # Errors
///
/// Returns an error when the model or runtime is unavailable — the
/// caller decides whether to surface it.
pub fn warm_layout(models_dir: &Path) -> Result<()> {
    let model = resolve_in_models(models_dir, MODEL_FILE, "DOCFOO_LAYOUT_MODEL")?;
    let ort_dll = resolve_in_models(models_dir, ORT_DLL_FILE, "DOCFOO_ORT_DLL")?;
    ensure_onnx_runtime(&ort_dll)?;
    acquire_detector(&model).map(|_| ())
}

/// Resolve a models-dir file, honoring an env-var override.
fn resolve_in_models(models_dir: &Path, file: &str, env: &str) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(env) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    let p = models_dir.join(file);
    if p.is_file() {
        return Ok(p);
    }
    Err(OcrError::MissingDependency(format!(
        "The document scanner could not find {} — it should sit next to the app in the models folder.",
        p.display()
    )))
}

/// Keep only safe characters for a folder name. Public so the app layer
/// can compute the same output folder name the pipeline will use (for
/// "open in Resources" links).
#[must_use]
pub fn sanitize_stem(stem: String) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "document".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::RegionKind;

    #[test]
    fn cache_key_is_stable_and_distinct() {
        let a = Region {
            kind: RegionKind::Text,
            label: "text".to_string(),
            confidence: 0.9,
            bbox: [100.04, 200.06, 300.0, 400.0],
            order: 1,
        };
        let b = Region {
            kind: RegionKind::Text,
            label: "text".to_string(),
            confidence: 0.9,
            bbox: [100.05, 200.15, 300.0, 400.0],
            order: 2,
        };
        let k1 = region_cache_key(7, &a);
        let k2 = region_cache_key(7, &b);
        assert_eq!(k1, "[7, \"text\", 100.0, 200.1, 300.0, 400.0]");
        assert_ne!(k1, k2); // half-to-even rounding separates .04/.05
        assert_ne!(k1, region_cache_key(8, &a));
    }

    #[test]
    fn cache_journal_roundtrip_and_legacy_merge() {
        let dir = std::env::temp_dir().join("docfoo-cache-journal-test");
        std::fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("ocr_cache.jsonl");
        let legacy = dir.join("ocr_cache.json");
        let _ = std::fs::remove_file(&journal);
        let _ = std::fs::remove_file(&legacy);

        // legacy single-JSON file loads as the base layer
        std::fs::write(&legacy, r#"{"a":"1"}"#).unwrap();
        let loaded = load_cache(&journal, &legacy);
        assert_eq!(loaded.get("a").map(String::as_str), Some("1"));

        // records append to the journal; reload merges legacy + journal
        // with last-write-wins
        let mut pending = vec![("b".to_string(), "2".to_string())];
        flush_cache(&journal, &mut pending);
        assert!(pending.is_empty());
        let mut pending = vec![("a".to_string(), "9".to_string()), ("c".to_string(), "3".to_string())];
        flush_cache(&journal, &mut pending);
        let merged = load_cache(&journal, &legacy);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged.get("a").map(String::as_str), Some("9"));
        assert_eq!(merged.get("b").map(String::as_str), Some("2"));
        assert_eq!(merged.get("c").map(String::as_str), Some("3"));

        // corrupt journal lines are skipped, never fatal
        let mut file = std::fs::OpenOptions::new().append(true).open(&journal).unwrap();
        file.write_all(b"not json{{\n").unwrap();
        drop(file);
        let merged = load_cache(&journal, &legacy);
        assert_eq!(merged.len(), 3);

        // corrupt legacy file -> empty base layer (journal still applies)
        std::fs::write(&legacy, "not json{{").unwrap();
        let merged = load_cache(&journal, &legacy);
        assert_eq!(merged.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_md_streaming_joins_with_separators() {
        let mut buf: Vec<u8> = Vec::new();
        let mut count = 0usize;
        append_page_md(&mut buf, &mut count, "one").unwrap();
        append_page_md(&mut buf, &mut count, "two").unwrap();
        append_page_md(&mut buf, &mut count, "three").unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "one\n\n---\n\ntwo\n\n---\n\nthree");
        assert_eq!(count, 3);
    }

    #[test]
    fn analysis_cache_key_distinguishes_asset_model_prompt_and_content() {
        let asset = AssetCrop {
            region_index: 0,
            kind: crate::assemble::AssetKind::Figure,
            name: "figure_1.png".to_string(),
            bytes: Arc::new(vec![1, 2, 3]),
        };
        let same_content_other_model = analysis_cache_key(&asset, "model-b", "prompt-a");
        let other_prompt = analysis_cache_key(&asset, "model-a", "prompt-b");
        let other_content = AssetCrop {
            bytes: Arc::new(vec![1, 2, 4]),
            ..asset.clone()
        };
        assert_ne!(analysis_cache_key(&asset, "model-a", "prompt-a"), same_content_other_model);
        assert_ne!(analysis_cache_key(&asset, "model-a", "prompt-a"), other_prompt);
        assert_ne!(
            analysis_cache_key(&asset, "model-a", "prompt-a"),
            analysis_cache_key(&other_content, "model-a", "prompt-a")
        );
        // the namespace is stable across runs: model + prompt only (the
        // endpoint/proxy port is deliberately not part of it)
        assert_eq!(analysis_cache_namespace("model-a", "prompt-a"), analysis_cache_namespace("model-a", "prompt-a"));
        assert_ne!(
            analysis_cache_namespace("model-a", "prompt-a"),
            analysis_cache_namespace("model-b", "prompt-a")
        );
    }

    #[test]
    fn count_includes_tables_and_figures() {
        let regions = vec![
            Region {
                kind: RegionKind::Figure,
                label: "chart".to_string(),
                confidence: 1.0,
                bbox: [0.0; 4],
                order: 0,
            },
            Region {
                kind: RegionKind::Table,
                label: "table".to_string(),
                confidence: 1.0,
                bbox: [0.0; 4],
                order: 1,
            },
        ];
        assert_eq!(count_figures(&regions, &std::collections::HashSet::new()), 2);
    }

    enum StubFail {
        Transport(&'static str),
        Model(&'static str),
    }

    enum StubBehavior {
        Ok(&'static str),
        TransportOnceThenOk(&'static str),
        Fail(StubFail),
    }

    struct StubClient {
        calls: std::sync::atomic::AtomicUsize,
        behavior: StubBehavior,
    }

    impl StubClient {
        fn new(behavior: StubBehavior) -> Self {
            Self { calls: std::sync::atomic::AtomicUsize::new(0), behavior }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CompletionClient for StubClient {
        fn complete_image(
            &self,
            _model: &str,
            _prompt: &str,
            _image: &[u8],
            _mime: &str,
            _cancel: Option<&AtomicBool>,
        ) -> Result<String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                StubBehavior::Ok(text) => Ok((*text).to_string()),
                StubBehavior::TransportOnceThenOk(text) => {
                    if n == 0 {
                        Err(OcrError::Transport("network down".to_string()))
                    } else {
                        Ok((*text).to_string())
                    }
                }
                StubBehavior::Fail(StubFail::Transport(message)) => Err(OcrError::Transport((*message).to_string())),
                StubBehavior::Fail(StubFail::Model(message)) => Err(OcrError::OcrPage {
                    page: 0,
                    message: (*message).to_string(),
                }),
            }
        }
    }

    #[test]
    fn completion_retries_transport_failures_once() {
        let client = StubClient::new(StubBehavior::TransportOnceThenOk("recovered"));
        let out = complete_with_retry(&client, "m", "p", &[1], "image/png", None).unwrap();
        assert_eq!(out, "recovered");
        assert_eq!(client.calls(), 2);
    }

    #[test]
    fn completion_gives_up_after_two_transport_failures() {
        let client = StubClient::new(StubBehavior::Fail(StubFail::Transport("down")));
        let error = complete_with_retry(&client, "m", "p", &[1], "image/png", None).unwrap_err();
        assert!(matches!(error, OcrError::Transport(_)));
        assert_eq!(client.calls(), 2);
    }

    #[test]
    fn completion_does_not_retry_model_errors() {
        let client = StubClient::new(StubBehavior::Fail(StubFail::Model("bad model")));
        let error = complete_with_retry(&client, "m", "p", &[1], "image/png", None).unwrap_err();
        assert!(matches!(error, OcrError::OcrPage { .. }));
        assert_eq!(client.calls(), 1);
    }

    #[test]
    fn completion_rejects_empty_responses_without_retry() {
        let client = StubClient::new(StubBehavior::Ok("   "));
        let error = complete_with_retry(&client, "m", "p", &[1], "image/png", None).unwrap_err();
        assert!(error.to_string().contains("empty response"));
        assert_eq!(client.calls(), 1);
    }

    #[test]
    fn analyze_assets_records_descriptions() {
        let assets = vec![
            AssetCrop {
                region_index: 0,
                kind: crate::assemble::AssetKind::Figure,
                name: "figure_1.png".to_string(),
                bytes: Arc::new(vec![1, 2, 3]),
            },
            AssetCrop {
                region_index: 1,
                kind: crate::assemble::AssetKind::Table,
                name: "table_1.png".to_string(),
                bytes: Arc::new(vec![4, 5, 6]),
            },
        ];
        let config = ImageAnalysisOptions { model: "test/model".to_string(), prompt: "describe".to_string() };
        let client: Arc<dyn CompletionClient> = Arc::new(StubClient::new(StubBehavior::Ok("A table of values.")));
        let batch = analyze_assets(&assets, &config, &HashMap::new(), &client, 2, None, None);
        assert!(!batch.failed && !batch.cancelled);
        assert!(batch.error.is_none());
        assert_eq!(batch.descriptions.len(), 2);
        assert_eq!(batch.records.len(), 2);
        assert_eq!(batch.descriptions.get(&1).map(String::as_str), Some("A table of values."));
    }

    #[test]
    fn analyze_assets_reports_failure_without_records() {
        let assets = vec![AssetCrop {
            region_index: 0,
            kind: crate::assemble::AssetKind::Figure,
            name: "figure_1.png".to_string(),
            bytes: Arc::new(vec![1, 2, 3]),
        }];
        let config = ImageAnalysisOptions { model: "test/model".to_string(), prompt: "describe".to_string() };
        let client: Arc<dyn CompletionClient> = Arc::new(StubClient::new(StubBehavior::Fail(StubFail::Model("no vision"))));
        let batch = analyze_assets(&assets, &config, &HashMap::new(), &client, 1, None, None);
        assert!(batch.failed);
        assert!(batch.records.is_empty());
        assert!(batch.descriptions.is_empty());
        assert!(batch.error.as_deref().is_some_and(|error| error.contains("no vision")));
    }

    #[test]
    fn analysis_failed_event_serializes_with_type_tag() {
        let value = serde_json::to_value(Progress::AnalysisFailed {
            page: 2,
            assets: 3,
            message: "400 Invalid request parameters".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "analysis_failed");
        assert_eq!(value["page"], 2);
        assert_eq!(value["assets"], 3);
        assert_eq!(value["message"], "400 Invalid request parameters");
    }

    #[test]
    fn analysis_is_disabled_by_default() {
        let client: Arc<dyn CompletionClient> = Arc::new(StubClient::new(StubBehavior::Ok("x")));
        assert!(ImportOptions::new(client).analysis.is_none());
        assert_eq!(COMPLETION_ATTEMPTS, 2);
        assert_eq!(effective_analysis_prompt(" \n\t "), crate::ocr::IMAGE_ANALYSIS_PROMPT);
    }

    #[test]
    fn rgba_to_rgb_drops_alpha_without_clone() {
        let mut rgba = RgbaImage::new(2, 2);
        for (x, y, px) in rgba.enumerate_pixels_mut() {
            *px = image::Rgba([x as u8, y as u8, 7, 255]);
        }
        let rgb = rgba_to_rgb(&rgba);
        assert_eq!(rgb.width(), 2);
        assert_eq!(*rgb.get_pixel(1, 1), image::Rgb([1, 1, 7]));
    }
}
