use anyhow::Context;
use anyhow::Result;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::fingerprint::FingerprintMode;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use crate::request_wire_ids::translate_request_ids;

struct ResolvedTurn {
    turn_id: Option<String>,
    root_turn_id: Option<String>,
    parent_turn_id: Option<String>,
    started_at_unix_ms: Option<i64>,
}

pub(crate) struct ResolvedProjection {
    pub(crate) identity: ResolvedRequestIdentity,
    pub(crate) synthesized_item_ids: Vec<String>,
}

pub(crate) fn resolve_and_project(
    editor: &mut RequestStateEditor<'_>,
    fingerprint_mode: FingerprintMode,
    evidence: &RequestIdentityEvidence,
    headers: &mut http::HeaderMap,
    object: &mut Map<String, Value>,
    synthesized_item_ids: &[String],
) -> Result<ResolvedProjection> {
    let installation_lookup = evidence
        .installation
        .as_deref()
        .map(|raw| editor.lookup("installation", raw))
        .unwrap_or_else(|| editor.derived_lookup("installation-absent", &[b"absent"]));
    let installation_id = editor.installation_id(
        fingerprint_mode,
        (fingerprint_mode == FingerprintMode::Off).then_some(installation_lookup.as_str()),
    )?;
    if let Some(raw) = evidence.installation.as_deref() {
        editor.bind_wire_pair(WireIdDomain::Installation, raw, &installation_id)?;
    }

    let conversation = match evidence.conversation.as_deref() {
        Some(raw) => resolve_conversation(editor, raw)?,
        None => {
            let anchor = conversation_anchor(object);
            let key = editor.derived_lookup("conversation-fallback", &[anchor.as_slice()]);
            editor.conversation(&key)?
        }
    };
    if let Some(raw) = evidence.conversation.as_deref() {
        editor.bind_wire_pair(WireIdDomain::Session, raw, &conversation.id)?;
    }
    let (thread_id, parent_thread_id, forked_from_thread_id, stored_window) =
        resolve_thread(editor, evidence, &conversation.id)?;

    let current_turn_raw = evidence
        .turn
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            evidence
                .items
                .iter()
                .rev()
                .find_map(|item| item.turn_id.clone())
        });
    let turn_key = if let Some(raw) = current_turn_raw.as_deref() {
        turn_key_for_raw(editor, raw)?
    } else if !has_user_message(object)
        && let Some(current) = editor.current_turn_id(&thread_id)
    {
        turn_key_for_raw(editor, &current)?
    } else {
        let anchor = turn_anchor(object);
        editor.derived_lookup("turn-fallback", &[thread_id.as_bytes(), anchor.as_slice()])
    };
    let resolved_turn = resolve_turn(editor, evidence, &turn_key, &thread_id, &conversation.id)?;
    let turn_id = resolved_turn.turn_id;
    let root_turn_id = resolved_turn.root_turn_id;
    let parent_turn_id = resolved_turn.parent_turn_id;
    let turn_started_at_unix_ms = resolved_turn.started_at_unix_ms;
    if let (Some(raw), Some(projected)) = (current_turn_raw.as_deref(), turn_id.as_deref()) {
        editor.bind_wire_pair(WireIdDomain::Turn, raw, projected)?;
    }
    if let Some(projected) = turn_id.as_deref().filter(|turn| !turn.is_empty()) {
        editor.set_current_turn(&thread_id, projected)?;
    }
    if let Some(raw) = evidence.thread.as_deref() {
        editor.bind_wire_pair(WireIdDomain::Thread, raw, &thread_id)?;
    }
    if let (Some(raw), Some(projected)) = (
        evidence.parent_thread.as_deref(),
        parent_thread_id.as_deref(),
    ) {
        editor.bind_wire_pair(WireIdDomain::Thread, raw, projected)?;
    }
    if let (Some(raw), Some(projected)) = (
        evidence.forked_from_thread.as_deref(),
        forked_from_thread_id.as_deref(),
    ) {
        editor.bind_wire_pair(WireIdDomain::Thread, raw, projected)?;
    }
    if let (Some(raw), Some(projected)) = (evidence.root_turn.as_deref(), root_turn_id.as_deref()) {
        editor.bind_wire_pair(WireIdDomain::Turn, raw, projected)?;
    }
    if let (Some(raw), Some(projected)) =
        (evidence.parent_turn.as_deref(), parent_turn_id.as_deref())
    {
        editor.bind_wire_pair(WireIdDomain::Turn, raw, projected)?;
    }

    let explicit_window = evidence.window_number;
    let mut output_window = explicit_window.unwrap_or(stored_window);
    if let Some(window) = explicit_window {
        editor.observe_window_number(&thread_id, window)?;
    }
    if evidence.request_kind == "compaction" {
        let marker = compaction_anchor(object, &turn_key);
        let marker_key = editor.derived_lookup("compaction", &[marker.as_slice()]);
        let next_window = editor.apply_compaction(&marker_key, &thread_id)?;
        output_window = explicit_window.unwrap_or_else(|| next_window.saturating_sub(1));
    }

    let identity = ResolvedRequestIdentity {
        installation_id,
        session_id: conversation.id,
        thread_id,
        parent_thread_id,
        forked_from_thread_id,
        turn_id,
        root_turn_id,
        parent_turn_id,
        window_number: output_window,
        request_kind: evidence.request_kind.clone(),
        turn_started_at_unix_ms,
    };
    let generated_upstream_ids = project_items(
        editor,
        object,
        evidence,
        synthesized_item_ids,
        &identity,
        current_turn_raw.as_deref(),
    )?;
    crate::request_identity_projection::apply(headers, object, &identity)
        .map_err(|_| anyhow::anyhow!("projecting request identity"))?;
    translate_request_ids(editor, object, &generated_upstream_ids)?;
    Ok(ResolvedProjection {
        identity,
        synthesized_item_ids: generated_upstream_ids.into_iter().collect(),
    })
}

