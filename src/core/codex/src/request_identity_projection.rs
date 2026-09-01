use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;

use crate::ascii_json::to_ascii_json_string;
use crate::request_identity::turn_metadata::bounded_turn_metadata;

const INSTALLATION_HEADER: &str = "x-codex-installation-id";
const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const WINDOW_HEADER: &str = "x-codex-window-id";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRequestIdentity {
    pub(crate) installation_id: String,
    pub(crate) session_id: String,
    pub(crate) thread_id: String,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) forked_from_thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) root_turn_id: Option<String>,
    pub(crate) parent_turn_id: Option<String>,
    pub(crate) window_number: u64,
    pub(crate) request_kind: String,
    pub(crate) turn_started_at_unix_ms: Option<i64>,
}

impl ResolvedRequestIdentity {
    pub(crate) fn window_id(&self) -> String {
        format!("{}:{}", self.thread_id, self.window_number)
    }

    fn prewarm(&self) -> bool {
        self.request_kind == "prewarm"
    }

    fn memory(&self) -> bool {
        self.request_kind == "memory"
    }
}

pub(crate) fn apply(
    headers: &mut HeaderMap,
    object: &mut Map<String, Value>,
    identity: &ResolvedRequestIdentity,
) -> Result<(), ()> {
    object.insert(
        "prompt_cache_key".to_string(),
        Value::String(identity.session_id.clone()),
    );
    insert_header(headers, "session-id", &identity.session_id)?;
    insert_header(headers, "thread-id", &identity.thread_id)?;
    insert_header(headers, "x-client-request-id", &identity.thread_id)?;
    insert_header(headers, WINDOW_HEADER, &identity.window_id())?;
    match &identity.parent_thread_id {
        Some(parent) => insert_header(headers, "x-codex-parent-thread-id", parent)?,
        None => {
            headers.remove("x-codex-parent-thread-id");
        }
    }

    let metadata = object
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let metadata = metadata.as_object_mut().ok_or(())?;
    let existing_turn = metadata
        .get(TURN_METADATA_HEADER)
        .and_then(Value::as_str)
        .and_then(parse_object)
        .or_else(|| {
            headers
                .get(TURN_METADATA_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_object)
        })
        .unwrap_or_default();
    let turn = canonical_turn_metadata(existing_turn, identity);
    let serialized = to_ascii_json_string(&Value::Object(turn)).map_err(|_| ())?;

    put(metadata, "session_id", &identity.session_id);
    put(metadata, "thread_id", &identity.thread_id);
    put(metadata, INSTALLATION_HEADER, &identity.installation_id);
    put(metadata, WINDOW_HEADER, &identity.window_id());
    set_optional(
        metadata,
        "parent_thread_id",
        identity.parent_thread_id.as_deref(),
    );
    set_optional(
        metadata,
        "forked_from_thread_id",
        identity.forked_from_thread_id.as_deref(),
    );
    if identity.memory() {
        for name in ["turn_id", "root_turn_id", "parent_turn_id"] {
            metadata.remove(name);
        }
    } else if identity.prewarm() {
        put(metadata, "turn_id", "");
        metadata.remove("root_turn_id");
        metadata.remove("parent_turn_id");
    } else {
        set_optional(metadata, "turn_id", identity.turn_id.as_deref());
        set_optional(metadata, "root_turn_id", identity.root_turn_id.as_deref());
        set_optional(
            metadata,
            "parent_turn_id",
            identity.parent_turn_id.as_deref(),
        );
    }
    metadata.insert(
        TURN_METADATA_HEADER.to_string(),
        Value::String(serialized.clone()),
    );

    let header_turn = bounded_turn_metadata(&serialized).unwrap_or(serialized);
    insert_header(headers, TURN_METADATA_HEADER, &header_turn)?;
    Ok(())
}

