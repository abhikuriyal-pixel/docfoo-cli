//! Scan pipeline driver — Tauri-free port of `src-tauri/src/scan/mod.rs`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use base64::Engine as _;
use docfoo_ocr::{CompletionClient, ImageAnalysisOptions, ImportOptions, ImportOutput, Progress};

use crate::error::{CliError, Result};
use crate::sidecar::{CompletionRequest, SidecarClient, SidecarError, DEFAULT_TIMEOUT};
use crate::workspace::Workspace;

/// [`docfoo_ocr::CompletionClient`] backed by the Pi sidecar completion
/// service. Images ride as OpenAI-style data URLs; the sidecar maps them back
/// to pi content parts.
pub struct SidecarCompletionClient {
    sidecar: Arc<SidecarClient>,
}

impl SidecarCompletionClient {
    pub fn new(sidecar: Arc<SidecarClient>) -> Self {
        Self { sidecar }
    }
}

impl CompletionClient for SidecarCompletionClient {
    fn complete_image(
        &self,
        model: &str,
        prompt: &str,
        image: &[u8],
        mime: &str,
        cancel: Option<&AtomicBool>,
    ) -> docfoo_ocr::Result<String> {
        let data = base64::engine::general_purpose::STANDARD.encode(image);
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } }
            ]
        })];
        let mut request = CompletionRequest::new(model, messages);
        // Reasoning and the answer share this budget; models that cannot
        // disable thinking spend part of it before the transcription.
        request.max_tokens = Some(8192);
        request.reasoning = Some("off".to_string());
        self.sidecar
            .complete(request, cancel, None, DEFAULT_TIMEOUT)
            .map_err(bridge_failure)
    }
}

fn bridge_failure(error: SidecarError) -> docfoo_ocr::OcrError {
    match error {
        SidecarError::Cancelled => docfoo_ocr::OcrError::Cancelled,
        SidecarError::Transport(message) | SidecarError::Spawn(message) => {
            docfoo_ocr::OcrError::Transport(message)
        }
        SidecarError::Model(message) => docfoo_ocr::OcrError::OcrPage { page: 0, message },
    }
}

/// Sanitize a scan destination into a safe resources-relative folder path.
/// Each `/`-segment is cleaned to safe characters; empty/dot segments are
/// dropped, so `""` means "a new top-level card named after the file".
pub fn sanitize_destination(destination: &str) -> String {
    destination
        .split('/')
        .map(|segment| {
            let cleaned: String = segment
                .trim()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || " ._-()".contains(c) {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            cleaned.trim().to_string()
        })
        .filter(|segment| !segment.is_empty() && segment != "." && segment != "..")
        .collect::<Vec<_>>()
        .join("/")
}

pub struct ScanRequest<'a> {
    pub file: &'a Path,
    pub text_model: &'a str,
    /// `None` disables figure/table analysis.
    pub figure_model: Option<&'a str>,
    pub analysis_prompt: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub concurrency: usize,
    pub destination: &'a str,
    pub pages: Option<Vec<u32>>,
    pub cancel: Arc<AtomicBool>,
}

/// Run one scan into `resources/<destination>/<stem>/`. The caller supplies
/// the progress sink; the result is the crate's [`ImportOutput`].
pub fn run_scan(
    workspace: &Workspace,
    request: ScanRequest,
    sidecar: Arc<SidecarClient>,
    progress: &mut dyn FnMut(Progress),
) -> Result<ImportOutput> {
    let destination = sanitize_destination(request.destination);
    let resources_root = workspace.resources_dir();
    let output_dir = if destination.is_empty() {
        resources_root
    } else {
        resources_root.join(destination.replace('/', std::path::MAIN_SEPARATOR_STR))
    };
    std::fs::create_dir_all(&output_dir)?;

    let client: Arc<dyn CompletionClient> = Arc::new(SidecarCompletionClient::new(sidecar));
    let mut options = ImportOptions::new(client);
    options.ocr_model = request.text_model.to_string();
    if let Some(prompt) = request.prompt {
        if !prompt.trim().is_empty() {
            options.ocr_prompt = prompt.trim().to_string();
        }
    }
    options.analysis = request.figure_model.map(|model| ImageAnalysisOptions {
        model: model.to_string(),
        prompt: request
            .analysis_prompt
            .map(str::to_string)
            .filter(|prompt| !prompt.trim().is_empty())
            .unwrap_or_else(|| docfoo_ocr::ocr::IMAGE_ANALYSIS_PROMPT.to_string()),
    });
    options.concurrency = request.concurrency.clamp(1, 40);
    options.models_dir = workspace.models_dir.clone();
    options.output_dir = output_dir;
    options.pages = request.pages;
    options.cancel = Some(request.cancel);

    docfoo_ocr::run_import(request.file, &options, progress)
        .map_err(|error| CliError::Message(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_cannot_escape() {
        assert_eq!(sanitize_destination(""), "");
        assert_eq!(sanitize_destination("Papers"), "Papers");
        assert_eq!(sanitize_destination("Papers/Sub"), "Papers/Sub");
        assert_eq!(sanitize_destination("../evil"), "evil");
        assert_eq!(sanitize_destination("a/../../b"), "a/b");
        assert_eq!(sanitize_destination("weird:name"), "weird_name");
        assert_eq!(sanitize_destination("  /  "), "");
    }
}
