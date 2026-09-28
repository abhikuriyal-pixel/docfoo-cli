//! PP-DocLayoutV3 detection (port of owl-ocr's `layout/mod.rs`, V3 only).
//!
//! The model consumes plain `[0, 1]` RGB pixels resized to 800x800 and is
//! fed the real resize factors as `scale_factor`, so its boxes come out
//! directly in original-image pixel coordinates. Rows are 7 floats wide:
//! `[label, score, xmin, ymin, xmax, ymax, order_seq]`; the reading-order
//! column is ignored (the analyze module derives reading order from the
//! boxes, like the reference pipeline).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};

use ndarray::Array2;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::error::{OcrError, Result};
use crate::preprocess::{INPUT_SIZE, INPUT_SIZE_F32, image_to_blob};

/// Detection confidence threshold; boxes below it are dropped.
const SCORE_THRESHOLD: f32 = 0.5;

/// PP-DocLayoutV3 label list (25 classes), verbatim from the official
/// `PP-DocLayoutV3_infer/inference.yml` `label_list`, in index order.
const V3_LABELS: [&str; 25] = [
    "abstract",
    "algorithm",
    "aside_text",
    "chart",
    "content",
    "display_formula",
    "doc_title",
    "figure_title",
    "footer",
    "footer_image",
    "footnote",
    "formula_number",
    "header",
    "header_image",
    "image",
    "inline_formula",
    "number",
    "paragraph_title",
    "reference",
    "reference_content",
    "seal",
    "table",
    "text",
    "vertical_text",
    "vision_footnote",
];

/// Semantic kind of a detected layout region (the coarse grouping the
/// pipeline cares about).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RegionKind {
    Title,
    Text,
    Table,
    Equation,
    Figure,
    Reference,
    Footnote,
    Header,
    Footer,
    Algorithm,
    Other,
}

/// Map a PP-DocLayoutV3 label name to its semantic kind (port of
/// owl-ocr's `RegionKind::from_v3_name`).
pub fn kind_from_v3_name(name: &str) -> RegionKind {
    match name {
        "abstract" | "aside_text" | "content" | "reference_content" | "text"
        | "vertical_text" => RegionKind::Text,
        "algorithm" => RegionKind::Algorithm,
        "chart" | "footer_image" | "header_image" | "image" => RegionKind::Figure,
        "display_formula" | "formula_number" | "inline_formula" => RegionKind::Equation,
        "doc_title" | "figure_title" | "paragraph_title" => RegionKind::Title,
        "footer" => RegionKind::Footer,
        "footnote" | "vision_footnote" => RegionKind::Footnote,
        "header" => RegionKind::Header,
        "reference" => RegionKind::Reference,
        "table" => RegionKind::Table,
        _ => RegionKind::Other, // number, seal, unknown
    }
}

/// A detected layout region in original-image pixel coordinates (floats,
/// as emitted by the model — the exact values the reference pipeline
/// works with).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Region {
    /// Semantic kind.
    pub kind: RegionKind,
    /// The model's fine-grained label (e.g. `"display_formula"`).
    pub label: String,
    /// Model confidence in `[0, 1]`.
    pub confidence: f32,
    /// Raw box coordinates: `[xmin, ymin, xmax, ymax]`.
    pub bbox: [f32; 4],
    /// The model's predicted reading-order index (V3 `order_seq` column),
    /// 0 when the output rows carry no order column.
    pub order: i32,
}

/// The ONNX Runtime dll loaded before any other ort API use. The path is
/// supplied by the caller (e.g. the `models/` dir next to the app).
static ORT_DLL: Mutex<Option<String>> = Mutex::new(None);
static ORT_ONCE: Once = Once::new();
static ORT_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Bind the explicitly chosen ONNX Runtime dll exactly once per process.
///
/// # Errors
///
/// Returns [`OcrError::MissingDependency`] when the dll cannot be loaded.
pub fn ensure_onnx_runtime(dll_path: &Path) -> Result<()> {
    ORT_ONCE.call_once(|| {
        let path = dll_path.to_path_buf();
        match ort::init_from(&path) {
            Ok(builder) => {
                if !builder.commit() {
                    *ORT_ERROR.lock().unwrap() = Some(
                        "ONNX Runtime environment was already configured".to_string(),
                    );
                } else {
                    *ORT_DLL.lock().unwrap() = Some(path.display().to_string());
                }
            }
            Err(err) => *ORT_ERROR.lock().unwrap() = Some(err.to_string()),
        }
    });
    if let Some(message) = ORT_ERROR.lock().unwrap().clone() {
        return Err(OcrError::MissingDependency(format!(
            "The helper's document scanner could not start (ONNX Runtime: {message})"
        )));
    }
    Ok(())
}

