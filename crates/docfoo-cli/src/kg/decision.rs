//! System One (Jev) decision bridge — OpenCode Zen / OpenRouter / TypeSafe.
//!
//! Port of `src-tauri/src/kg/decision.rs` with `ureq` instead of `reqwest`.
//! Every failure is returned as a [`DecisionError`] and never aborts a query —
//! the crate's routing stage falls back to lexical seeds and records the
//! reason in the trace.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use docfoo_kg::decision::{DecisionAnswer, DecisionClient, DecisionError, DecisionQuestion};
use docfoo_kg::tunables::QueryTunables;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

struct ProviderProfile {
    env_var: &'static str,
    endpoint: &'static str,
    default_model: &'static str,
}

fn profile(provider: &str) -> ProviderProfile {
    match provider.trim().to_lowercase().as_str() {
        "openrouter" => ProviderProfile {
            env_var: "OPENROUTER_API_KEY",
            endpoint: "https://openrouter.ai/api/v1/systemone",
            default_model: "typesafe/jev-1.13",
        },
        "typesafe" => ProviderProfile {
            env_var: "TYPESAFE_API_KEY",
            endpoint: "https://api.typesafe.ai/v1/systemone",
            default_model: "jev-1.13.0",
        },
        _ => ProviderProfile {
            env_var: "OPENCODE_API_KEY",
            endpoint: "https://opencode.ai/zen/v1/systemone",
            default_model: "jev-1.13-free",
        },
    }
}

/// [`DecisionClient`] backed by a System One provider.
pub struct JevDecisionClient {
    agent: ureq::Agent,
    endpoint: String,
    api_key: String,
    model: String,
}

impl JevDecisionClient {
    /// Build from the query tunables. `None` when routing is disabled or no
    /// key is available (settings override first, then the provider's
    /// environment variable) — the query path then proceeds with lexical
    /// seeds only.
    pub fn from_settings(tunables: &QueryTunables) -> Option<Self> {
        if !tunables.enable_concept_routing {
            return None;
        }
        let profile = profile(&tunables.concept_provider);
        let api_key = if tunables.concept_api_key.trim().is_empty() {
            std::env::var(profile.env_var).unwrap_or_default()
        } else {
            tunables.concept_api_key.trim().to_string()
        };
        if api_key.trim().is_empty() {
            return None;
        }
        let model = if tunables.concept_model.trim().is_empty() {
            profile.default_model.to_string()
        } else {
            tunables.concept_model.trim().to_string()
        };
        let agent = ureq::config::Config::builder()
            .timeout_per_call(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Some(Self {
            agent,
            endpoint: profile.endpoint.to_string(),
            api_key,
            model,
        })
    }
}

impl DecisionClient for JevDecisionClient {
    fn decide(
        &self,
        state: &Value,
        questions: &BTreeMap<String, DecisionQuestion>,
        cancel: &AtomicBool,
    ) -> Result<BTreeMap<String, DecisionAnswer>, DecisionError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(DecisionError::Cancelled);
        }
        let body = json!({
            "model": self.model,
            "state": state,
            "questions": questions,
        });
        let payload =
            serde_json::to_string(&body).map_err(|error| DecisionError::Failed(error.to_string()))?;
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send(payload.as_str())
            .map_err(|error| DecisionError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| DecisionError::Transport(error.to_string()))?;
        if cancel.load(Ordering::Relaxed) {
            return Err(DecisionError::Cancelled);
        }
        if !(200..300).contains(&status) {
            let snippet: String = text.chars().take(300).collect();
            return Err(DecisionError::Failed(format!("HTTP {status}: {snippet}")));
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|error| DecisionError::Failed(format!("unreadable response: {error}")))?;
        let Some(answers) = parsed.get("answers").and_then(Value::as_object) else {
            return Err(DecisionError::Failed(
                "the response carried no answers".to_string(),
            ));
        };
        let mut out = BTreeMap::new();
        for (key, value) in answers {
            if let Ok(answer) = serde_json::from_value::<DecisionAnswer>(value.clone()) {
                out.insert(key.clone(), answer);
            }
        }
        Ok(out)
    }

    fn label(&self) -> String {
        self.model.clone()
    }
}
