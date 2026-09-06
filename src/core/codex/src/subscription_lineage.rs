//! Lightweight effective-history ownership survives bulk body expiry for explicit WS references.
use super::ContextPlan;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct HistoryLineage {
    pub(crate) turns: Arc<BTreeSet<String>>,
    pub(crate) cost: usize,
}

impl HistoryLineage {
    fn insert(&mut self, turn: &str) {
        if !turn.is_empty() && !self.turns.contains(turn) {
            self.cost = self.cost.saturating_add(turn.len().saturating_add(64));
            Arc::make_mut(&mut self.turns).insert(turn.to_string());
        }
    }
}

impl ContextPlan {
    pub(crate) fn inherit_fork_source(
        &self,
        editor: &mut RequestStateEditor<'_>,
        evidence: &mut crate::request_identity_evidence::RequestIdentityEvidence,
    ) -> anyhow::Result<()> {
        if evidence.forked_from_thread.is_some() || self.evidence.previous.is_none() {
            return Ok(());
        }
        let Some(base) = &self.baseline else {
            return Ok(());
        };
        let Some(source) = &base.identity.forked_from_thread_id else {
            return Ok(());
        };
        let mut same_thread = !evidence.explicit_thread_lineage;
        if let Some(thread) = &evidence.thread {
            same_thread |= thread == &base.identity.thread_id
                || editor
                    .existing_wire_from_downstream(
                        crate::request_state_types::WireIdDomain::Thread,
                        thread,
                    )?
                    .as_ref()
                    == Some(&base.identity.thread_id);
        }
        if same_thread {
            // A previous-only continuation restores the referenced thread's known provenance.
            // A new explicit branch must provide its own eligible source relationship.
            evidence.forked_from_thread = Some(source.clone());
        }
        Ok(())
    }

    pub(crate) fn history_lineage(
        &self,
        editor: &mut RequestStateEditor<'_>,
        identity: &ResolvedRequestIdentity,
        object: &Map<String, Value>,
    ) -> anyhow::Result<HistoryLineage> {
        let baseline = self.evidence.previous.as_ref().and(self.baseline.as_ref());
        let mut lineage = HistoryLineage::default();
        if let Some(baseline) = baseline {
            anyhow::ensure!(
                editor.history_thread_allowed(&baseline.identity.thread_id, identity),
                "previous response belongs to an unrelated historical thread"
            );
            // Validate all effective history, including a prior response's explicitly copied fork
            // source. The response owner alone cannot establish eligibility of that earlier history.
            for turn in baseline.lineage.turns.iter() {
                if let Some((_, known)) = editor.turn_by_id(turn) {
                    anyhow::ensure!(
                        editor.history_thread_allowed(&known.thread_id, identity),
                        "previous response history belongs to an unrelated thread"
                    );
                }
            }
            lineage = baseline.lineage.clone();
        }
        if let Some(items) = object.get("input").and_then(Value::as_array) {
            for turn in items.iter().filter_map(|item| {
                item.get("internal_chat_message_metadata_passthrough")?
                    .get("turn_id")?
                    .as_str()
            }) {
                lineage.insert(turn);
            }
        }
        if let Some(turn) = identity.turn_id.as_deref() {
            lineage.insert(turn);
        }
        Ok(lineage)
    }
}
