use crate::ascii_json::to_ascii_json_string;
use chrono::Utc;
use http::HeaderMap;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;

use super::TURN_METADATA_HEADER;
use super::WS_STREAM_START_METADATA;
use super::turn_metadata::bounded_turn_metadata;

const ROOT_TURN_ID: &str = "root_turn_id";
const PARENT_TURN_ID: &str = "parent_turn_id";
const TURN_ID: &str = "turn_id";
const TURN_STARTED_AT: &str = "turn_started_at_unix_ms";

pub(super) fn apply(object: &mut Map<String, Value>, headers: &mut HeaderMap) -> Result<(), ()> {
    let metadata = object
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
        .ok_or(())?;
    let raw = metadata
        .get(TURN_METADATA_HEADER)
        .and_then(Value::as_str)
        .ok_or(())?;
    let mut turn = serde_json::from_str::<Value>(raw).map_err(|_| ())?;
    let turn = turn.as_object_mut().ok_or(())?;
    turn.insert(
        "request_kind".to_string(),
        Value::String("prewarm".to_string()),
    );
    turn.insert(TURN_ID.to_string(), Value::String(String::new()));
    for name in [ROOT_TURN_ID, PARENT_TURN_ID, TURN_STARTED_AT] {
        turn.remove(name);
    }
    let serialized = to_ascii_json_string(&Value::Object(turn.clone())).map_err(|_| ())?;

    metadata.insert(TURN_ID.to_string(), Value::String(String::new()));
    metadata.insert(
        TURN_METADATA_HEADER.to_string(),
        Value::String(serialized.clone()),
    );
    metadata.insert(
        WS_STREAM_START_METADATA.to_string(),
        Value::String(Utc::now().timestamp_millis().to_string()),
    );
    for name in [ROOT_TURN_ID, PARENT_TURN_ID] {
        metadata.remove(name);
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            let Some(item_metadata) = item
                .as_object_mut()
                .and_then(|item| item.get_mut("internal_chat_message_metadata_passthrough"))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            item_metadata.insert(TURN_ID.to_string(), Value::String(String::new()));
        }
    }

    let header = bounded_turn_metadata(&serialized).unwrap_or(serialized);
    headers.insert(
        TURN_METADATA_HEADER,
        HeaderValue::from_str(&header).map_err(|_| ())?,
    );
    Ok(())
}
