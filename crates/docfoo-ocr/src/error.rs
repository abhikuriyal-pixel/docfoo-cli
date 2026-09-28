//! Error type of the OCR pipeline. Every variant carries a plain-English
//! message suitable for the non-technical end user.

/// Result alias for the crate.
pub type Result<T> = std::result::Result<T, OcrError>;

/// Pipeline errors, all user-friendly.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// The input document could not be read (bad PDF, missing file, …).
    #[error("{0}")]
    Input(String),

    /// A runtime dependency (pdfium.dll, onnxruntime.dll, the layout
    /// model) could not be found or loaded.
    #[error("{0}")]
    MissingDependency(String),

    /// Layout detection failed for a page.
    #[error("Layout detection failed on page {page}: {message}")]
    LayoutInference { page: u32, message: String },

    /// The OCR request failed for a page after all retries.
    #[error("OCR failed on page {page}: {message}")]
    OcrPage { page: u32, message: String },

    /// A network/timeout failure talking to the completion provider. This is
    /// the only error the pipeline retries.
    #[error("{0}")]
    Transport(String),

    /// The user stopped an in-flight OCR request.
    #[error("OCR stopped")]
    Cancelled,

    /// The output could not be written.
    #[error("Could not save the result: {0}")]
    Output(String),

    /// Anything else, already user-friendly.
    #[error("{0}")]
    Other(String),
}
