//! Protect only the identity graph required by admitted or retained Subscription contexts.
use crate::request_state_editor::ProtectedStateKeys;
use crate::request_state_types::{PersistedRequestState, WireIdDomain};
use crate::subscription_context::ContextStore;
use std::collections::HashSet;

impl ContextStore {
    pub(crate) fn protect_aliases(
        &self,
        state: &PersistedRequestState,
        protected: &mut ProtectedStateKeys,
    ) -> anyhow::Result<()> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("context state unavailable"))?;
        for (key, scope) in &state.scopes {
            let retained = inner
                .scopes
                .get(key)
                .into_iter()
                .flat_map(|s| s.records.values())
                .filter(|r| r.history.is_some() || r.socket.is_some());
            let active = inner
                .operations
                .values()
                .filter(|op| op.scope == *key)
                .map(|op| &op.record);
            let records: Vec<_> = retained.chain(active).collect();
            let mut sessions: HashSet<_> = records
                .iter()
                .map(|r| r.identity.session_id.as_str())
                .chain(
                    inner
                        .reservations
                        .values()
                        .filter(|p| p.scope == *key)
                        .map(|p| p.session.as_str()),
                )
                .collect();
            if sessions.is_empty() {
                continue;
            }
            protected.scopes.insert(key.clone());
            let turns: HashSet<_> = records
                .iter()
                .filter_map(|r| r.identity.turn_id.as_deref())
                .chain(
                    records
                        .iter()
                        .flat_map(|r| r.lineage.turns.iter().map(String::as_str)),
                )
                .collect();
            // A retained fork can depend on history owned by another root in this same Key scope.
            // Protect those owners as well as their turn aliases while the live reference is valid.
            for turn in scope
                .turns
                .values()
                .filter(|turn| turns.contains(turn.id.as_str()))
            {
                let session = scope
                    .child_threads
                    .values()
                    .find(|thread| thread.id == turn.thread_id)
                    .map_or(turn.thread_id.as_str(), |thread| thread.session_id.as_str());
                sessions.insert(session);
            }
            let mut ids: HashSet<&str> = records
                .iter()
                .flat_map(|r| {
                    r.dependencies
                        .items
                        .iter()
                        .chain(r.dependencies.calls.keys())
                })
                .map(String::as_str)
                .collect();
            ids.extend(sessions.iter().copied());
            ids.extend(turns.iter().copied());
            ids.extend(records.iter().map(|r| r.identity.thread_id.as_str()));
            for (lookup, pair) in &scope.wire_ids {
                if ids.contains(pair.upstream_id.as_str())
                    || ids.contains(pair.downstream_id.as_str())
                    || pair
                        .owner
                        .as_ref()
                        .is_some_and(|owner| sessions.contains(owner.session_id.as_str()))
                    || matches!(
                        pair.domain,
                        WireIdDomain::Installation | WireIdDomain::ContextWindow
                    )
                {
                    protected.wire_ids.insert((key.clone(), lookup.clone()));
                }
            }
            for (lookup, entry) in &scope.conversations {
                if sessions.contains(entry.id.as_str()) {
                    protected
                        .conversations
                        .insert((key.clone(), lookup.clone()));
                    if let Some(turn) = entry.current_turn_id.as_deref() {
                        for (lookup, entry) in &scope.turns {
                            if entry.id == turn {
                                protected.turns.insert((key.clone(), lookup.clone()));
                            }
                        }
                    }
                }
            }
            for (lookup, entry) in &scope.child_threads {
                if sessions.contains(entry.session_id.as_str()) {
                    protected
                        .child_threads
                        .insert((key.clone(), lookup.clone()));
                }
            }
            for (lookup, entry) in &scope.turns {
                if turns.contains(entry.id.as_str()) {
                    protected.turns.insert((key.clone(), lookup.clone()));
                }
            }
            for (lookup, entry) in &scope.generated_items {
                if entry
                    .turn_id
                    .as_deref()
                    .is_some_and(|id| turns.contains(id))
                {
                    protected
                        .generated_items
                        .insert((key.clone(), lookup.clone()));
                }
            }
            for lookup in scope.scoped_installations.keys() {
                protected
                    .installations
                    .insert((key.clone(), lookup.clone()));
            }
        }
        Ok(())
    }
}