fn canonical_turn_metadata(
    mut turn: Map<String, Value>,
    identity: &ResolvedRequestIdentity,
) -> Map<String, Value> {
    if !identity.memory() {
        crate::sandbox_projection::normalize(&mut turn);
    }
    put(&mut turn, "installation_id", &identity.installation_id);
    put(&mut turn, "session_id", &identity.session_id);
    put(&mut turn, "thread_id", &identity.thread_id);
    put(&mut turn, "window_id", &identity.window_id());
    put(&mut turn, "request_kind", &identity.request_kind);
    set_optional(
        &mut turn,
        "parent_thread_id",
        identity.parent_thread_id.as_deref(),
    );
    set_optional(
        &mut turn,
        "forked_from_thread_id",
        identity.forked_from_thread_id.as_deref(),
    );
    if identity.memory() {
        for name in [
            "turn_id",
            "root_turn_id",
            "parent_turn_id",
            "turn_started_at_unix_ms",
        ] {
            turn.remove(name);
        }
    } else if identity.prewarm() {
        put(&mut turn, "turn_id", "");
        for name in ["root_turn_id", "parent_turn_id", "turn_started_at_unix_ms"] {
            turn.remove(name);
        }
    } else {
        set_optional(&mut turn, "turn_id", identity.turn_id.as_deref());
        set_optional(&mut turn, "root_turn_id", identity.root_turn_id.as_deref());
        set_optional(
            &mut turn,
            "parent_turn_id",
            identity.parent_turn_id.as_deref(),
        );
        match identity.turn_started_at_unix_ms {
            Some(started) => {
                turn.insert("turn_started_at_unix_ms".to_string(), started.into());
            }
            None => {
                turn.remove("turn_started_at_unix_ms");
            }
        }
    }
    turn
}

fn parse_object(raw: &str) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(raw)
        .ok()?
        .as_object()
        .cloned()
}

fn put(object: &mut Map<String, Value>, name: &str, value: &str) {
    object.insert(name.to_string(), Value::String(value.to_string()));
}

fn set_optional(object: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    match value {
        Some(value) => put(object, name, value),
        None => {
            object.remove(name);
        }
    }
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), ()> {
    let value = HeaderValue::from_str(value).map_err(|_| ())?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_one_identity_into_body_header_and_nested_metadata() {
        let mut headers = HeaderMap::new();
        let mut body = serde_json::json!({
            "prompt_cache_key": "conflict",
            "client_metadata": {
                "session_id": "conflict",
                "x-codex-turn-metadata": "{\"workspaces\":{\"/tmp\":{}},\"session_id\":\"conflict\"}"
            }
        })
        .as_object()
        .expect("body")
        .clone();
        let identity = ResolvedRequestIdentity {
            installation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            session_id: "01900000-0000-7000-8000-000000000001".to_string(),
            thread_id: "01900000-0000-7000-8000-000000000001".to_string(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            turn_id: Some("01900000-0000-7000-8000-000000000002".to_string()),
            root_turn_id: Some("01900000-0000-7000-8000-000000000002".to_string()),
            parent_turn_id: None,
            window_number: 3,
            request_kind: "turn".to_string(),
            turn_started_at_unix_ms: Some(1_700_000_000_000),
        };
        apply(&mut headers, &mut body, &identity).expect("projection");
        assert_eq!(body["prompt_cache_key"], identity.session_id);
        assert_eq!(body["client_metadata"]["thread_id"], identity.thread_id);
        let nested: Value = serde_json::from_str(
            body["client_metadata"][TURN_METADATA_HEADER]
                .as_str()
                .expect("nested"),
        )
        .expect("nested JSON");
        assert_eq!(nested["session_id"], identity.session_id);
        assert_eq!(nested["thread_id"], identity.thread_id);
        assert_eq!(nested["window_id"], identity.window_id());
        assert!(nested["workspaces"].get("/tmp").is_some());
        let header: Value = serde_json::from_str(
            headers[TURN_METADATA_HEADER]
                .to_str()
                .expect("header metadata"),
        )
        .expect("header JSON");
        assert_eq!(header["turn_id"], identity.turn_id.unwrap());
    }
}