fn resolve_conversation(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
) -> Result<crate::request_state_editor::ConversationAssignment> {
    if let Some((_, assignment)) = editor.conversation_by_id(raw) {
        return Ok(assignment);
    }
    if let Some(projected) = editor.existing_wire_from_downstream(WireIdDomain::Session, raw)?
        && let Some((_, assignment)) = editor.conversation_by_id(&projected)
    {
        return Ok(assignment);
    }
    let key = editor.lookup("conversation", raw);
    editor.conversation(&key)
}

fn resolve_thread(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    session_id: &str,
) -> Result<(String, Option<String>, Option<String>, u64)> {
    if !evidence.explicit_thread_lineage {
        let window = editor
            .window_number(session_id)
            .ok_or_else(|| anyhow::anyhow!("root conversation window is missing"))?;
        return Ok((session_id.to_string(), None, None, window));
    }
    let parent_raw = evidence
        .parent_thread
        .as_deref()
        .or(evidence.forked_from_thread.as_deref());
    let parent = match parent_raw {
        Some(raw) if evidence.conversation.as_deref() == Some(raw) => session_id.to_string(),
        Some(raw) => resolve_thread_reference(editor, raw, session_id)?,
        None => session_id.to_string(),
    };
    let child = match evidence.thread.as_deref() {
        Some(raw) => match resolve_existing_child(editor, raw)? {
            Some(child) => {
                anyhow::ensure!(
                    child.session_id == session_id
                        && child.parent_thread_id.as_deref() == Some(parent.as_str()),
                    "child thread relationship changed"
                );
                child
            }
            None => {
                let key = editor.lookup("thread", raw);
                editor.child_thread(&key, session_id, Some(&parent))?
            }
        },
        None => {
            let key = editor.derived_lookup(
                "thread-fallback",
                &[
                    session_id.as_bytes(),
                    parent.as_bytes(),
                    evidence.request_kind.as_bytes(),
                ],
            );
            editor.child_thread(&key, session_id, Some(&parent))?
        }
    };
    let forked = evidence
        .forked_from_thread
        .as_deref()
        .map(|raw| resolve_thread_reference(editor, raw, session_id))
        .transpose()?;
    Ok((child.id, Some(parent), forked, child.window_number))
}

