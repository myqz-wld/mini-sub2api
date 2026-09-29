//! Body-only caller provenance; it never selects a session or creates a turn.
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub(crate) const KEY: &str = "mcp_attribution";
const MAXIMUM: usize = 16 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    None,
    Complete,
    AttributionError,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_id: Option<String>,
    server_name: String,
    tool_name: String,
    first_turn_id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attribution {
    status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_reason: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<Source>,
}

fn failure(reason: &str) -> Attribution {
    Attribution {
        status: Status::AttributionError,
        error_reason: Some(reason.into()),
        sources: Vec::new(),
    }
}

pub(crate) fn read(object: &Map<String, Value>) -> Option<Attribution> {
    let raw = object.get("client_metadata")?.get(KEY)?;
    let Some(mut value) = raw
        .as_str()
        .and_then(|raw| serde_json::from_str::<Attribution>(raw).ok())
    else {
        return Some(failure("source_invalid"));
    };
    value.error_reason = value
        .error_reason
        .and_then(|v| v.as_str().map(str::to_owned))
        .map(|reason| match reason.as_str() {
            "history_missing_checkpoint"
            | "checkpoint_invalid"
            | "checkpoint_source_conflict"
            | "source_invalid"
            | "recorder_poisoned"
            | "restored_error_unknown"
            | "payload_too_large"
            | "serialization_failed"
            | "unknown" => reason.into(),
            _ => "unknown".into(),
        });
    if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAXIMUM) {
        value = failure("payload_too_large");
    }
    Some(value)
}

pub(crate) fn project(
    editor: &mut RequestStateEditor<'_>,
    attribution: Option<&Attribution>,
    metadata: &mut Map<String, Value>,
) {
    let Some(attribution) = attribution else {
        return;
    };
    let mut value = attribution.clone();
    for source in &mut value.sources {
        let Ok(Some(turn)) =
            editor.existing_wire_from_downstream(WireIdDomain::Turn, &source.first_turn_id)
        else {
            // Optional provenance cannot establish ownership or leak an unmapped caller ID.
            value = failure("source_invalid");
            break;
        };
        source.first_turn_id = turn;
    }
    let mut encoded = serde_json::to_string(&value).unwrap_or_else(|_| {
        r#"{"status":"attribution_error","error_reason":"serialization_failed"}"#.into()
    });
    if encoded.len() > MAXIMUM {
        encoded = r#"{"status":"attribution_error","error_reason":"payload_too_large"}"#.into();
    }
    metadata.insert(KEY.into(), encoded.into());
}

#[cfg(test)]
#[path = "request_mcp_attribution_tests.rs"]
mod tests;
