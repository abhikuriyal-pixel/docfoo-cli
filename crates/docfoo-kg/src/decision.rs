//! System One decision seam — the crate's typed-classifier contract.
//!
//! The KG crate never talks to TypeSafe/Jev itself: every decision call goes
//! through the [`DecisionClient`] trait, mirroring how
//! [`crate::llm::ChatClient`] keeps text generation behind a transport-free
//! seam. The app supplies a client backed by the System One API (tests use a
//! fake), and every call site treats a decision failure as non-fatal —
//! retrieval falls back to its lexical path unchanged.
//!
//! Wire shape (TypeSafe System One): a state plus a map of named questions.
//! Questions are one of three primitives — `Choice` (pick one of a named
//! option set), `Score` (position on an ordered rubric), `Noul` (calibrated
//! yes/no probability) — and every question in one request is evaluated in
//! parallel against the same state.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

/// What a "yes" and a "no" mean for a Noul question.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<String>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<String>,
}

/// One typed question in a decision request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DecisionQuestion {
    /// Pick one option out of a defined set (at most 255).
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Option<String>>,
    },
    /// A position on an ordered rubric (2..=10 levels).
    Score {
        instructions: Value,
        criteria: Vec<String>,
    },
    /// A yes/no judgment, answered with the probability of "yes".
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

/// The model's answer to one [`DecisionQuestion`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DecisionAnswer {
    Choice {
        choice: String,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: f64,
    },
    Score {
        score: f64,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
}

impl DecisionAnswer {
    /// The calibrated probability this answer carries (Noul: the probability
    /// itself; Choice/Score: the model's confidence in the answer).
    pub fn probability(&self) -> f64 {
        match self {
            Self::Noul { noul } => *noul,
            Self::Choice { confidence, .. } | Self::Score { confidence, .. } => *confidence,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("the decision call was cancelled")]
    Cancelled,
    /// A network/timeout failure talking to the decision API.
    #[error("{0}")]
    Transport(String),
    /// The endpoint returned an error or an unreadable answer.
    #[error("{0}")]
    Failed(String),
}

/// One typed-classifier gateway. Implementations own transport, auth and the
/// model id; the crate only sends a state and named questions.
pub trait DecisionClient: Send + Sync {
    /// Evaluate every question against `state` in one request.
    fn decide(
        &self,
        state: &Value,
        questions: &BTreeMap<String, DecisionQuestion>,
        cancel: &AtomicBool,
    ) -> Result<BTreeMap<String, DecisionAnswer>, DecisionError>;

    /// Human-readable model label for the trace ("" = unknown).
    fn label(&self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn noul_serializes_with_true_false_criteria_keys() {
        let q = DecisionQuestion::Noul {
            instructions: json!("Is this about cache memory?"),
            criteria: Some(NoulCriteria {
                yes: Some("the query is about this concept".into()),
                no: Some("unrelated to this concept".into()),
            }),
        };
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "type": "noul",
                "instructions": "Is this about cache memory?",
                "criteria": {
                    "true": "the query is about this concept",
                    "false": "unrelated to this concept",
                }
            })
        );
    }

    #[test]
    fn noul_without_criteria_omits_the_field() {
        let q = DecisionQuestion::Noul {
            instructions: json!("Urgent?"),
            criteria: None,
        };
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type": "noul", "instructions": "Urgent?"})
        );
    }

    #[test]
    fn choice_criteria_keep_null_for_undescribed_options() {
        let mut criteria = BTreeMap::new();
        criteria.insert("billing".to_string(), Some("money problems".to_string()));
        criteria.insert("bug".to_string(), None);
        let q = DecisionQuestion::Choice {
            instructions: json!("Which team?"),
            criteria,
        };
        let value = serde_json::to_value(&q).unwrap();
        assert_eq!(value["type"], "choice");
        assert_eq!(value["criteria"]["billing"], "money problems");
        assert_eq!(value["criteria"]["bug"], Value::Null);
    }

    #[test]
    fn answers_parse_from_the_documented_shapes() {
        let answers: BTreeMap<String, DecisionAnswer> = serde_json::from_value(json!({
            "tone": {"type": "choice", "choice": "billing",
                     "probabilities": {"billing": 0.94, "bug": 0.06}, "confidence": 0.91},
            "urgency": {"type": "score", "score": 1.8,
                        "probabilities": {"0": 0.02, "1": 0.16, "2": 0.82}, "confidence": 0.88},
            "refund": {"type": "noul", "noul": 0.98},
        }))
        .unwrap();
        assert_eq!(answers["refund"].probability(), 0.98);
        assert!((answers["tone"].probability() - 0.91).abs() < 1e-9);
        match &answers["urgency"] {
            DecisionAnswer::Score { score, .. } => assert!((score - 1.8).abs() < 1e-9),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
