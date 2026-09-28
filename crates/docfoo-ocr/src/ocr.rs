//! Text OCR and Figures & Tables analysis over an OpenAI-compatible endpoint
//! (the cloud equivalent of pdfium_probe's `chat_completion`: image-first
//! message ordering, temperature 0.0, retries with backoff).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;

use crate::error::{OcrError, Result};

/// OpenAI-compatible endpoint for the paid (Go) tier.
pub const ENDPOINT: &str = "https://opencode.ai/zen/go/v1/chat/completions";

/// The OCR model id at that endpoint.
pub const MODEL: &str = "mimo-v2.5";

/// Max attempts per region. Retry transient provider failures, but do not
/// retry malformed requests or other non-retryable client errors.
const RETRIES: u32 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-region OCR prompt (pdfium_probe's "Text Recognition" translated
/// for the cloud model): a region crop contains one kind of content, so
/// the model just transcribes it.
pub const REGION_PROMPT: &str = "Transcribe the text content of this image exactly as printed. \
If it contains a mathematical formula, output it in LaTeX. \
Output ONLY the transcription, with no preamble, commentary, or markdown code fences.";

/// Default prompt for optional Figures & Tables image analysis. The result is
/// used directly as Markdown image alt metadata, so it stays plain text and
/// makes no claims that are not supported by the visible asset.
pub const IMAGE_ANALYSIS_PROMPT: &str = "Describe all visible content in this image accurately and in detail. For figures, charts, and other visual assets, include the asset type, axes, legends, labels, values and trends, spatial relationships, and notable details. For tables, include headers, row and column structure, and all visible cell values. Do not add a preamble. Do not infer unsupported facts. Output plain text only, suitable directly as Markdown image alt metadata.";

/// One completion choice.
#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

/// The assistant message.
#[derive(Debug, Deserialize)]
struct Message {
    content: serde_json::Value,
}

/// A chat-completion response body.
///
/// Most OpenAI-compatible endpoints return `{ "choices": [...] }`, but the
/// api.cline.bot gateway wraps that in a `{ "data": { ... }, "success": true }`
/// envelope. Accept both: `#[serde(flatten)]` with a fallback handles the
/// wrapper transparently.
#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(flatten)]
    payload: ChoicePayload,
}

/// The actual `{ "choices": [...] }` object — either at the top level or
/// nested under a `data` key (api.cline.bot).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChoicePayload {
    Flat { choices: Vec<Choice> },
    Enveloped { data: InnerEnvelope },
}

#[derive(Debug, Deserialize)]
struct InnerEnvelope {
    choices: Vec<Choice>,
}

/// OpenAI-style error envelope.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

