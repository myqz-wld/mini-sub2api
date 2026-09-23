//! Optional v0.156.0 lifecycle metadata. These fields never locate a session.
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use http::HeaderMap;
use serde_json::{Map, Value};

#[derive(Default)]
pub(crate) struct NativeMetadata {
    pub(crate) window_number: Option<u64>,
    pub(crate) context_window_id: Option<String>,
    fork_ordinal: Option<u64>,
    turn_trigger: Option<String>,
    history_ingest: Option<bool>,
    cache_key: Option<String>,
    cache_header: bool,
    parent_response: Option<String>,
}

impl NativeMetadata {
    pub(crate) fn read(object: &Map<String, Value>, headers: &HeaderMap) -> anyhow::Result<Self> {
        let raw = object
            .get("client_metadata")
            .and_then(Value::as_object)
            .and_then(|m| m.get("x-codex-turn-metadata"))
            .and_then(Value::as_str)
            .or_else(|| {
                headers
                    .get("x-codex-turn-metadata")
                    .and_then(|v| v.to_str().ok())
            });
        let values: Map<String, Value> = raw
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        if let Some(value) = values.get("analytics_enabled") {
            anyhow::ensure!(
                value.is_boolean() || value.is_null(),
                "invalid analytics metadata"
            );
        }
        let number = |name| -> anyhow::Result<Option<u64>> {
            match values.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => value
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| anyhow::anyhow!("invalid native metadata number")),
            }
        };
        let text = |name| -> anyhow::Result<Option<String>> {
            match values.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) => {
                    crate::request_state_types::validate_wire_id(value)?;
                    Ok(Some(value.clone()))
                }
                _ => anyhow::bail!("invalid native metadata string"),
            }
        };
        let window_number = number("window_number")?;
        if let (Some(number), Some(window)) = (
            window_number,
            values.get("window_id").and_then(Value::as_str),
        ) && let Some((_, suffix)) = window.rsplit_once(':')
        {
            anyhow::ensure!(
                suffix.parse::<u64>().ok() == Some(number),
                "conflicting window metadata"
            );
        }
        let context_window_id = text("context_window_id")?
            .map(|id| uuid::Uuid::parse_str(&id).map(|id| id.to_string()))
            .transpose()?;
        let fork_ordinal = number("forked_from_ordinal_exclusive")?;
        if fork_ordinal.is_some() {
            anyhow::ensure!(
                values
                    .get("forked_from_thread_id")
                    .and_then(Value::as_str)
                    .is_some(),
                "fork ordinal requires a parent thread"
            );
        }
        let history_ingest = match values.get("history_ingest_requested") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(value)) => Some(*value),
            _ => anyhow::bail!("invalid history-ingest metadata"),
        };
        let session = crate::subscription_request::selected_session(object, headers)
            .map_err(|_| anyhow::anyhow!("invalid session metadata"))?;
        let flat = object.get("client_metadata").and_then(Value::as_object);
        let native_context = values.get("installation_id").is_some_and(Value::is_string)
            && values.get("agent_name").is_some_and(Value::is_string)
            && values.get("session_id") == flat.and_then(|m| m.get("session_id"))
            && values.get("thread_id") == flat.and_then(|m| m.get("thread_id"));
        let cache_key = object
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .filter(|key| {
                native_context
                    || headers.get("session-id").and_then(|v| v.to_str().ok()) == Some(*key)
            })
            .filter(|key| session.as_deref().is_some_and(|session| session != *key))
            .map(str::to_string);
        if let Some(key) = &cache_key {
            crate::request_state_types::validate_wire_id(key)?;
        }
        let cache_header = cache_key.as_deref().is_some_and(|key| {
            headers
                .get("session-id")
                .and_then(|value| value.to_str().ok())
                == Some(key)
        });
        let parent_response = object
            .get("client_metadata")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("parent_response_id"))
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid parent response"))
            })
            .transpose()?
            .map(str::to_string);
        Ok(Self {
            window_number,
            context_window_id,
            fork_ordinal,
            turn_trigger: text("turn_trigger")?,
            history_ingest,
            cache_key,
            cache_header,
            parent_response,
        })
    }

    pub(crate) fn project(
        &self,
        editor: &mut RequestStateEditor<'_>,
        object: &mut Map<String, Value>,
        headers: &mut HeaderMap,
        identity: &ResolvedRequestIdentity,
    ) -> anyhow::Result<()> {
        if let Some(key) = &self.cache_key {
            // Reuse a known source session's cache affinity without making it the owner.
            // Reserve a UUID for a source that has not sent its own request yet.
            let projected =
                match editor.existing_wire_from_downstream(WireIdDomain::Session, key)? {
                    Some(id) => id,
                    None => {
                        let id = uuid::Uuid::now_v7().to_string();
                        editor.bind_wire_pair(WireIdDomain::Session, key, &id)?;
                        id
                    }
                };
            if self.cache_header {
                headers.insert("session-id", projected.parse()?);
            }
            object.insert("prompt_cache_key".into(), Value::String(projected));
        }
        let metadata = object
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow::anyhow!("client metadata missing"))?;
        if let Some(parent) = &self.parent_response {
            let projected =
                editor.required_wire_from_downstream(WireIdDomain::Response, parent, true)?;
            metadata.insert("parent_response_id".into(), Value::String(projected));
        }
        let mut turn: Map<String, Value> = serde_json::from_str(
            metadata
                .get("x-codex-turn-metadata")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("turn metadata missing"))?,
        )?;
        if self.window_number.is_some() {
            turn.insert("window_number".into(), Value::from(identity.window_number));
        }
        if let Some(id) = &self.context_window_id {
            turn.insert(
                "context_window_id".into(),
                Value::String(editor.wire_from_downstream(WireIdDomain::ContextWindow, id)?),
            );
        }
        if let Some(ordinal) = self.fork_ordinal {
            turn.insert("forked_from_ordinal_exclusive".into(), Value::from(ordinal));
        }
        if let Some(trigger) = &self.turn_trigger {
            turn.insert("turn_trigger".into(), Value::String(trigger.clone()));
        }
        if let Some(ingest) = self.history_ingest {
            turn.insert("history_ingest_requested".into(), Value::Bool(ingest));
        }
        let encoded = crate::ascii_json::to_ascii_json_string(&Value::Object(turn))
            .map_err(|_| anyhow::anyhow!("invalid metadata encoding"))?;
        if let Some(header) =
            crate::request_identity::turn_metadata::bounded_turn_metadata(&encoded)
        {
            headers.insert("x-codex-turn-metadata", header.parse()?);
        }
        metadata.insert("x-codex-turn-metadata".into(), Value::String(encoded));
        Ok(())
    }
}
