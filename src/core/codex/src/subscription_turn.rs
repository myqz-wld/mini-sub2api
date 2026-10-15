//! Select an implicit execution turn only after resolving the target thread.
use super::HistoryImport;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_state_editor::{RequestStateEditor, TurnAssignment};
use crate::request_state_types::WireIdDomain;
use serde_json::Value;
use uuid::Uuid;

pub(super) fn has_new_task(items: &[Value]) -> bool {
    let start = items
        .iter()
        .rposition(model_output)
        .map_or(0, |index| index + 1);
    items[start..].iter().any(crate::agent_message::starts_turn)
}

fn model_output(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => item.get("role").and_then(Value::as_str) == Some("assistant"),
        Some(
            "function_call"
            | "custom_tool_call"
            | "local_shell_call"
            | "web_search_call"
            | "image_generation_call"
            | "tool_search_call"
            | "compaction",
        ) => true,
        _ => false,
    }
}

fn known_turn(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
) -> anyhow::Result<Option<TurnAssignment>> {
    if let Some((_, turn)) = editor.turn_by_id(raw) {
        return Ok(Some(turn));
    }
    if let Some(mapped) = editor.existing_wire_from_downstream(WireIdDomain::Turn, raw)? {
        return Ok(editor.turn_by_id(&mapped).map(|(_, turn)| turn));
    }
    let key = editor.lookup("turn", raw);
    Ok(editor.existing_turn(&key))
}

impl HistoryImport<'_> {
    pub(crate) fn current_turn(
        &self,
        editor: &mut RequestStateEditor<'_>,
        evidence: &RequestIdentityEvidence,
        thread: &str,
        forked: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(turn) = &evidence.turn {
            return Ok(Some(turn.clone()));
        }
        if evidence.is_memory() {
            return Ok(None);
        }
        let fresh = || Some(Uuid::now_v7().to_string());
        if self.plan.starts_new_turn
            || (self.plan.new_turn_input
                && self
                    .plan
                    .identity_baseline()
                    .is_some_and(|base| base.thread_id != thread))
        {
            return Ok(fresh());
        }
        if let Some(base) = self.plan.identity_baseline() {
            if base.thread_id == thread {
                if let Some(turn) = &self.plan.turn {
                    return Ok(Some(turn.clone()));
                }
            } else {
                // The parent supplies history, never the child's execution identity.
                let source = self
                    .plan
                    .evidence
                    .previous
                    .as_deref()
                    .or(base.turn_id.as_deref())
                    .unwrap_or(&base.thread_id);
                let key = editor.derived_lookup(
                    "child-continuation-turn",
                    &[thread.as_bytes(), source.as_bytes()],
                );
                return Ok(Some(
                    Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()).to_string(),
                ));
            }
        }
        if let Some(source) = &self.plan.association
            && source.identity.thread_id == thread
            && let Some(turn) = source
                .identity
                .turn_id
                .as_ref()
                .filter(|turn| !turn.is_empty())
        {
            return Ok(Some(turn.clone()));
        }
        // Prefer the invocation producing the trailing result over a newer thread pointer.
        let input = self.plan.input();
        let mut selected = None;
        for (index, result) in input.iter().enumerate().rev() {
            if model_output(result) || crate::agent_message::starts_turn(result) {
                break;
            }
            if !matches!(
                result.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output" | "tool_search_output")
            ) {
                continue;
            }
            let Some(call_id) = result.get("call_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(call) = input[..index].iter().rev().find(|call| {
                model_output(call) && call.get("call_id").and_then(Value::as_str) == Some(call_id)
            }) else {
                continue;
            };
            let Some(raw) = call
                .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(source) = known_turn(editor, raw)? else {
                continue;
            };
            let turn = if source.thread_id == thread {
                Some(source.id)
            } else {
                self.resolve_turn_for_thread(editor, &source, thread, forked)?
                    .filter(|turn| {
                        editor
                            .turn_by_id(turn)
                            .is_some_and(|(_, owner)| owner.thread_id == thread)
                    })
            };
            if let Some(turn) = turn {
                anyhow::ensure!(
                    selected.as_ref().is_none_or(|previous| previous == &turn),
                    "tool continuation spans execution turns"
                );
                selected = Some(turn);
            }
        }
        Ok(selected
            .or_else(|| editor.current_turn_id(thread))
            .or_else(fresh))
    }
}
