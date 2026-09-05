//! Optional v0.153.4 lifecycle metadata. These fields never locate a session.
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
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        let values: Map<String, Value> = serde_json::from_str(raw)?;
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
        Ok(Self {
            window_number,
            context_window_id,
            fork_ordinal,
            turn_trigger: text("turn_trigger")?,
            history_ingest,
        })
    }

    pub(crate) fn project(
        &self,
        editor: &mut RequestStateEditor<'_>,
        object: &mut Map<String, Value>,
        headers: &mut HeaderMap,
        identity: &ResolvedRequestIdentity,
    ) -> anyhow::Result<()> {
        let metadata = object
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow::anyhow!("client metadata missing"))?;
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