fn resolve_thread_reference(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
    session_id: &str,
) -> Result<String> {
    if raw == session_id {
        return Ok(session_id.to_string());
    }
    if let Some(existing) = resolve_existing_child(editor, raw)? {
        anyhow::ensure!(
            existing.session_id == session_id,
            "thread reference crosses sessions"
        );
        return Ok(existing.id);
    }
    let key = editor.lookup("thread", raw);
    if let Some(existing) = editor.existing_child_thread(&key) {
        anyhow::ensure!(
            existing.session_id == session_id,
            "thread reference crosses sessions"
        );
        return Ok(existing.id);
    }
    editor
        .child_thread(&key, session_id, Some(session_id))
        .map(|thread| thread.id)
}

fn resolve_existing_child(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
) -> Result<Option<crate::request_state_editor::ThreadAssignment>> {
    if let Some((_, assignment)) = editor.child_thread_by_id(raw) {
        return Ok(Some(assignment));
    }
    if let Some(projected) = editor.existing_wire_from_downstream(WireIdDomain::Thread, raw)?
        && let Some((_, assignment)) = editor.child_thread_by_id(&projected)
    {
        return Ok(Some(assignment));
    }
    Ok(None)
}

fn resolve_turn(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    turn_key: &str,
    thread_id: &str,
    root_thread_id: &str,
) -> Result<ResolvedTurn> {
    if evidence.is_memory() {
        return Ok(ResolvedTurn {
            turn_id: None,
            root_turn_id: None,
            parent_turn_id: None,
            started_at_unix_ms: None,
        });
    }
    if evidence.is_prewarm() {
        return Ok(ResolvedTurn {
            turn_id: Some(String::new()),
            root_turn_id: None,
            parent_turn_id: None,
            started_at_unix_ms: None,
        });
    }
    let child_lineage = evidence.parent_turn.is_some()
        || (evidence.explicit_thread_lineage && evidence.root_turn.is_some());
    if !child_lineage {
        let turn = editor.turn(turn_key, thread_id, None, None)?;
        return Ok(ResolvedTurn {
            turn_id: Some(turn.id.clone()),
            root_turn_id: Some(turn.id),
            parent_turn_id: None,
            started_at_unix_ms: Some(turn.started_at_unix_ms),
        });
    }

    let root_raw = evidence
        .root_turn
        .as_deref()
        .or(evidence.parent_turn.as_deref())
        .unwrap_or(turn_key);
    let root_key = turn_key_for_raw(editor, root_raw)?;
    let root = editor.turn(&root_key, root_thread_id, None, None)?;
    let parent = evidence
        .parent_turn
        .as_deref()
        .map(|raw| {
            let key = turn_key_for_raw(editor, raw)?;
            if key == root_key {
                Ok(root.id.clone())
            } else if let Some(existing) = editor.existing_turn(&key) {
                Ok(existing.id)
            } else {
                editor
                    .turn(&key, root_thread_id, Some(&root.id), Some(&root.id))
                    .map(|turn| turn.id)
            }
        })
        .transpose()?;
    if turn_key == root_key {
        return Ok(ResolvedTurn {
            turn_id: Some(root.id.clone()),
            root_turn_id: Some(root.id),
            parent_turn_id: parent,
            started_at_unix_ms: Some(root.started_at_unix_ms),
        });
    }
    let turn = editor.turn(turn_key, thread_id, Some(&root.id), parent.as_deref())?;
    Ok(ResolvedTurn {
        turn_id: Some(turn.id),
        root_turn_id: Some(root.id),
        parent_turn_id: parent,
        started_at_unix_ms: Some(turn.started_at_unix_ms),
    })
}

