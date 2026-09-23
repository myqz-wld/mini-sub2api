use anyhow::Context;
use anyhow::Result;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::collections::BTreeSet;

use super::item_anchor;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

pub(super) fn project_items(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    evidence: &RequestIdentityEvidence,
    synthesized_item_ids: &[String],
    identity: &ResolvedRequestIdentity,
    current_turn_raw: Option<&str>,
    history_import: Option<&crate::subscription_prepare::HistoryImport<'_>>,
) -> Result<BTreeSet<String>> {
    let synthesized = synthesized_item_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let generated_upstream = BTreeSet::new();
    let supplied_turns: BTreeSet<_> = evidence
        .items
        .iter()
        .filter_map(|item| item.turn_id.as_deref())
        .collect();
    let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(generated_upstream);
    };
    // Emulation prepends tool/base items; the caller's business items keep their order.
    let prefix_count = items.len().saturating_sub(evidence.items.len());
    for (index, item) in items.iter_mut().enumerate() {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) == Some("configuration_update") {
            continue;
        }
        let temporary_id = item.get("id").and_then(Value::as_str).map(str::to_string);
        // Repeated item IDs must retain each occurrence's own turn evidence. Looking up
        // only the first matching ID could hide a later, unrelated historical owner.
        let raw = index
            .checked_sub(prefix_count)
            .and_then(|index| evidence.items.get(index))
            .filter(|item| item.id.is_none() || item.id == temporary_id)
            .or_else(|| {
                temporary_id.as_deref().and_then(|id| {
                    evidence
                        .items
                        .iter()
                        .find(|evidence| evidence.id.as_deref() == Some(id))
                })
            });
        let projected_turn = project_item_turn(
            editor,
            raw.and_then(|item| item.turn_id.as_deref()).or_else(|| {
                item.get("internal_chat_message_metadata_passthrough")
                    .and_then(|metadata| metadata.get("turn_id"))
                    .and_then(Value::as_str)
                    .filter(|turn| supplied_turns.contains(turn))
            }),
            current_turn_raw,
            identity,
            history_import,
        )?;
        set_item_turn(
            item,
            projected_turn.as_deref(),
            evidence.is_prewarm(),
            evidence.is_memory(),
        );

        let is_synthesized = temporary_id
            .as_ref()
            .is_some_and(|id| synthesized.contains(id));
        let add_create_time = crate::response_item_metadata::adds_create_time(item)
            && raw.is_none_or(|raw| !raw.had_create_time);
        if is_synthesized || add_create_time {
            let anchor = item_anchor(item);
            let index = (index as u64).to_be_bytes();
            let key = editor.derived_lookup(
                "generated-item",
                &[
                    identity.thread_id.as_bytes(),
                    projected_turn.as_deref().unwrap_or_default().as_bytes(),
                    &index,
                    anchor.as_slice(),
                ],
            );
            let prefix = item
                .get("type")
                .and_then(Value::as_str)
                .and_then(crate::responses_lite::item_id_prefix)
                .unwrap_or("item");
            let owned_turn = projected_turn
                .as_deref()
                .filter(|turn| !turn.is_empty() && editor.turn_by_id(turn).is_some());
            let assignment = editor.generated_item(&key, prefix, owned_turn, add_create_time)?;
            if is_synthesized {
                // Ordinary Responses input may omit IDs. Preserve that omission instead of
                // changing all historical IDs when a new turn is projected.
                item.remove("id");
            }
            if add_create_time && let Some(micros) = assignment.create_time_micros {
                set_create_time(item, micros)?;
            }
        }
    }
    Ok(generated_upstream)
}

fn project_item_turn(
    editor: &mut RequestStateEditor<'_>,
    raw: Option<&str>,
    current_raw: Option<&str>,
    identity: &ResolvedRequestIdentity,
    history_import: Option<&crate::subscription_prepare::HistoryImport<'_>>,
) -> Result<Option<String>> {
    if identity.request_kind == "memory" {
        return Ok(raw.map(str::to_string));
    }
    if identity.request_kind == "prewarm" {
        return Ok(Some(String::new()));
    }
    if raw.is_none() || raw == current_raw {
        return Ok(identity.turn_id.clone());
    }
    let raw = raw.expect("checked above");
    let key = turn_key_for_raw(editor, raw)?;
    let projected = if let Some(existing) = editor.existing_turn(&key) {
        if !editor.history_thread_allowed(&existing.thread_id, identity)
            && let Some(import) = history_import
            && let Some(imported) = import.resolve_turn(editor, &existing, identity)?
        {
            // The source's global alias must keep its original owner. This copy belongs
            // solely to the target thread and has a separate reversible public identity.
            return Ok(Some(imported));
        }
        anyhow::ensure!(
            editor.history_thread_allowed(&existing.thread_id, identity),
            "historical turn belongs to an unrelated thread"
        );
        existing.id
    } else {
        // Supplied history may predate this gateway. Preserve a reversible alias, but do not
        // invent its owning thread or causal parent. A later real turn assignment adopts it.
        editor
            .existing_wire_from_downstream(WireIdDomain::Turn, raw)?
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
    };
    editor.bind_wire_pair(WireIdDomain::Turn, raw, &projected)?;
    Ok(Some(projected))
}

pub(super) fn turn_key_for_raw(editor: &mut RequestStateEditor<'_>, raw: &str) -> Result<String> {
    if let Some((key, _)) = editor.turn_by_id(raw) {
        return Ok(key);
    }
    if let Some(projected) = editor.existing_wire_from_downstream(WireIdDomain::Turn, raw)?
        && let Some((key, _)) = editor.turn_by_id(&projected)
    {
        return Ok(key);
    }
    Ok(editor.lookup("turn", raw))
}

fn set_item_turn(item: &mut Map<String, Value>, turn: Option<&str>, prewarm: bool, memory: bool) {
    let metadata = item
        .entry("internal_chat_message_metadata_passthrough".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let metadata = metadata.as_object_mut().expect("item metadata object");
    if memory {
        metadata.remove("turn_id");
    } else {
        metadata.insert(
            "turn_id".to_string(),
            Value::String(
                if prewarm {
                    ""
                } else {
                    turn.unwrap_or_default()
                }
                .to_string(),
            ),
        );
    }
}

fn set_create_time(item: &mut Map<String, Value>, micros: i64) -> Result<()> {
    let seconds = micros as f64 / 1_000_000.0;
    let number = Number::from_f64(seconds).context("invalid generated item create time")?;
    let metadata = item
        .get_mut("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("item metadata is missing"))?;
    metadata.insert("create_time".to_string(), Value::Number(number));
    Ok(())
}
