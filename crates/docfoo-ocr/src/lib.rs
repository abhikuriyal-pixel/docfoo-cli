//! DocFoo Scan — the exact pipeline of `mimo-v2.5-ocr/ocr_to_markdown.py`,
//! ported to Rust:
//!
//! 1. PDF pages are rendered to images with PDFium (300 dpi, downscaled
//!    to a max side); plain images (png/jpg/…) are used as-is.
//! 2. PP-DocLayoutV3 (local ONNX) detects layout regions and orders them
//!    column-major (columns left→right, within each column top→bottom) —
//!    the exact order the LLM is told to read in.
//! 3. The selected Text model OCRs each eligible region independently with a
//!    strict transcription prompt (asset OCR text is not rendered separately).
//! 4. Detected figures, charts, images, and tables are saved as embedded PNG
//!    assets. Optional Figures & Tables analysis describes those exact saved
//!    crops with a separate model and replaces only their generic alt text.
//! 5. Pages are processed through a bounded worker pipeline; per-page
//!    markdown is stitched with `---` and resumable journal caches.

pub mod analyze;
pub mod assemble;
pub mod client;
pub mod error;
pub mod layout;
pub mod ocr;
pub mod pipeline;
pub mod preprocess;
pub mod raster;

pub use error::{OcrError, Result};
pub use client::{image_mime, CompletionClient, HttpCompletionClient};
pub use pipeline::{
    layout_model_file_name, ort_library_file_name, pdfium_library_file_name, run_import,
    sanitize_stem, warm_layout, ImageAnalysisOptions, ImportOptions, ImportOutput, OcrPermit,
    OcrPermitPool, Progress,
};
pub use preprocess::{image_to_blob as preprocess_blob, resize_fixed_point};
