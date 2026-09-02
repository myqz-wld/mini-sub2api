use super::EntryKind;
use super::entry_count;
use crate::request_state_editor::ProtectedStateKeys;
use crate::request_state_types::DETAIL_RETENTION_DAYS;
use crate::request_state_types::MAX_CHILD_THREADS;
use crate::request_state_types::MAX_COMPACTION_MARKERS;
use crate::request_state_types::MAX_CONVERSATIONS;
use crate::request_state_types::MAX_GENERATED_ITEMS;
use crate::request_state_types::MAX_SCOPE_CHILD_THREADS;
use crate::request_state_types::MAX_SCOPE_COMPACTION_MARKERS;
use crate::request_state_types::MAX_SCOPE_CONVERSATIONS;
use crate::request_state_types::MAX_SCOPE_GENERATED_ITEMS;
use crate::request_state_types::MAX_SCOPE_INSTALLATIONS;
use crate::request_state_types::MAX_SCOPE_TURNS;
use crate::request_state_types::MAX_SCOPE_WIRE_ID_PAIRS;
use crate::request_state_types::MAX_SCOPED_INSTALLATIONS;
use crate::request_state_types::MAX_SCOPES;
use crate::request_state_types::MAX_TURNS;
use crate::request_state_types::MAX_WIRE_ID_PAIRS;
use crate::request_state_types::PersistedRequestState;

impl PersistedRequestState {
    pub(crate) fn prune(
        &mut self,
        day: i64,
        protected: &ProtectedStateKeys,
    ) -> anyhow::Result<bool> {
        let mut changed = self.prune_expired(day, protected);
        for (kind, maximum) in [
            (EntryKind::Installation, MAX_SCOPE_INSTALLATIONS),
            (EntryKind::Conversation, MAX_SCOPE_CONVERSATIONS),
            (EntryKind::ChildThread, MAX_SCOPE_CHILD_THREADS),
            (EntryKind::Turn, MAX_SCOPE_TURNS),
            (EntryKind::GeneratedItem, MAX_SCOPE_GENERATED_ITEMS),
            (EntryKind::CompactionMarker, MAX_SCOPE_COMPACTION_MARKERS),
            (EntryKind::WireId, MAX_SCOPE_WIRE_ID_PAIRS),
        ] {
            let scope_keys = self.scopes.keys().cloned().collect::<Vec<_>>();
            for scope_key in scope_keys {
                while self
                    .scopes
                    .get(&scope_key)
                    .is_some_and(|scope| entry_count(scope, kind) > maximum)
                {
                    let Some((_, key)) = self.oldest_entry(kind, Some(&scope_key), protected)
                    else {
                        anyhow::bail!("request state per-scope capacity is protected");
                    };
                    self.remove_entry(kind, &scope_key, &key);
                    changed = true;
                }
            }
        }
        for (kind, maximum) in [
            (EntryKind::Installation, MAX_SCOPED_INSTALLATIONS),
            (EntryKind::Conversation, MAX_CONVERSATIONS),
            (EntryKind::ChildThread, MAX_CHILD_THREADS),
            (EntryKind::Turn, MAX_TURNS),
            (EntryKind::GeneratedItem, MAX_GENERATED_ITEMS),
            (EntryKind::CompactionMarker, MAX_COMPACTION_MARKERS),
            (EntryKind::WireId, MAX_WIRE_ID_PAIRS),
        ] {
            while self.total_entries(kind) > maximum {
                let Some((scope_key, key)) = self.oldest_entry(kind, None, protected) else {
                    anyhow::bail!("request state global capacity is protected");
                };
                self.remove_entry(kind, &scope_key, &key);
                changed = true;
            }
        }
        while self.scopes.len() > MAX_SCOPES {
            let candidate = self
                .scopes
                .iter()
                .filter(|(key, _)| !protected.scopes.contains(*key))
                .min_by_key(|(key, scope)| (scope.last_seen_day, (*key).clone()))
                .map(|(key, _)| key.clone());
            let Some(candidate) = candidate else {
                anyhow::bail!("request state scope capacity is protected");
            };
            self.scopes.remove(&candidate);
            changed = true;
        }
        Ok(changed)
    }

    pub(crate) fn evict_one(&mut self, protected: &ProtectedStateKeys) -> bool {
        for kind in [
            EntryKind::WireId,
            EntryKind::GeneratedItem,
            EntryKind::CompactionMarker,
            EntryKind::Turn,
            EntryKind::ChildThread,
            EntryKind::Conversation,
            EntryKind::Installation,
        ] {
            if let Some((scope_key, key)) = self.oldest_entry(kind, None, protected) {
                self.remove_entry(kind, &scope_key, &key);
                return true;
            }
        }
        let candidate = self
            .scopes
            .iter()
            .filter(|(key, _)| !protected.scopes.contains(*key))
            .min_by_key(|(key, scope)| (scope.last_seen_day, (*key).clone()))
            .map(|(key, _)| key.clone());
        candidate.is_some_and(|key| self.scopes.remove(&key).is_some())
    }

    fn prune_expired(&mut self, day: i64, protected: &ProtectedStateKeys) -> bool {
        let cutoff = day.saturating_sub(DETAIL_RETENTION_DAYS);
        let mut changed = false;
        for kind in [
            EntryKind::WireId,
            EntryKind::GeneratedItem,
            EntryKind::CompactionMarker,
            EntryKind::Turn,
        ] {
            loop {
                let candidate = self
                    .oldest_entry(kind, None, protected)
                    .filter(|(scope, key)| self.entry_day(kind, scope, key) < cutoff);
                let Some((scope, key)) = candidate else {
                    break;
                };
                self.remove_entry(kind, &scope, &key);
                changed = true;
            }
        }
        changed
    }
}
