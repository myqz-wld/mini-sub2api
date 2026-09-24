//! Independent historical turn copies for self-contained anonymous full replay.
use super::ContextPlan;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::{RequestStateEditor, TurnAssignment};
use crate::request_state_types::WireIdDomain;
use crate::subscription_context::ContextStore;
use serde_json::Value;

pub(crate) struct HistoryImport<'a> {
    pub(crate) plan: &'a ContextPlan,
    pub(crate) store: &'a ContextStore,
}

impl ContextPlan {
    pub(crate) fn enable_history_import(
        &mut self,
        identity: &RequestIdentityEvidence,
        bound: bool,
    ) {
        // Complete first replay can carry explicit session/turn identity. This does not grant
        // the separate ability to copy expired historical turns from an unrelated owner.
        self.preserve_imported_outputs = !bound
            && self.evidence.previous.is_none()
            && self.baseline.is_none()
            && self.checkpoint.is_none()
            && self.restored_input.is_none()
            && !self.external_context
            && identity.request_kind == "turn"
            && !self.dependencies.awaiting_tools()
            && self_contained(&self.evidence.input, false);
        self.allow_history_import = !bound
            && self.evidence.session.is_none()
            && self.evidence.turn.is_none()
            && self.evidence.previous.is_none()
            && self.baseline.is_none()
            && self.checkpoint.is_none()
            && self.restored_input.is_none()
            && !self.external_context
            && identity.request_kind == "turn"
            && identity.thread.is_none()
            && identity.parent_thread.is_none()
            && identity.forked_from_thread.is_none()
            && identity.root_turn.is_none()
            && identity.parent_turn.is_none()
            && !identity.explicit_thread_lineage
            && !self.dependencies.awaiting_tools()
            && self_contained(&self.evidence.input, true);
    }
}

fn self_contained(items: &[Value], detached_turn_import: bool) -> bool {
    let mut seen_user = false;
    let mut ids = std::collections::BTreeMap::new();
    for item in items {
        if let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            && ids
                .insert(id, item)
                .is_some_and(|previous| previous != item)
        {
            // Identical history repetition is valid. Conflicting declarations of one item
            // cannot establish an unambiguous source turn or authorize a detached import.
            return false;
        }
        match item.get("type").and_then(Value::as_str) {
            Some("message") => match item.get("role").and_then(Value::as_str) {
                Some("user") => seen_user = true,
                Some("system" | "developer") => {}
                Some("assistant") if seen_user => {}
                _ => return false,
            },
            Some("additional_tools" | "configuration_update") => {}
            Some("reasoning") if seen_user => {
                if detached_turn_import
                    && item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                {
                    return false;
                }
            }
            Some(
                "function_call"
                | "function_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output",
            ) if seen_user => {}
            Some(
                "local_shell_call"
                | "web_search_call"
                | "image_generation_call"
                | "tool_search_call"
                | "tool_search_output",
            ) if seen_user && !detached_turn_import => {}
            _ => return false,
        }
    }
    seen_user
}

impl HistoryImport<'_> {
    pub(crate) fn resolve_turn(
        &self,
        editor: &mut RequestStateEditor<'_>,
        source: &TurnAssignment,
        target: &ResolvedRequestIdentity,
    ) -> anyhow::Result<Option<String>> {
        let key = editor.derived_lookup(
            "imported-history-turn",
            &[target.thread_id.as_bytes(), source.id.as_bytes()],
        );
        // An imported copy is part of its target's ordinary history. Its descendants and
        // declared forks may reuse it under the same ancestry policy as other historical turns.
        let mut visited = std::collections::BTreeSet::new();
        for root in [
            Some(target.thread_id.as_str()),
            target.forked_from_thread_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let mut thread = root.to_string();
            while visited.insert(thread.clone()) {
                let candidate = editor.derived_lookup(
                    "imported-history-turn",
                    &[thread.as_bytes(), source.id.as_bytes()],
                );
                if let Some(imported) = editor.existing_turn(&candidate) {
                    anyhow::ensure!(imported.thread_id == thread, "imported turn owner changed");
                    return Ok(Some(imported.id));
                }
                let Some(parent) = editor
                    .child_thread_by_id(&thread)
                    .and_then(|(_, entry)| entry.parent_thread_id)
                else {
                    break;
                };
                thread = parent;
            }
        }
        if !self.plan.allow_history_import {
            return Ok(None);
        }
        let session = editor
            .child_thread_by_id(&source.thread_id)
            .map(|(_, thread)| thread.session_id)
            .unwrap_or_else(|| source.thread_id.clone());
        let mut inner = self
            .store
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context store poisoned"))?;
        inner.expire(std::time::Instant::now());
        let retained = inner.scopes.get(&self.plan.scope).is_some_and(|scope| {
            scope
                .records
                .values()
                .any(|record| record.identity.session_id == session && record.history.is_some())
        });
        let active = inner.operations.values().any(|operation| {
            operation.scope == self.plan.scope && operation.record.identity.session_id == session
        }) || inner
            .reservations
            .values()
            .any(|pending| pending.scope == self.plan.scope && pending.session == session);
        if retained || active {
            return Ok(None);
        }
        drop(inner);
        let imported = editor.turn_with_id(&key, &target.thread_id, None, None, None)?;
        editor.wire_from_upstream(WireIdDomain::Turn, &imported.id)?;
        Ok(Some(imported.id))
    }
}