/// A loaded PP-DocLayoutV3 ONNX session.
pub struct LayoutDetector {
    session: Session,
}

/// Process-wide detector cache: session creation from the 130 MB model is
/// the single slowest step (tens of seconds with ORT's default graph
/// optimization), so a loaded detector is kept for the process lifetime
/// and reused by every import. Reloaded only when the model path changes.
static DETECTOR: OnceLock<Mutex<Option<(PathBuf, LayoutDetector)>>> = OnceLock::new();

/// Guard handing out the shared detector; reloads the session on first
/// use or when the model path changed.
pub struct DetectorGuard<'a> {
    guard: std::sync::MutexGuard<'a, Option<(PathBuf, LayoutDetector)>>,
}

impl DetectorGuard<'_> {
    /// Run layout detection on RGB pixels with the shared session.
    ///
    /// # Errors
    ///
    /// Returns [`OcrError::LayoutInference`] on inference failure.
    pub fn detect_image(&mut self, image: &image::RgbImage) -> Result<Vec<Region>> {
        self.guard
            .as_mut()
            .expect("detector present after acquire")
            .1
            .detect_image(image)
    }
}

/// Acquire the process-wide detector for `model_path`, creating the ONNX
/// session on first use (and caching it). The onnx runtime dll must be
/// bound with [`ensure_onnx_runtime`] before the first call.
///
/// # Errors
///
/// Returns [`OcrError::MissingDependency`] when the model cannot be
/// loaded; a failed load is not cached.
pub fn acquire_detector(model_path: &Path) -> Result<DetectorGuard<'static>> {
    let slot = DETECTOR.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let stale = match &*guard {
        Some((p, _)) => p.as_path() != model_path,
        None => true,
    };
    if stale {
        let detector = LayoutDetector::load(model_path)?;
        *guard = Some((model_path.to_path_buf(), detector));
    }
    Ok(DetectorGuard { guard })
}

impl LayoutDetector {
    /// Load the V3 model from `model_path`.
    ///
    /// Session creation from the 130 MB model is the slowest single step
    /// of the pipeline: ORT's default `Level3` graph optimization spends
    /// tens of seconds constant-folding and layout-transforming the graph
    /// at commit time. The chosen level is the lightest that keeps
    /// inference speed intact (`Level1`; `Disable` via
    /// `DOCFOO_ORT_OPT_LEVEL=disable` if even that is too slow) — the
    /// session is created once per process and cached, so the cost is
    /// paid at most once anyway.
    ///
    /// # Errors
    ///
    /// Returns [`OcrError::MissingDependency`] when the model cannot be
    /// loaded.
    pub fn load(model_path: &Path) -> Result<Self> {
        let level = match std::env::var("DOCFOO_ORT_OPT_LEVEL").as_deref() {
            Ok("disable") => GraphOptimizationLevel::Disable,
            _ => GraphOptimizationLevel::Level1,
        };
        let mut builder = Session::builder()
            .map_err(|err| OcrError::MissingDependency(err.to_string()))?;
        builder = builder
            .with_optimization_level(level)
            .map_err(|err| OcrError::MissingDependency(err.to_string()))?;
        if let Ok(threads) = std::thread::available_parallelism() {
            let threads = threads.get().max(1);
            builder = builder
                .with_intra_threads(threads)
                .map_err(|err| OcrError::MissingDependency(err.to_string()))?;
        }
        let session = builder
            .commit_from_file(model_path)
            .map_err(|err| {
                OcrError::MissingDependency(format!(
                    "Could not load the layout model ({}): {err}",
                    model_path.display()
                ))
            })?;
        Ok(Self { session })
    }

