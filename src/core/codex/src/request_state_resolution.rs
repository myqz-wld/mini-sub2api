use anyhow::Result;
use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;

use crate::fingerprint::FingerprintMode;
use crate::request_compaction::PendingCompaction;
use crate::request_compaction::operation_anchor;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use crate::request_state_types::WireIdOwner;
use crate::request_wire_ids::translate_request_ids;

#[path = "request_state_resolution_items.rs"]
mod items;
use items::project_items;
use items::turn_key_for_raw;

#[path = "request_state_resolution_threads.rs"]
mod threads;
use threads::{resolve_conversation, resolve_thread};

struct ResolvedTurn {
    turn_id: Option<String>,
    root_turn_id: Option<String>,
    parent_turn_id: Option<String>,
    started_at_unix_ms: Option<i64>,
}

pub(crate) struct ResolvedProjection {
    pub(crate) identity: ResolvedRequestIdentity,
    pub(crate) synthesized_item_ids: Vec<String>,
    pub(crate) pending_compaction: Option<PendingCompaction>,
}

pub(crate) struct InputProjection<'a> {
    pub(crate) synthesized_item_ids: &'a [String],
    pub(crate) lite_prefixes: &'a [usize],
    pub(crate) native_prefixes: &'a [crate::lite_prefix_identity::NativePrefix],
}

pub(crate) fn resolve_and_project(
    editor: &mut RequestStateEditor<'_>,
    fingerprint_mode: FingerprintMode,
    evidence: &RequestIdentityEvidence,
    headers: &mut http::HeaderMap,
    object: &mut Map<String, Value>,
    input: InputProjection<'_>,
) -> Result<ResolvedProjection> {
    let InputProjection {
        synthesized_item_ids,
        lite_prefixes,
        native_prefixes,
    } = input;
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

    let previous_owner = previous_response_owner(editor, object)?;
    let mut conversation_from_previous = false;
    let conversation = match evidence.conversation.as_deref() {
        Some(raw) => resolve_conversation(editor, raw)?,
        None => match previous_owner.as_ref() {
            Some(owner) => {
                conversation_from_previous = true;
                editor
                    .conversation_by_id(&owner.session_id)
                    .map(|(_, assignment)| assignment)
                    .ok_or_else(|| anyhow::anyhow!("previous response session is missing"))?
            }
            None => match evidence.responses_conversation.as_deref() {
                Some(raw) => {
                    editor.required_wire_from_downstream(WireIdDomain::Conversation, raw, true)?;
                    let key = editor.lookup("responses-conversation", raw);
                    editor.conversation(&key)?
                }
                None => {
                    let anchor = Uuid::now_v7();
                    let key = editor.derived_lookup("conversation-fallback", &[anchor.as_bytes()]);
                    editor.conversation(&key)?
                }
            },
        },
    };
    if let Some(raw) = evidence.conversation.as_deref() {
        editor.bind_wire_pair(WireIdDomain::Session, raw, &conversation.id)?;
    }
    let (thread_id, parent_thread_id, forked_from_thread_id, stored_window) = resolve_thread(
        editor,
        evidence,
        &conversation.id,
        conversation_from_previous
            .then_some(previous_owner.as_ref())
            .flatten(),
    )?;

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
    } else if !evidence.new_user_submission
        && let Some(current) = editor.current_turn_id(&thread_id)
    {
        turn_key_for_raw(editor, &current)?
    } else {
        let anchor = turn_anchor(object, evidence);
        editor.derived_lookup("turn-fallback", &[thread_id.as_bytes(), anchor.as_slice()])
    };
    let reserved_turn = current_turn_raw
        .as_deref()
        .map(|raw| editor.existing_wire_from_downstream(WireIdDomain::Turn, raw))
        .transpose()?
        .flatten();
    let resolved_turn = resolve_turn(
        editor,
        evidence,
        &turn_key,
        &thread_id,
        &conversation.id,
        reserved_turn.as_deref(),
    )?;
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
    if let Some(window) = explicit_window
        && evidence.request_kind != "compaction"
    {
        editor.observe_window_number(&thread_id, window)?;
    }
    let pending_compaction = if evidence.request_kind == "compaction" {
        let marker = operation_anchor(object, evidence, current_turn_raw.is_some(), &turn_key);
        let marker_key =
            editor.derived_lookup("compaction", &[thread_id.as_bytes(), marker.as_slice()]);
        let target_window = editor.begin_compaction(&marker_key, &thread_id)?;
        let pending = PendingCompaction {
            marker_key,
            thread_id: thread_id.clone(),
            target_window,
            requires_compaction_item: crate::request_compaction::requires_compaction_item(object),
        };
        output_window = pending.committed_base();
        Some(pending)
    } else {
        None
    };

    let identity = ResolvedRequestIdentity {
        connection_id: None,
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
    let mut generated_upstream_ids = project_items(
        editor,
        object,
        evidence,
        synthesized_item_ids,
        &identity,
        current_turn_raw.as_deref(),
    )?;
    crate::request_identity_projection::apply(headers, object, &identity)
        .map_err(|_| anyhow::anyhow!("projecting request identity"))?;
    generated_upstream_ids.extend(crate::lite_prefix_identity::apply(
        object,
        &identity.thread_id,
        lite_prefixes,
    )?);
    generated_upstream_ids.extend(crate::lite_prefix_identity::project_native(
        editor,
        object,
        &identity.thread_id,
        evidence.thread.as_deref(),
        native_prefixes,
    )?);
    translate_request_ids(editor, object, &generated_upstream_ids)?;
    Ok(ResolvedProjection {
        identity,
        synthesized_item_ids: Vec::new(),
        pending_compaction,
    })
}

