//! A classifier turn is logical inference state; its thread is a transport lease.
use super::{RequestStateEditor, TurnAssignment};
use anyhow::Result;

impl RequestStateEditor<'_> {
    pub(crate) fn classifier_turn_with_id(
        &mut self,
        key: &str,
        thread_id: &str,
        root_turn_id: Option<&str>,
        parent_turn_id: Option<&str>,
        reserved_id: Option<&str>,
    ) -> Result<TurnAssignment> {
        let (_, lease) = self
            .child_thread_by_id(thread_id)
            .ok_or_else(|| anyhow::anyhow!("classifier lease is not a child thread"))?;
        let source = lease
            .parent_thread_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("classifier source is missing"))?;
        if let Some(parent) = parent_turn_id {
            let (_, parent) = self
                .turn_by_id(parent)
                .ok_or_else(|| anyhow::anyhow!("classifier parent is missing"))?;
            anyhow::ensure!(
                parent.thread_id == source,
                "classifier parent source changed"
            );
        }
        if self
            .scope()
            .turns
            .get(key)
            .is_some_and(|turn| turn.classifier)
        {
            let known = self.existing_turn(key).expect("known classifier turn");
            let (_, owner) = self
                .child_thread_by_id(&known.thread_id)
                .ok_or_else(|| anyhow::anyhow!("classifier canonical owner is missing"))?;
            anyhow::ensure!(
                owner.session_id == lease.session_id
                    && owner.parent_thread_id.as_deref() == Some(source)
                    && known.root_turn_id == root_turn_id.unwrap_or(&known.id)
                    && known.parent_turn_id.as_deref() == parent_turn_id
                    && reserved_id.is_none_or(|id| known.id == id),
                "classifier retry ancestry changed"
            );
            return Ok(known);
        }
        // Old, untagged state can acquire provenance only without changing its owner.
        // Ambiguous legacy misparenting remains a rejected relationship, never a reparent.
        let turn = self.turn_with_id(key, thread_id, root_turn_id, parent_turn_id, reserved_id)?;
        self.scope_mut()
            .turns
            .get_mut(key)
            .expect("admitted turn")
            .classifier = true;
        self.changed = true;
        Ok(turn)
    }
}

#[cfg(test)]
#[path = "request_state_classifier_tests.rs"]
mod tests;
