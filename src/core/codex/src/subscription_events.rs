use crate::subscription_context::{ContextStore, Operation, Pending};
use crate::subscription_index::{History, candidate_key, ids_compatible};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::time::Instant;

fn accepted_compaction(
    pending: &crate::request_compaction::PendingCompaction,
    response: &Value,
    active: &crate::subscription_context::Active,
) -> bool {
    pending.accepts_response(response, Some(&active.compaction_output))
}

impl ContextStore {
    pub(crate) fn accepts_compaction(
        &self,
        operation: Option<&Operation>,
        pending: &crate::request_compaction::PendingCompaction,
        event: &Value,
    ) -> anyhow::Result<bool> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        let active = operation.and_then(|operation| inner.operations.get(&operation.0.id));
        let response = event.get("response").unwrap_or(event);
        Ok(active.map_or_else(
            || pending.accepts_response(response, None),
            |active| accepted_compaction(pending, response, active),
        ))
    }

    /// Called after alias publication and before any corresponding downstream bytes are yielded.
    pub(crate) fn observe(
        &self,
        operation: &Operation,
        event: &Value,
        terminal: Option<bool>,
    ) -> anyhow::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        if event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
            && let Some(item) = event.get("item")
        {
            let extra = serde_json::to_vec(item)?
                .len()
                .saturating_mul(4)
                .saturating_add(128);
            if let Some(active) = inner.operations.get(&operation.0.id) {
                let (scope, session, available) = (
                    active.scope.clone(),
                    active.record.identity.session_id.clone(),
                    active.output_available,
                );
                let retain = available && inner.make_room(&self.limits, &scope, &session, extra);
                if let Some(active) = inner.operations.get_mut(&operation.0.id) {
                    if retain {
                        active.reserved += extra;
                        active.buffer_charge += extra;
                    } else {
                        active.output_available = false;
                        active.output.clear();
                        active.reserved = active.reserved.saturating_sub(active.buffer_charge);
                        active.buffer_charge = 0;
                    }
                }
            }
        }
        let Some(active) = inner.operations.get_mut(&operation.0.id) else {
            return Ok(());
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let response = if terminal.is_some() {
            event.get("response").unwrap_or(event)
        } else {
            event.get("response").unwrap_or(&Value::Null)
        };
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            crate::request_state_types::validate_wire_id(id)?;
            if let Some(existing) = &active.response_id {
                anyhow::ensure!(
                    existing == id,
                    "response ownership changed during inference"
                );
            } else {
                active.response_id = Some(id.into());
            }
        }
        if kind == "response.output_item.done"
            && let Some(item) = event.get("item")
        {
            active.compaction_output.observe(item);
            let index = event
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(active.observed_items.len());
            let encoded = serde_json::to_vec(item)?;
            let size = encoded.len();
            if index < self.limits.output_items {
                let fingerprint: [u8; 32] = Sha256::digest(&encoded).into();
                if let Some(previous) = active.observed_items.insert(index, fingerprint) {
                    anyhow::ensure!(
                        previous == fingerprint,
                        "output item changed at a completed index"
                    );
                } else {
                    active
                        .record
                        .dependencies
                        .append_output(std::slice::from_ref(item))
                        .map_err(|_| anyhow::anyhow!("invalid output dependencies"))?;
                }
            } else {
                active.dependencies_available = false;
            }
            if index >= self.limits.output_items
                || active.output_bytes.saturating_add(size) > self.limits.output_bytes
            {
                active.output_available = false;
                active.output.clear();
            } else if active.output_available {
                if let Some(previous) = active.output.insert(index, item.clone()) {
                    active.output_bytes = active
                        .output_bytes
                        .saturating_sub(serde_json::to_vec(&previous)?.len());
                }
                active.output_bytes += size;
            }
        }
        let completed = terminal
            .or(match kind {
                "response.completed" => Some(true),
                "response.failed" | "response.incomplete" | "error" => Some(false),
                _ => None,
            })
            .map(|completed| {
                completed
                    && active
                        .record
                        .compaction
                        .as_ref()
                        .is_none_or(|pending| accepted_compaction(pending, response, active))
            });
        if completed.is_none() {
            if kind == "response.created"
                && let Some(id) = &active.response_id
            {
                let mut record = active.record.clone();
                record.history = None;
                record.settings = None;
                let (scope, id) = (active.scope.clone(), id.clone());
                inner
                    .scopes
                    .entry(scope)
                    .or_default()
                    .records
                    .insert(id, record);
            }
            return Ok(());
        }
        let mut active = inner
            .operations
            .remove(&operation.0.id)
            .expect("active operation");
        let Some(id) = active.response_id else {
            return Ok(());
        };
        if completed != Some(true) {
            if let Some(scope) = inner.scopes.get_mut(&active.scope) {
                scope.records.remove(&id);
            }
            return Ok(());
        }
        let output: Cow<'_, [Value]> =
            if let Some(output) = response.get("output").and_then(Value::as_array) {
                for (index, item) in &active.output {
                    if output.get(*index).is_none_or(|final_item| {
                        candidate_key(item) != candidate_key(final_item)
                            || !ids_compatible(item, final_item)
                    }) {
                        active.output_available = false;
                        active.dependencies_available = false;
                    }
                }
                Cow::Borrowed(output)
            } else if active
                .observed_items
                .keys()
                .copied()
                .eq(0..active.observed_items.len())
            {
                Cow::Owned(active.output.into_values().collect())
            } else {
                active.output_available = false;
                active.dependencies_available = false;
                Cow::Owned(Vec::new())
            };
        let output_bytes = serde_json::to_vec(&output)?.len();
        active.output_available &=
            output.len() <= self.limits.output_items && output_bytes <= self.limits.output_bytes;
        // Dependencies are small routing facts. Never publish a partial set as usable state.
        if output.len() > self.limits.output_items || !active.dependencies_available {
            // Delivery can finish, but no partial reference/dependency set is advertised as usable.
            if let Some(scope) = inner.scopes.get_mut(&active.scope) {
                scope.records.remove(&id);
                scope.rebuild_index();
            }
            return Ok(());
        }
        active
            .record
            .dependencies
            .append_output(&output)
            .map_err(|_| anyhow::anyhow!("invalid output dependencies"))?;
        if let Some(compaction) = &active.record.compaction {
            active.record.identity.window_number = compaction.target_window;
        }
        active.record.completed = true;
        active.record.last_used = Instant::now();
        let session = active.record.identity.session_id.clone();
        let retain_cost = output_bytes
            .saturating_mul(6)
            .saturating_add(active.record.descriptor_cost());
        // Native compaction also depends on client retention/truncation choices. Keep the actual
        // item untouched and require that client-supplied replacement history before HTTP expansion.
        let compaction = active.record.identity.request_kind == "compaction"
            || output
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction"));
        // The operation was taken out for terminal assembly. Keep its capacity and session pinned
        // until publication, so pressure cannot prune its created record/aliases or ignore assembly.
        inner.reservations.insert(
            operation.0.id.clone(),
            Pending {
                scope: active.scope.clone(),
                session: session.clone(),
                reserved: active.reserved,
            },
        );
        let can_retain = !compaction
            && active.output_available
            && inner.make_room(&self.limits, &active.scope, &session, retain_cost);
        let scope = inner.scopes.entry(active.scope.clone()).or_default();
        if can_retain {
            if let Some(parent) = active.record.history.take() {
                let items = output
                    .iter()
                    .cloned()
                    .map(|item| scope.interner.intern(item))
                    .collect();
                active.record.history = Some(History::extend(Some(parent), items));
            }
        } else {
            active.record.history = None;
            active.record.settings = None;
        }
        if let Some(session) = scope.sessions.get_mut(&session) {
            session.last_business = Instant::now();
        }
        let scope_key = active.scope.clone();
        scope.records.insert(id.clone(), active.record);
        scope.rebuild_index();
        scope.interner.sweep();
        inner.reservations.remove(&operation.0.id);
        if !inner.fits(&self.limits, &scope_key, &session, 0) {
            let scope = inner.scopes.get_mut(&scope_key).expect("published scope");
            if let Some(record) = scope.records.get_mut(&id) {
                record.history = None;
                record.settings = None;
            }
            scope.rebuild_index();
            scope.interner.sweep();
        }
        Ok(())
    }
}