/// Build the shared HTTP agent for OCR requests. Clones of the returned
/// agent share one connection pool, so region requests reuse warm TLS
/// connections instead of paying a fresh TCP+TLS handshake per crop.
#[must_use]
pub fn ocr_agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .timeout_per_call(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// OpenAI-compatible chat-completion request for one region crop,
/// targeting a caller-selected endpoint (provider) and model. Uses a
/// throwaway connection; prefer [`ocr_region_with`] with a shared agent.
///
/// # Errors
///
/// Returns [`OcrError::Cancelled`] when `cancel` is set; other variants
/// describe transport, auth, or malformed-response failures.
pub fn ocr_region(
    png_bytes: &[u8],
    api_key: &str,
    endpoint: &str,
    model: &str,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    ocr_region_with(&ocr_agent(), png_bytes, api_key, endpoint, model, cancel)
}

/// [`ocr_region`] over a caller-supplied agent — pass clones of one
/// shared [`ocr_agent`] so concurrent workers pool their connections.
///
/// # Errors
///
/// Same contract as [`ocr_region`].
pub fn ocr_region_with(
    agent: &ureq::Agent,
    png_bytes: &[u8],
    api_key: &str,
    endpoint: &str,
    model: &str,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    ocr_region_with_prompt(agent, png_bytes, api_key, endpoint, model, REGION_PROMPT, &[], cancel)
}

/// [`ocr_region_with`] with a caller-supplied prompt. `image_bytes` are the
/// request image, either a PNG page tile or a JPEG/PNG asset crop.
/// `headers` carry any provider-required request headers resolved by pi
/// (for example Cloudflare's gateway auth); they are applied last, so they
/// can override the default `Authorization`/`Content-Type` values.
pub fn ocr_region_with_prompt(
    agent: &ureq::Agent,
    image_bytes: &[u8],
    api_key: &str,
    endpoint: &str,
    model: &str,
    prompt: &str,
    headers: &[(String, String)],
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    if is_cancelled(cancel) {
        return Err(OcrError::Cancelled);
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
    let mime = if image_bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
    let body = serde_json::json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": 4096,
        "thinking": { "type": "disabled" },
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{b64}") } },
                { "type": "text", "text": prompt },
            ]
        }],
    });

    let mut last_err = String::new();
    let mut transport = false;
    for attempt in 0..RETRIES {
        if is_cancelled(cancel) {
            return Err(OcrError::Cancelled);
        }
        let mut request = agent
            .post(endpoint)
            .header("Content-Type", "application/json");
        if !api_key.is_empty() {
            request = request.header("Authorization", &format!("Bearer {api_key}"));
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        match request.send(body.to_string())
        {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                let text = match resp.body_mut().read_to_string() {
                    Ok(t) => t,
                    Err(e) => {
                        last_err = format!("response read failed: {e}");
                        transport = true;
                        if sleep_backoff(attempt, cancel) {
                            return Err(OcrError::Cancelled);
                        }
                        continue;
                    }
                };
                if status != 200 {
                    // auth errors never heal by waiting — fail fast
                    if status == 401 || status == 403 {
                        return Err(OcrError::OcrPage {
                            page: 0,
                            message: format!("the API key was rejected ({status})"),
                        });
                    }
                    let msg = serde_json::from_str::<ErrorEnvelope>(&text)
                        .ok()
                        .and_then(|e| e.error)
                        .and_then(|e| e.message)
                        .unwrap_or_else(|| text.chars().take(160).collect());
                    last_err = format!("server error ({status}): {msg}");
                    transport = false;
                    // A bad request will never become valid by retrying. Keep
                    // the UI from appearing hung for the full backoff cycle.
                    if (400..500).contains(&status)
                        && !matches!(status, 408 | 409 | 425 | 429)
                    {
                        return Err(OcrError::OcrPage {
                            page: 0,
                            message: last_err,
                        });
                    }
                    if sleep_backoff(attempt, cancel) {
                        return Err(OcrError::Cancelled);
                    }
                    continue;
                }
                match serde_json::from_str::<ChatResponse>(&text) {
                    Ok(resp) => {
                        let content = match resp.payload {
                            ChoicePayload::Flat { choices } => choices,
                            ChoicePayload::Enveloped { data } => data.choices,
                        }
                        .into_iter()
                        .next()
                        .map(|c| c.message.content)
                        .unwrap_or(serde_json::Value::Null);
                        let text = match content {
                            serde_json::Value::String(s) => s,
                            serde_json::Value::Array(parts) => parts
                                .iter()
                                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                                .collect::<String>(),
                            _ => String::new(),
                        };
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            last_err = "empty response".to_string();
                            transport = false;
                            if sleep_backoff(attempt, cancel) {
                                return Err(OcrError::Cancelled);
                            }
                            continue;
                        }
                        return Ok(text);
                    }
                    Err(e) => {
                        last_err = format!("bad response JSON: {e}");
                        transport = false;
                        if sleep_backoff(attempt, cancel) {
                            return Err(OcrError::Cancelled);
                        }
                    }
                }
            }
            Err(e) => {
                last_err = format!("request failed: {e}");
                transport = true;
                if sleep_backoff(attempt, cancel) {
                    return Err(OcrError::Cancelled);
                }
            }
        }
    }
    if transport {
        Err(OcrError::Transport(last_err))
    } else {
        Err(OcrError::OcrPage {
            page: 0,
            message: last_err,
        })
    }
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn sleep_backoff(attempt: u32, cancel: Option<&AtomicBool>) -> bool {
    let wait = 2u64.saturating_mul(1u64 << attempt.min(4)); // 2, 4, 8, 16, 32 s
    for _ in 0..wait.saturating_mul(10) {
        if is_cancelled(cancel) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract the assistant text from a raw response body (top-level
    /// `choices` or api.cline.bot's `data`-wrapped envelope).
    fn content_of(body: &str) -> String {
        let resp: ChatResponse = serde_json::from_str(body).expect("parses");
        let content = match resp.payload {
            ChoicePayload::Flat { choices } => choices,
            ChoicePayload::Enveloped { data } => data.choices,
        }
        .into_iter()
        .next()
        .unwrap()
        .message
        .content;
        match content {
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<String>(),
            _ => String::new(),
        }
    }

    #[test]
    fn parses_flat_openai_envelope() {
        // opencode.ai token/zen returns the standard flat shape.
        let body = r#"{"choices":[{"message":{"content":"hello world"}}]}"#;
        assert_eq!(content_of(body), "hello world");
    }

    #[test]
    fn parses_cline_enveloped_response() {
        // api.cline.bot wraps the same object under `data`.
        let body = r#"{"data":{"choices":[{"message":{"content":"hello cline"}}]},"success":true}"#;
        assert_eq!(content_of(body), "hello cline");
    }
}