fn resolve_turn(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    turn_key: &str,
    thread_id: &str,
    root_thread_id: &str,
    reserved_turn: Option<&str>,
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
        let turn = editor.turn_with_id(turn_key, thread_id, None, None, reserved_turn)?;
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
    let root_alias = editor.existing_wire_from_downstream(WireIdDomain::Turn, root_raw)?;
    let root = editor.turn_with_id(&root_key, root_thread_id, None, None, root_alias.as_deref())?;
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
                let alias = editor.existing_wire_from_downstream(WireIdDomain::Turn, raw)?;
                editor
                    .turn_with_id(
                        &key,
                        root_thread_id,
                        Some(&root.id),
                        Some(&root.id),
                        alias.as_deref(),
                    )
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
    let turn = editor.turn_with_id(
        turn_key,
        thread_id,
        Some(&root.id),
        parent.as_deref(),
        reserved_turn,
    )?;
    Ok(ResolvedTurn {
        turn_id: Some(turn.id),
        root_turn_id: Some(root.id),
        parent_turn_id: parent,
        started_at_unix_ms: Some(turn.started_at_unix_ms),
    })
}

fn previous_response_owner(
    editor: &mut RequestStateEditor<'_>,
    object: &Map<String, Value>,
) -> Result<Option<WireIdOwner>> {
    let Some(previous) = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    editor
        .required_response_owner_from_downstream(previous)
        .map(Some)
}

fn turn_anchor(object: &Map<String, Value>, evidence: &RequestIdentityEvidence) -> Vec<u8> {
    if let Some(id) = evidence
        .items
        .iter()
        .rev()
        .find(|item| item.is_user)
        .and_then(|item| item.id.as_deref())
    {
        return id.as_bytes().to_vec();
    }
    if let Some(previous) = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let mut anchor = previous.as_bytes().to_vec();
        if let Some(user) = latest_user_anchor(object) {
            anchor.extend_from_slice(&user);
        }
        return anchor;
    }
    Uuid::now_v7().as_bytes().to_vec()
}

fn latest_user_anchor(object: &Map<String, Value>) -> Option<Vec<u8>> {
    object
        .get("input")?
        .as_array()?
        .iter()
        .rev()
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
