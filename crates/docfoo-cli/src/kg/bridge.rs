//! KG ↔ sidecar model bridge.
//!
//! Implements `docfoo_kg::llm::ChatClient` over the Pi sidecar completion
//! service, mirroring `src-tauri/src/kg/bridge.rs` and
//! `src-tauri/src/core/model_bridge.rs`.

use std::sync::Arc;

use serde_json::Value;

use docfoo_kg::llm::{ChatClient, LlmError, Reasoning};

use crate::sidecar::{CompletionRequest, SidecarClient, SidecarError, DEFAULT_TIMEOUT};

/// [`ChatClient`] backed by the pi sidecar.
pub struct AgentChatClient {
    sidecar: Arc<SidecarClient>,
    model_key: String,
    reasoning: Reasoning,
}

impl AgentChatClient {
    pub fn new(sidecar: Arc<SidecarClient>, model_key: &str, reasoning: Reasoning) -> Self {
        Self {
            sidecar,
            model_key: model_key.to_string(),
            reasoning,
        }
    }

    fn request(
        &self,
        messages: &[Value],
        max_tokens: u32,
        response_format: Option<Value>,
        stream: bool,
    ) -> CompletionRequest {
        let mut request = CompletionRequest::new(self.model_key.clone(), messages.to_vec());
        request.max_tokens = Some(max_tokens);
        request.reasoning = Some(self.reasoning.as_str().to_string());
        request.stream = stream;
        request.response_format = response_format;
        request
    }
}

impl ChatClient for AgentChatClient {
    fn chat(
        &self,
        messages: &[Value],
        max_tokens: u32,
        response_format: Option<Value>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<String, LlmError> {
        let request = self.request(messages, max_tokens, response_format, false);
        self.sidecar
            .complete(request, Some(cancel), None, DEFAULT_TIMEOUT)
            .map_err(bridge_failure)
    }

    fn chat_stream(
        &self,
        messages: &[Value],
        max_tokens: u32,
        on_delta: &mut dyn FnMut(&str),
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<String, LlmError> {
        let request = self.request(messages, max_tokens, None, true);
        self.sidecar
            .complete(request, Some(cancel), Some(on_delta), DEFAULT_TIMEOUT)
            .map_err(bridge_failure)
    }

    fn label(&self) -> String {
        self.model_key.clone()
    }
}

fn bridge_failure(failure: SidecarError) -> LlmError {
    match failure {
        SidecarError::Cancelled => LlmError::Cancelled,
        // Keep the KG crate's EmptyResponse semantics (warm-up still succeeds)
        // but carry the provider detail so the caller can explain it.
        SidecarError::Model(message) => {
            if message.to_ascii_lowercase().contains("empty response") {
                LlmError::EmptyResponse(message)
            } else {
                LlmError::Failed(message)
            }
        }
        SidecarError::Transport(message) | SidecarError::Spawn(message) => {
            LlmError::Transport(message)
        }
    }
}
