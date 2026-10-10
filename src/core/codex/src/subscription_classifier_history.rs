//! Completed classifier transcripts can outlive their transport lease, within one source.
use super::ContextPlan;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;

impl ContextPlan {
    pub(crate) fn classifier_history_source_allowed(
        &self,
        editor: &RequestStateEditor<'_>,
        turn: &str,
        target: &ResolvedRequestIdentity,
    ) -> bool {
        let Some(source) = target.parent_thread_id.as_deref() else {
            return false;
        };
        let Some(base) = &self.baseline else {
            return false;
        };
        // plan() selects only a completed, non-conflicting full prefix in this Key/account's
        // ContextStore scope and protects its retained session with an admission reservation.
        // A durable alias or caller role marker alone is never completion/body evidence.
        target.request_kind == "guardian_classifier"
            && self.evidence.previous.is_none()
            && self.checkpoint.is_none()
            && base.completed
            && base.history.as_ref().is_some_and(|history| history.len > 0)
            && base.identity.request_kind == "guardian_classifier"
            && base.identity.session_id == target.session_id
            && base.identity.parent_thread_id.as_deref() == Some(source)
            && base.lineage.turns.contains(turn)
            && editor.classifier_history_origin(turn, &target.session_id, source)
    }

    pub(crate) fn classifier_history_item_allowed(
        &self,
        editor: &mut RequestStateEditor<'_>,
        turn: &str,
        target: &ResolvedRequestIdentity,
        index: Option<usize>,
    ) -> anyhow::Result<bool> {
        if !self.classifier_history_source_allowed(editor, turn, target) {
            return Ok(false);
        }
        let Some(index) = index else {
            return Ok(false);
        };
        let history = self
            .baseline
            .as_ref()
            .and_then(|base| base.history.as_ref())
            .unwrap();
        let Some(saved) = history.get(index) else {
            return Ok(false);
        };
        // History matching intentionally ignores some transport metadata. Require the saved
        // occurrence's turn too, so an appended/retagged item cannot borrow a completed turn.
        let Some(raw) = saved
            .value
            .get("internal_chat_message_metadata_passthrough")
            .and_then(|metadata| metadata.get("turn_id"))
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(false);
        };
        Ok(raw == turn
            || editor
                .existing_wire_from_downstream(WireIdDomain::Turn, raw)?
                .as_deref()
                == Some(turn))
    }
}
