//! PDF page rasterization via PDFium (port of owl-doc's `raster.rs`):
//! render one page at a time at a configurable DPI, producing an
//! [`image::RgbaImage`] per page. pdfium-render only allows one `Pdfium`
//! per process, so the bindings live in a process-global slot.

use std::sync::OnceLock;

use image::RgbaImage;
use pdfium_render::prelude::*;

use crate::error::{OcrError, Result};

/// The process-global `PDFium` instance.
static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();

/// Serializes every pdfium call process-wide. PDFium is not thread-safe
/// unless the binary was built with multithreading support, and concurrent
/// scan workspaces share this one instance — rendering is fast, so a lock
/// costs almost nothing and keeps parallel imports safe.
static PDFIUM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A PDF opened for rendering; each page is rendered on demand from the
/// byte buffer (a fresh `PdfDocument` per page, like the reference).
pub struct PdfiumRasterizer {
    pdfium: &'static Pdfium,
    bytes: Vec<u8>,
    dpi: u32,
    pages: u32,
}

impl PdfiumRasterizer {
    /// Bind pdfium (once per process) and load `data` for rendering at
    /// `dpi` pixels per inch.
    ///
    /// # Errors
    ///
    /// Returns [`OcrError::MissingDependency`] when the dll cannot be
    /// bound; [`OcrError::Input`] when the bytes are not a readable PDF.
    pub fn new(data: Vec<u8>, dpi: u32, dll_path: &std::path::Path) -> Result<Self> {
        let pdfium = match PDFIUM.get_or_init(|| {
            match Pdfium::bind_to_library(dll_path) {
                Ok(bindings) => Ok(Pdfium::new(bindings)),
                Err(error) => Err(format!("pdfium bind {}: {error}", dll_path.display())),
            }
        }) {
            Ok(pdfium) => pdfium,
            Err(message) => {
                return Err(OcrError::MissingDependency(format!(
                    "The document scanner could not start (PDF reader: {message})"
                )))
            }
        };
        let doc = {
            let _lock = PDFIUM_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            pdfium
                .load_pdf_from_byte_slice(&data, None)
                .map_err(|e| OcrError::Input(format!("Could not read the PDF: {e}")))?
        };
        let pages = u32::try_from(doc.pages().len())
            .map_err(|_| OcrError::Input("page count does not fit in u32".to_owned()))?;
        drop(doc);
        Ok(Self {
            pdfium,
            bytes: data,
            dpi: dpi.max(1),
            pages,
        })
    }

    /// Total number of pages.
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.pages
    }

    /// Render 0-based page `index` into an RGBA bitmap.
    ///
    /// # Errors
    ///
    /// Returns [`OcrError::Input`] when the page cannot be rendered.
    pub fn render_page(&self, index: u32) -> Result<RgbaImage> {
        // every pdfium touch happens under the process-wide lock (see
        // PDFIUM_LOCK): load, page access, render and bitmap conversion
        let _lock = PDFIUM_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let doc = self
            .pdfium
            .load_pdf_from_byte_slice(&self.bytes, None)
            .map_err(|e| OcrError::Input(format!("PDF reload failed: {e}")))?;
        let idx = i32::try_from(index).unwrap_or(i32::MAX);
        let page = doc
            .pages()
            .get(idx)
            .map_err(|e| OcrError::Input(format!("page {index}: {e}")))?;
        // Target size = page points * dpi/72, rounded — the reference's
        // exact render config.
        #[allow(clippy::cast_precision_loss)] // dpi is far below f32's exact range
        let scale = self.dpi as f32 / 72.0;
        #[allow(clippy::cast_possible_truncation)] // rounded, then exact-sized
        let tw = (page.width().value * scale).round() as i32;
        #[allow(clippy::cast_possible_truncation)] // rounded, then exact-sized
        let th = (page.height().value * scale).round() as i32;
        let config = PdfRenderConfig::new()
            .set_target_width(tw)
            .set_target_height(th);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| OcrError::Input(format!("page {index} render failed: {e}")))?;
        let img = bitmap
            .as_image()
            .map_err(|e| OcrError::Input(format!("page {index} bitmap failed: {e}")))?;
        Ok(img.to_rgba8())
    }
}
