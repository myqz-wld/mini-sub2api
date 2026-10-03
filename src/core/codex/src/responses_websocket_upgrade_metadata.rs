//! Connection-only native metadata and its projection onto admitted response streams.
use http::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

pub(crate) const REASONING_HEADER: &str = "x-reasoning-included";

#[derive(Clone, Default)]
pub(crate) struct UpgradeMetadata {
    pub(crate) reasoning: Option<HeaderValue>,
    pub(crate) model: Option<String>,
}

impl UpgradeMetadata {
    pub(crate) fn read(headers: &HeaderMap) -> Self {
        Self {
            reasoning: headers.get(REASONING_HEADER).cloned(),
            model: headers
                .get("openai-model")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        }
    }

    pub(crate) fn matches_reasoning(&self, advertised: bool) -> bool {
        // The pinned client tests presence, including an empty or literal "false" value.
        self.reasoning.is_some() == advertised
    }
}

#[derive(Default)]
pub(crate) struct ResponseModel {
    model: Option<String>,
    generation: Option<u64>,
    notice: Option<String>,
}

impl ResponseModel {
    pub(crate) fn new(model: Option<String>) -> Self {
        Self {
            model,
            generation: None,
            notice: None,
        }
    }

    pub(crate) fn project(&mut self, text: String, generation: u64) -> anyhow::Result<String> {
        let Some(model) = &self.model else {
            return Ok(text);
        };
        if self.generation == Some(generation) {
            return Ok(text);
        }
        let mut event: Value = serde_json::from_str(&text)?;
        // Native consumes rate-limit events before looking for model headers.
        if event.get("type").and_then(Value::as_str) == Some("codex.rate_limits") {
            return Ok(text);
        }
        let event_model = [
            event
                .get("response")
                .and_then(|response| response.get("headers")),
            event.get("headers"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find_map(|headers| {
            headers.iter().find_map(|(name, value)| {
                if name.eq_ignore_ascii_case("openai-model")
                    || name.eq_ignore_ascii_case("x-openai-model")
                {
                    native_header_string(value)
                } else {
                    None
                }
            })
        });
        self.generation = Some(generation);
        if event.get("type").and_then(Value::as_str) == Some("error") {
            // Native maps wrapped errors before inspecting their headers. Emit only after
            // this real upstream event arrives, so delivery/TTFB never precedes inference.
            let notice = json!({"type":"response.metadata", "headers":{
                "openai-model":event_model.unwrap_or(model)
            }})
            .to_string();
            anyhow::ensure!(
                notice.len() <= crate::inference_limits::get().output_bytes,
                "response metadata exceeds limit"
            );
            self.notice = Some(notice);
            return Ok(text);
        }
        if event_model.is_some() {
            return Ok(text);
        }
        let object = event
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid response event"))?;
        let headers = object.entry("headers").or_insert_with(|| json!({}));
        let headers = headers
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid response headers"))?;
        headers.insert("openai-model".into(), model.clone().into());
        let encoded = serde_json::to_string(&event)?;
        anyhow::ensure!(
            encoded.len() <= crate::inference_limits::get().output_bytes,
            "response metadata exceeds limit"
        );
        Ok(encoded)
    }

    pub(crate) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
}

fn native_header_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        Value::Array(values) => values.first().and_then(native_header_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_contract_uses_presence_not_boolean_value() {
        let absent = UpgradeMetadata::read(&HeaderMap::new());
        assert!(absent.matches_reasoning(false));
        assert!(!absent.matches_reasoning(true));
        for value in ["", "false", "true"] {
            let mut headers = HeaderMap::new();
            headers.insert(REASONING_HEADER, value.parse().unwrap());
            let present = UpgradeMetadata::read(&headers);
            assert!(present.matches_reasoning(true));
            assert!(!present.matches_reasoning(false));
            assert_eq!(present.reasoning.unwrap(), value);
        }
    }

    #[test]
    fn model_projects_per_operation_and_preserves_newer_event_metadata() {
        let mut model = ResponseModel::new(Some("synthetic-handshake-model".into()));
        let original = r#"{"type":"response.created","response":{"id":"resp_synthetic"}}"#;
        let first: Value =
            serde_json::from_str(&model.project(original.into(), 1).unwrap()).unwrap();
        assert_eq!(
            first["headers"]["openai-model"],
            "synthetic-handshake-model"
        );
        assert_eq!(model.project(original.into(), 1).unwrap(), original);
        for (generation, event) in [
            (
                2,
                json!({"type":"response.created","headers":{"OpenAI-Model":"actual-model"}}),
            ),
            (
                3,
                json!({"type":"response.created","response":{"headers":{"openai-model":"actual-model"}}}),
            ),
        ] {
            let text = event.to_string();
            assert_eq!(model.project(text.clone(), generation).unwrap(), text);
        }
        let next: Value =
            serde_json::from_str(&model.project(original.into(), 4).unwrap()).unwrap();
        assert_eq!(next["headers"]["openai-model"], "synthetic-handshake-model");
        assert_eq!(
            ResponseModel::default()
                .project(original.into(), 1)
                .unwrap(),
            original
        );
    }

    #[test]
    fn native_model_aliases_and_array_headers_are_preserved() {
        for name in ["OpenAI-Model", "x-openai-model"] {
            for value in [
                json!("event-model"),
                json!(["event-model"]),
                json!([["event-model"]]),
            ] {
                for nested in [false, true] {
                    let mut event = json!({"type":"response.created", "response":{}});
                    let container = if nested {
                        &mut event["response"]
                    } else {
                        &mut event
                    };
                    container["headers"] = json!({name:value});
                    let text = event.to_string();
                    let mut model = ResponseModel::new(Some("handshake-model".into()));
                    assert_eq!(model.project(text.clone(), 1).unwrap(), text);
                }
            }
        }
    }

    #[test]
    fn native_short_circuits_cannot_swallow_handshake_model() {
        let mut model = ResponseModel::new(Some("handshake-model".into()));
        let limits = json!({"type":"codex.rate_limits"}).to_string();
        assert_eq!(model.project(limits.clone(), 1).unwrap(), limits);
        assert!(model.take_notice().is_none());
        let error = json!({"type":"error", "status":429, "error":{"code":"flex_unavailable"},
            "headers":{"x-openai-model":["event-model"]}})
        .to_string();
        assert_eq!(model.project(error.clone(), 1).unwrap(), error);
        let notice: Value = serde_json::from_str(&model.take_notice().unwrap()).unwrap();
        assert_eq!(notice["type"], "response.metadata");
        assert_eq!(notice["headers"]["openai-model"], "event-model");
        assert_eq!(model.project(error.clone(), 1).unwrap(), error);
        assert!(model.take_notice().is_none());
    }
}
