//! LLM gateway — the crate's transport seam.
//!
//! The KG crate never talks to a provider itself: every call goes through the
//! [`ChatClient`] trait. The app supplies a client backed by pi's runtime (the
//! agent bridge); tests use a tiny fake. pi owns transport, auth, provider
//! headers, retries, reasoning policy and sampling, so this module only defines
//! the contract, the error shape and the JSON repair chain the callers rely on.

use serde_json::Value;

/// Whether a call may use chain-of-thought. Indexing defaults to [`Self::Off`];
/// the per-run thinking toggle sends an explicit pi level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reasoning {
    /// Suppress thinking (pi thinking level "off").
    Off,
    /// Let the model's own default apply.
    Default,
    /// An explicit pi thinking level ("minimal" … "max").
    Level(String),
}

impl Reasoning {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Off => "off",
            Self::Default => "default",
            Self::Level(level) => level,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("empty response from the model: {0}")]
    EmptyResponse(String),
    #[error("the call was cancelled")]
    Cancelled,
    /// A network/timeout failure talking to the model bridge.
    #[error("{0}")]
    Transport(String),
    /// The provider/model returned an error (already user-friendly).
    #[error("{0}")]
    Failed(String),
}

/// One chat gateway. Implementations own transport, auth, provider headers and
/// sampling policy: the KG crate only sends messages, a token budget and an
/// optional response format, so the model's own defaults apply.
pub trait ChatClient: Send + Sync {
    /// One non-streaming completion. `response_format` is the raw JSON-schema
    /// payload (honored by OpenAI-compatible clients, ignored elsewhere).
    fn chat(
        &self,
        messages: &[Value],
        max_tokens: u32,
        response_format: Option<Value>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<String, LlmError>;

    /// Streaming completion. Every content delta is passed to `on_delta` as it
    /// arrives and the full text is returned. Once the stream starts, failures
    /// are terminal (partial output has already been delivered to the caller).
    /// Reasoning deltas are swallowed so the UI only ever shows the answer.
    fn chat_stream(
        &self,
        messages: &[Value],
        max_tokens: u32,
        on_delta: &mut dyn FnMut(&str),
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<String, LlmError>;

    /// Human-readable model label for progress messages ("" = unknown).
    fn label(&self) -> String {
        String::new()
    }
}

/// Best-effort JSON recovery from an LLM reply — the exact chain from
/// llm.py `extract_json`: raw parse → fenced block → outermost braces →
/// trailing-comma repair.
pub fn extract_json(text: &str) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    let stripped = text.trim();
    if let Some(inner) = fenced_block(stripped) {
        if let Ok(v) = serde_json::from_str::<Value>(&inner) {
            return Some(v);
        }
    }
    let start = stripped.find('{');
    let end = stripped.rfind('}');
    let candidate = match (start, end) {
        (Some(s), Some(e)) if e > s => &stripped[s..=e],
        _ => return None,
    };
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        return Some(v);
    }
    let repaired = strip_trailing_commas(candidate);
    serde_json::from_str::<Value>(&repaired).ok()
}

fn fenced_block(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?s)```(?:json)?\s*(.*?)```").ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn strip_trailing_commas(text: &str) -> String {
    let re = regex::Regex::new(r",(\s*[}\]])").expect("static regex");
    re.replace_all(text, "$1").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_json_handles_all_repair_shapes() {
        assert_eq!(extract_json("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(
            extract_json("Sure!\n```json\n{\"a\": 2}\n```\ndone"),
            Some(json!({"a": 2}))
        );
        assert_eq!(
            extract_json("prefix {\"a\": [1,2,],} suffix"),
            Some(json!({"a": [1, 2]}))
        );
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json(""), None);
    }

    #[test]
    fn reasoning_maps_to_wire_strings() {
        assert_eq!(Reasoning::Off.as_str(), "off");
        assert_eq!(Reasoning::Default.as_str(), "default");
        assert_eq!(Reasoning::Level("high".to_string()).as_str(), "high");
    }
}
