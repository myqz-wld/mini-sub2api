//! Producer identity is independent of the caller's backend routing headers.
use http::{HeaderMap, HeaderValue};
use serde_json::{Map, Value};

pub(crate) const CLASSIFIER_MODEL: &str = "gpt-5.6-luna";
pub(crate) const BACKEND_REVIEWER_MODEL: &str = "codex-auto-review";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Model,
    Reviewer { backend: bool },
    Classifier,
    Memory,
}

impl Role {
    pub(crate) fn read(object: &Map<String, Value>, headers: &HeaderMap) -> Self {
        Self::resolve(object, headers).unwrap_or(Self::Model)
    }

    pub(crate) fn resolve(object: &Map<String, Value>, headers: &HeaderMap) -> Result<Self, ()> {
        let turn = turn_metadata(object, headers);
        let marker = |name| {
            turn.as_ref()
                .and_then(|turn| turn.get(name))
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "guardian_classifier" | "guardian_review"))
        };
        let source = marker("thread_source");
        let trigger = marker("turn_trigger");
        if source.zip(trigger).is_some_and(|(a, b)| a != b) {
            return Err(());
        }
        let reviewer = Self::Reviewer {
            // Native CodexResponsesHeaders uses exact selected-model equality, even when
            // the caller supplied a backend header. Model overrides keep review semantics.
            backend: object.get("model").and_then(Value::as_str) == Some(BACKEND_REVIEWER_MODEL),
        };
        match headers
            .get("x-codex-guardian")
            .and_then(|v| v.to_str().ok())
        {
            Some("classifier") => return Ok(Self::Classifier),
            Some("reviewer") => return Ok(reviewer),
            _ => {}
        }
        if turn
            .as_ref()
            .is_some_and(|turn| turn["request_kind"] == "memory")
        {
            return Ok(Self::Memory);
        }
        Ok(match source.or(trigger) {
            Some("guardian_classifier") => Self::Classifier,
            Some("guardian_review") => reviewer,
            _ => Self::Model,
        })
    }

    pub(crate) fn project_backend_headers(self, headers: &mut HeaderMap) {
        match self {
            Self::Classifier => {
                headers.insert("x-codex-guardian", HeaderValue::from_static("classifier"));
            }
            Self::Reviewer { backend: true } => {
                headers.insert("x-codex-guardian", HeaderValue::from_static("reviewer"));
            }
            Self::Reviewer { backend: false } => {
                headers.remove("x-codex-guardian");
            }
            _ => {}
        }
    }

    pub(crate) fn is_reviewer(self) -> bool {
        matches!(self, Self::Reviewer { .. })
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Reviewer { .. } => "reviewer",
            Self::Classifier => "classifier",
            Self::Memory => "memory",
        }
    }
}

pub(crate) fn turn_metadata(object: &Map<String, Value>, headers: &HeaderMap) -> Option<Value> {
    let raw = object
        .get("client_metadata")
        .and_then(|metadata| metadata.get("x-codex-turn-metadata"))
        .and_then(Value::as_str)
        .or_else(|| {
            headers
                .get("x-codex-turn-metadata")
                .and_then(|v| v.to_str().ok())
        })?;
    serde_json::from_str(raw).ok()
}
