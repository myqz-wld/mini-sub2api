//! Persist only negative inventory evidence, scoped by Key and source thread.
use super::RequestStateEditor;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::MAX_TOOL_INVENTORY_REVOCATIONS;
use crate::request_state_types::WireIdDomain;
use crate::tool_observation_budget::{self as budget, Origin};
use serde_json::{Map, Value};

impl RequestStateEditor<'_> {
    pub(crate) fn preserve_inventory_origin(
        &mut self,
        imported_key: &str,
        source: &super::TurnAssignment,
    ) -> anyhow::Result<()> {
        let owner = source
            .inventory_source_thread_id
            .as_ref()
            .unwrap_or(&source.thread_id);
        let entry = self
            .scope_mut()
            .turns
            .get_mut(imported_key)
            .ok_or_else(|| anyhow::anyhow!("imported inventory turn is missing"))?;
        if let Some(existing) = &entry.inventory_source_thread_id {
            anyhow::ensure!(existing == owner, "inventory source ownership changed");
        } else {
            entry.inventory_source_thread_id = Some(owner.clone());
            self.changed = true;
        }
        Ok(())
    }

    fn inventory_keys(&mut self, origin: &Origin, thread: &str) -> Option<Vec<String>> {
        let owners = if let Some(turn) = &origin.turn {
            let turn = self.turn_by_id(turn)?.1;
            let mut owners = vec![turn.thread_id];
            if let Some(source) = turn.inventory_source_thread_id
                && !owners.contains(&source)
            {
                owners.push(source);
            }
            owners
        } else {
            vec![thread.to_string()]
        };
        let mut keys = Vec::new();
        for owner in owners {
            for (kind, raw, domain) in [
                ("tool-cell-loss", &origin.cell, WireIdDomain::Call),
                ("tool-item-loss", &origin.item, WireIdDomain::Item),
                ("tool-call-loss", &origin.call, WireIdDomain::Call),
            ] {
                if let Some(raw) = raw.as_deref().filter(|raw| !raw.is_empty()) {
                    // Full imported history and ordinary replay may use opposite
                    // representations of the same scoped invocation. Neither alias
                    // choice nor optional item-ID omission restores completeness.
                    let mut anchors = std::collections::BTreeSet::from([raw.to_string()]);
                    anchors.extend(self.existing_wire_from_upstream(domain, raw).ok()?);
                    anchors.extend(self.existing_wire_from_downstream(domain, raw).ok()?);
                    for anchor in anchors {
                        keys.push(
                            self.derived_lookup(kind, &[owner.as_bytes(), anchor.as_bytes()]),
                        );
                    }
                }
            }
        }
        (!keys.is_empty()).then_some(keys)
    }

    pub(crate) fn revoke_tool_inventories(&mut self, losses: &[Origin], thread: &str) {
        if self.state.tool_inventory_uncertain {
            return;
        }
        for origin in losses {
            if self.state.tool_inventory_revision == 0 {
                self.state.tool_inventory_revision = 1;
                self.changed = true;
            }
            let Some(keys) = self.inventory_keys(origin, thread) else {
                // An imported turn alias may have no retained owner assignment. Keep negative
                // evidence conservatively even if the same cell later has a resolvable turn.
                self.state.tool_inventory_uncertain = true;
                self.state.tool_inventory_revocations.clear();
                self.changed = true;
                return;
            };
            for key in keys {
                if self.state.tool_inventory_revocations.contains(&key) {
                    continue;
                }
                self.changed = true;
                if self.state.tool_inventory_revocations.len() == MAX_TOOL_INVENTORY_REVOCATIONS {
                    // Never evict negative evidence and then accept a contradictory true claim.
                    self.state.tool_inventory_uncertain = true;
                    self.state.tool_inventory_revocations.clear();
                    return;
                }
                self.state.tool_inventory_revocations.insert(key);
            }
        }
    }

    pub(crate) fn filter_tool_completeness(&mut self, value: &mut Value, thread: &str) {
        let Some(items) = value.get_mut("input").and_then(Value::as_array_mut) else {
            return;
        };
        for item in items {
            if item
                .get(budget::META)
                .and_then(|m| m.get("tool_calls_complete"))
                .is_none()
            {
                continue;
            }
            let origin = budget::origin(item);
            let revoked = self.state.tool_inventory_uncertain
                || self.inventory_keys(&origin, thread).is_none_or(|keys| {
                    keys.iter()
                        .any(|key| self.state.tool_inventory_revocations.contains(key))
                });
            if revoked
                && let Some(metadata) = item.get_mut(budget::META).and_then(Value::as_object_mut)
            {
                metadata.shift_remove("tool_calls_complete");
            }
        }
    }
}

pub(crate) fn prepare(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    thread: &str,
) {
    if !object
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| budget::observation_bytes(item) > 0))
    {
        return;
    }
    let mut value = Value::Object(std::mem::take(object));
    editor.filter_tool_completeness(&mut value, thread);
    let losses = budget::prompt(&mut value);
    editor.revoke_tool_inventories(&losses, thread);
    editor.filter_tool_completeness(&mut value, thread);
    *object = value.as_object_mut().map(std::mem::take).unwrap();
}

pub(crate) fn finalize(editor: &mut RequestStateEditor<'_>, value: &mut Value, thread: &str) {
    // Recheck even a previously prepared frame with no new losses. This and publication share
    // the account/Key state transaction, ordered against other HTTP and WS emission preparation.
    editor.filter_tool_completeness(value, thread);
    let losses = budget::message(value);
    editor.revoke_tool_inventories(&losses, thread);
    editor.filter_tool_completeness(value, thread);
}

pub(crate) async fn finalize_frame(
    store: &RequestStateStore,
    namespace: &str,
    account: &str,
    scope: &str,
    thread: &str,
    mut frame: Value,
    maximum: usize,
) -> anyhow::Result<String> {
    let thread = thread.to_string();
    store
        .edit(namespace, account, scope, move |editor| {
            finalize(editor, &mut frame, &thread);
            crate::responses_websocket_emulation::encode_frame_bounded(&frame, maximum)
                .map_err(|_| anyhow::anyhow!("observation frame is too large"))
        })
        .await
}

#[cfg(test)]
#[path = "tool_observation_state_tests.rs"]
mod tests;