    /// Run layout detection on RGB pixels (original page size) and return
    /// detected regions in original-image pixel coordinates.
    ///
    /// # Errors
    ///
    /// Returns [`OcrError::LayoutInference`] on any inference failure.
    pub fn detect_image(&mut self, image: &image::RgbImage) -> Result<Vec<Region>> {
        let (orig_w, orig_h) = (image.width(), image.height());
        let blob = image_to_blob(image);
        let input_shape = Array2::<f32>::from_shape_vec((1, 2), vec![INPUT_SIZE_F32; 2])
            .map_err(|err| OcrError::LayoutInference { page: 0, message: err.to_string() })?;
        // The REAL resize factors `[[800/h, 800/w]]`: with this feed the
        // model outputs boxes directly in original pixel coordinates.
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let scale_h = (f64::from(INPUT_SIZE) / f64::from(orig_h)) as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let scale_w = (f64::from(INPUT_SIZE) / f64::from(orig_w)) as f32;
        let scale_factor = Array2::<f32>::from_shape_vec((1, 2), vec![scale_h, scale_w])
            .map_err(|err| OcrError::LayoutInference { page: 0, message: err.to_string() })?;

        let mut inputs: HashMap<&str, Tensor<f32>> = HashMap::new();
        inputs.insert(
            "image",
            Tensor::from_array(blob).map_err(|e| ort_err(&e))?,
        );
        inputs.insert(
            "im_shape",
            Tensor::from_array(input_shape).map_err(|e| ort_err(&e))?,
        );
        inputs.insert(
            "scale_factor",
            Tensor::from_array(scale_factor).map_err(|e| ort_err(&e))?,
        );
        let mut feed: Vec<(String, SessionInputValue)> = Vec::new();
        for input in self.session.inputs() {
            let name = input.name().to_owned();
            let tensor = inputs.remove(name.as_str()).ok_or_else(|| {
                OcrError::LayoutInference {
                    page: 0,
                    message: format!("model declares unknown input {name:?}"),
                }
            })?;
            feed.push((name, tensor.into()));
        }

        let outputs = self
            .session
            .run(feed)
            .map_err(|err| OcrError::LayoutInference { page: 0, message: err.to_string() })?;
        let output = outputs
            .into_iter()
            .next()
            .ok_or_else(|| OcrError::LayoutInference {
                page: 0,
                message: "model produced no outputs".to_owned(),
            })?
            .1;
        let (out_shape, out_data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| ort_err(&e))?;

        // V3 emits `(300, 7)` or `(1, 300, 7)` detection rows.
        let stride = out_shape
            .get(2)
            .and_then(|dim| usize::try_from(*dim).ok())
            .unwrap_or(7);
        Ok(parse_detections(&out_data, stride, orig_w, orig_h))
    }
}

fn ort_err(err: &ort::Error) -> OcrError {
    OcrError::LayoutInference { page: 0, message: err.to_string() }
}

/// Turn the model's flat detection rows into regions (port of owl-ocr's
/// `parse_detections`, V3 semantics). Rows are `[label, score, xmin,
/// ymin, xmax, ymax, order_seq]`; the reading order column is ignored.
fn parse_detections(rows: &[f32], stride: usize, orig_w: u32, orig_h: u32) -> Vec<Region> {
    let mut regions = Vec::new();
    if stride < 6 {
        return regions;
    }
    let mut i = 0;
    while i + 5 < rows.len() {
        let label = rows[i];
        let score = rows[i + 1];
        if score > SCORE_THRESHOLD && label >= 0.0 {
            let xmin = rows[i + 2];
            let ymin = rows[i + 3];
            let xmax = rows[i + 4];
            let ymax = rows[i + 5];
            // Degenerate (zero-area) boxes are dropped like the reference.
            if xmax > xmin && ymax > ymin {
                let idx = label.round_ties_even() as usize;
                let name = V3_LABELS.get(idx).copied().unwrap_or("other");
                regions.push(Region {
                    kind: kind_from_v3_name(name),
                    label: name.to_string(),
                    confidence: score,
                    bbox: [xmin, ymin, xmax, ymax],
                    order: if stride >= 7 {
                        rows[i + 6].round_ties_even() as i32
                    } else {
                        0
                    },
                });
            }
        }
        i += stride;
    }
    let _ = (orig_w, orig_h); // boxes are already in page space
    regions
}
