//! Stable native Lite prefix identity, generated after the scoped thread has been resolved.
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) fn apply_generated(
    object: &mut Map<String, Value>,
    thread: &str,
    prefixes: &[usize],
) -> anyhow::Result<BTreeSet<String>> {
    let ids = apply(object, thread, prefixes)?;
    for index in prefixes {
        let item = object["input"][*index]
            .as_object_mut()
            .expect("generated prefix");
        // Classification is disabled in the fixed emulation feature policy. Native
        // skips empty metadata; the base has neither a turn stamp nor a creation time.
        item.shift_remove("internal_chat_message_metadata_passthrough");
    }
    Ok(ids)
}

fn apply(
    object: &mut Map<String, Value>,
    thread: &str,
    prefixes: &[usize],
) -> anyhow::Result<BTreeSet<String>> {
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, thread.as_bytes());
    let mut ids = BTreeSet::new();
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(ids);
    };
    for index in prefixes {
        let item = input
            .get_mut(*index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow::anyhow!("Lite prefix is missing"))?;
        let (prefix, payload) =
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                (
                    "at",
                    serde_json::to_vec(
                        item.get("tools")
                            .ok_or_else(|| anyhow::anyhow!("Lite tools missing"))?,
                    )?,
                )
            } else {
                let content = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("Lite base missing"))?;
                anyhow::ensure!(content.len() == 1, "Lite base shape changed");
                let text = content[0]
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("Lite base text missing"))?;
                ("msg", text.as_bytes().to_vec())
            };
        let id = format!("{prefix}_{}", Uuid::new_v5(&namespace, &payload));
        item.insert("id".into(), Value::String(id.clone()));
        ids.insert(id);
    }
    Ok(ids)
}

/// Only the two prompt-prefix positions are eligible. Keep the pre-overlay bytes for proof;
/// schema ordering may change afterward. These witnesses are temporary and never persisted.
pub(crate) struct NativePrefix {
    id: String,
    kind: &'static str,
    payload: Vec<u8>,
    metadata_fields: Option<BTreeSet<String>>,
}

pub(crate) fn capture_native(object: &Map<String, Value>) -> Vec<NativePrefix> {
    let Some(input) = object.get("input").and_then(Value::as_array) else {
        return Vec::new();
    };
    if input
        .first()
        .and_then(|i| i.get("type"))
        .and_then(Value::as_str)
        != Some("additional_tools")
    {
        return Vec::new();
    }
    let mut prefixes = Vec::new();
    if let (Some(id), Some(tools)) = (
        input[0].get("id").and_then(Value::as_str),
        input[0].get("tools"),
    ) && let Ok(payload) = serde_json::to_vec(tools)
    {
        prefixes.push(NativePrefix {
            id: id.to_string(),
            kind: "at",
            payload,
            metadata_fields: metadata_fields(&input[0]),
        });
    }
    if let Some(base) = input.get(1)
        && base.get("type").and_then(Value::as_str) == Some("message")
        && base.get("role").and_then(Value::as_str) == Some("developer")
        && let (Some(id), Some(content)) = (
            base.get("id").and_then(Value::as_str),
            base.get("content").and_then(Value::as_array),
        )
        && content.len() == 1
        && content[0].get("type").and_then(Value::as_str) == Some("input_text")
        && let Some(text) = content[0].get("text").and_then(Value::as_str)
        && !text.is_empty()
    {
        prefixes.push(NativePrefix {
            id: id.to_string(),
            kind: "msg",
            payload: text.as_bytes().to_vec(),
            metadata_fields: metadata_fields(base),
        });
    }
    prefixes
}

fn metadata_fields(item: &Value) -> Option<BTreeSet<String>> {
    item.get("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object)
        .map(|metadata| metadata.keys().cloned().collect())
}

pub(crate) fn project_native(
    editor: &mut crate::request_state_editor::RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    thread: &str,
    caller_thread: Option<&str>,
    prefixes: &[NativePrefix],
) -> anyhow::Result<BTreeSet<String>> {
    use crate::request_state_types::WireIdDomain;
    let mut generated = BTreeSet::new();
    if prefixes.is_empty() {
        return Ok(generated);
    }
    let inverse = editor.existing_wire_from_upstream(WireIdDomain::Thread, thread)?;
    let candidates: BTreeSet<&str> = caller_thread
        .into_iter()
        .chain(inverse.as_deref())
        .chain([thread])
        .collect();
    for prefix in prefixes {
        let proven = candidates.iter().any(|candidate| {
            let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, candidate.as_bytes());
            prefix.id
                == format!(
                    "{}_{}",
                    prefix.kind,
                    Uuid::new_v5(&namespace, &prefix.payload)
                )
        });
        if !proven {
            continue;
        }
        let Some(index) = object
            .get("input")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .position(|item| item.get("id").and_then(Value::as_str) == Some(&prefix.id))
            })
        else {
            continue;
        };
        // Setup prefixes have no per-turn attribution in native requests. Keep the caller's
        // original presence/empty-object shape after proving the deterministic prefix identity.
        let item = object["input"][index].as_object_mut().expect("prefix item");
        if let Some(fields) = &prefix.metadata_fields {
            if let Some(metadata) = item
                .get_mut("internal_chat_message_metadata_passthrough")
                .and_then(Value::as_object_mut)
            {
                metadata.retain(|name, _| fields.contains(name));
            }
        } else {
            item.shift_remove("internal_chat_message_metadata_passthrough");
        }
        // Existing reversible bindings may be used by live upstream contexts created by older
        // versions. Preserve them; silently rotating one would break valid historical references.
        if let Some(existing) =
            editor.existing_wire_from_downstream(WireIdDomain::Item, &prefix.id)?
        {
            object["input"][index]["id"] = Value::String(existing.clone());
            generated.insert(existing);
            continue;
        }
        let ids = apply(object, thread, &[index])?;
        let projected = ids
            .iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("native prefix projection missing"))?;
        editor.bind_wire_pair(WireIdDomain::Item, &prefix.id, projected)?;
        generated.extend(ids);
    }
    Ok(generated)
}