fn project_items(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    evidence: &RequestIdentityEvidence,
    synthesized_item_ids: &[String],
    identity: &ResolvedRequestIdentity,
    current_turn_raw: Option<&str>,
) -> Result<BTreeSet<String>> {
    let synthesized = synthesized_item_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut generated_upstream = BTreeSet::new();
    let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(generated_upstream);
    };
    for (index, item) in items.iter_mut().enumerate() {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let temporary_id = item.get("id").and_then(Value::as_str).map(str::to_string);
        let raw = temporary_id.as_deref().and_then(|id| {
            evidence
                .items
                .iter()
                .find(|evidence| evidence.id.as_deref() == Some(id))
        });
        let projected_turn = project_item_turn(
            editor,
            raw.and_then(|item| item.turn_id.as_deref()),
            current_turn_raw,
            identity,
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
            let assignment = editor.generated_item(
                &key,
                prefix,
                projected_turn.as_deref().filter(|turn| !turn.is_empty()),
                add_create_time,
            )?;
            if is_synthesized {
                item.insert("id".to_string(), Value::String(assignment.id.clone()));
                editor.wire_from_upstream(
                    crate::request_state_types::WireIdDomain::Item,
                    &assignment.id,
                )?;
                generated_upstream.insert(assignment.id);
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
    let projected = editor.turn(&key, &identity.thread_id, None, None)?.id;
    editor.bind_wire_pair(WireIdDomain::Turn, raw, &projected)?;
    Ok(Some(projected))
}

fn turn_key_for_raw(editor: &mut RequestStateEditor<'_>, raw: &str) -> Result<String> {
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

fn conversation_anchor(object: &Map<String, Value>) -> Vec<u8> {
    if let Some(anchor) = first_user_anchor(object) {
        return anchor;
    }
    if let Some(previous) = object.get("previous_response_id").and_then(Value::as_str) {
        return previous.as_bytes().to_vec();
    }
    Uuid::now_v7().as_bytes().to_vec()
}

fn turn_anchor(object: &Map<String, Value>) -> Vec<u8> {
    let Some(items) = object.get("input").and_then(Value::as_array) else {
        return Uuid::now_v7().as_bytes().to_vec();
    };
    let mut users = 0_u64;
    let mut latest = None;
    for item in items {
        if item.get("role").and_then(Value::as_str) == Some("user") {
            users = users.saturating_add(1);
            latest = item.as_object().map(item_anchor);
        }
    }
    match latest {
        Some(mut anchor) => {
            anchor.extend_from_slice(&users.to_be_bytes());
            anchor
        }
        None => Uuid::now_v7().as_bytes().to_vec(),
    }
}

fn has_user_message(object: &Map<String, Value>) -> bool {
    object
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        })
}

fn first_user_anchor(object: &Map<String, Value>) -> Option<Vec<u8>> {
    object
        .get("input")?
        .as_array()?
        .iter()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))?
        .as_object()
        .map(item_anchor)
}

fn item_anchor(item: &Map<String, Value>) -> Vec<u8> {
    let mut item = item.clone();
    item.remove("id");
    item.remove("internal_chat_message_metadata_passthrough");
    serde_json::to_vec(&Value::Object(item)).unwrap_or_default()
}

fn compaction_anchor(object: &Map<String, Value>, turn_key: &str) -> Vec<u8> {
    let metadata = object
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("x-codex-turn-metadata"))
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    if let Some(compaction) = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("compaction"))
    {
        return serde_json::to_vec(compaction).unwrap_or_else(|_| turn_key.as_bytes().to_vec());
    }
    turn_key.as_bytes().to_vec()
}
