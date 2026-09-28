//! Completion client abstraction.
//!
//! The pipeline never talks to a provider itself: every region OCR and asset
//! analysis request goes through a [`CompletionClient`]. The DocFoo app
//! supplies a client that runs the request through pi's runtime (agent
//! bridge); the examples and benches use the built-in [`HttpCompletionClient`]
//! for OpenAI-compatible endpoints.

use std::sync::atomic::AtomicBool;

use crate::error::Result;

/// One image+prompt completion.
///
/// Implementations own auth, transport, provider-specific headers and model
/// resolution. `model` is the caller's model key (the app passes
/// `provider/modelId`); `image` is raw PNG/JPEG bytes and `mime` its type.
pub trait CompletionClient: Send + Sync {
    /// Complete `prompt` over `image` with `model`, returning the assistant text.
    ///
    /// # Errors
    ///
    /// [`crate::OcrError::Transport`] for retryable network/timeout failures,
    /// [`crate::OcrError::Cancelled`] when `cancel` is set, other variants for
    /// model/provider errors (never retried by the pipeline).
    fn complete_image(
        &self,
        model: &str,
        prompt: &str,
        image: &[u8],
        mime: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<String>;
}

/// Content type of an image byte slice (PNG or JPEG), for clients that need
/// the mime type alongside the bytes.
#[must_use]
pub fn image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// OpenAI-compatible chat-completions client for examples, benches and local
/// servers. Keeps the crate's historical retry/backoff behavior inside the
/// HTTP layer; the app path uses the pi bridge instead.
pub struct HttpCompletionClient {
    endpoint: String,
    api_key: String,
    headers: Vec<(String, String)>,
    agent: ureq::Agent,
}

impl HttpCompletionClient {
    /// Build a client for `endpoint` (full `/chat/completions` URL) with an
    /// optional bearer key (`""` sends no Authorization header).
    #[must_use]
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            headers: Vec::new(),
            agent: crate::ocr::ocr_agent(),
        }
    }

    /// Extra provider headers, applied last (same contract as the old
    /// `ImageAnalysisOptions::headers`).
    #[must_use]
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }
}

impl CompletionClient for HttpCompletionClient {
    fn complete_image(
        &self,
        model: &str,
        prompt: &str,
        image: &[u8],
        _mime: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<String> {
        crate::ocr::ocr_region_with_prompt(
            &self.agent,
            image,
            &self.api_key,
            &self.endpoint,
            model,
            prompt,
            &self.headers,
            cancel,
        )
    }
}
