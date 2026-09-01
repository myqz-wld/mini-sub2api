use anyhow::Result;

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
use crate::request_state_types::ScopeState;
use crate::request_state_types::WireIdDomain;

#[derive(Clone, Copy)]
enum EntryKind {
    Installation,
    Conversation,
    ChildThread,
    Turn,
    GeneratedItem,
    CompactionMarker,
    WireId,
}

impl PersistedRequestState {
    pub(crate) fn prune(&mut self, day: i64, protected: &ProtectedStateKeys) -> Result<bool> {
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

    fn total_entries(&self, kind: EntryKind) -> usize {
        self.scopes
            .values()
            .map(|scope| entry_count(scope, kind))
            .sum()
    }

    fn entry_day(&self, kind: EntryKind, scope_key: &str, key: &str) -> i64 {
        let scope = self.scopes.get(scope_key).expect("candidate scope exists");
        match kind {
            EntryKind::Installation => scope.scoped_installations[key].last_seen_day,
            EntryKind::Conversation => scope.conversations[key].last_seen_day,
            EntryKind::ChildThread => scope.child_threads[key].last_seen_day,
            EntryKind::Turn => scope.turns[key].last_seen_day,
            EntryKind::GeneratedItem => scope.generated_items[key].last_seen_day,
            EntryKind::CompactionMarker => scope.compaction_markers[key].last_seen_day,
            EntryKind::WireId => scope.wire_ids[key].last_seen_day,
        }
    }

    fn oldest_entry(
        &self,
        kind: EntryKind,
        only_scope: Option<&str>,
        protected: &ProtectedStateKeys,
    ) -> Option<(String, String)> {
        self.scopes
            .iter()
            .filter(|(scope_key, _)| only_scope.is_none_or(|only| *scope_key == only))
            .flat_map(|(scope_key, scope)| {
                entry_keys(scope, kind)
                    .filter(move |key| self.removable(kind, scope_key, key, protected))
                    .map(move |key| {
                        (
                            self.entry_day(kind, scope_key, key),
                            scope_key.clone(),
                            key.to_string(),
                        )
                    })
            })
            .min()
            .map(|(_, scope, key)| (scope, key))
    }

    fn removable(
        &self,
        kind: EntryKind,
        scope_key: &str,
        key: &str,
        protected: &ProtectedStateKeys,
    ) -> bool {
        let pair = (scope_key.to_string(), key.to_string());
        let directly_protected = match kind {
            EntryKind::Installation => protected.installations.contains(&pair),
            EntryKind::Conversation => protected.conversations.contains(&pair),
            EntryKind::ChildThread => protected.child_threads.contains(&pair),
            EntryKind::Turn => protected.turns.contains(&pair),
            EntryKind::GeneratedItem => protected.generated_items.contains(&pair),
            EntryKind::CompactionMarker => protected.compaction_markers.contains(&pair),
            EntryKind::WireId => protected.wire_ids.contains(&pair),
        };
        if directly_protected {
            return false;
        }
        let scope = &self.scopes[scope_key];
        match kind {
            EntryKind::Conversation => {
                let id = &scope.conversations[key].id;
                !scope.child_threads.iter().any(|(child_key, child)| {
                    child.session_id == *id
                        && protected
                            .child_threads
                            .contains(&(scope_key.to_string(), child_key.clone()))
                }) && !protected_turn_for_thread(scope, scope_key, id, protected)
            }
            EntryKind::ChildThread => {
                let id = &scope.child_threads[key].id;
                !protected_turn_for_thread(scope, scope_key, id, protected)
            }
            EntryKind::Turn => {
                let id = &scope.turns[key].id;
                !scope.generated_items.iter().any(|(item_key, item)| {
                    item.turn_id.as_deref() == Some(id.as_str())
                        && protected
                            .generated_items
                            .contains(&(scope_key.to_string(), item_key.clone()))
                })
            }
            _ => true,
        }
    }

    fn remove_entry(&mut self, kind: EntryKind, scope_key: &str, key: &str) {
        let Some(scope) = self.scopes.get_mut(scope_key) else {
            return;
        };
        match kind {
            EntryKind::Installation => {
                if let Some(entry) = scope.scoped_installations.remove(key) {
                    remove_wire_pairs(scope, WireIdDomain::Installation, &[entry.id]);
                }
            }
            EntryKind::Conversation => {
                if let Some(entry) = scope.conversations.remove(key) {
                    remove_wire_pairs(
                        scope,
                        WireIdDomain::Session,
                        std::slice::from_ref(&entry.id),
                    );
                    remove_wire_pairs(scope, WireIdDomain::Thread, std::slice::from_ref(&entry.id));
                    remove_thread_graph(scope, &entry.id);
                    let child_ids = scope
                        .child_threads
                        .iter()
                        .filter(|(_, child)| child.session_id == entry.id)
                        .map(|(_, child)| child.id.clone())
                        .collect::<Vec<_>>();
                    scope
                        .child_threads
                        .retain(|_, child| child.session_id != entry.id);
                    for child_id in child_ids {
                        remove_wire_pairs(
                            scope,
                            WireIdDomain::Thread,
                            std::slice::from_ref(&child_id),
                        );
                        remove_thread_graph(scope, &child_id);
                    }
                }
            }
            EntryKind::ChildThread => {
                if let Some(entry) = scope.child_threads.remove(key) {
                    remove_wire_pairs(scope, WireIdDomain::Thread, std::slice::from_ref(&entry.id));
                    remove_thread_graph(scope, &entry.id);
                }
            }
            EntryKind::Turn => {
                if let Some(entry) = scope.turns.remove(key) {
                    clear_current_turn(scope, &entry.id);
                    remove_wire_pairs(scope, WireIdDomain::Turn, std::slice::from_ref(&entry.id));
                    let item_ids = scope
                        .generated_items
                        .values()
                        .filter(|item| item.turn_id.as_deref() == Some(entry.id.as_str()))
                        .map(|item| item.id.clone())
                        .collect::<Vec<_>>();
                    scope
                        .generated_items
                        .retain(|_, item| item.turn_id.as_deref() != Some(entry.id.as_str()));
                    remove_wire_pairs(scope, WireIdDomain::Item, &item_ids);
                }
            }
            EntryKind::GeneratedItem => {
                if let Some(entry) = scope.generated_items.remove(key) {
                    remove_wire_pairs(scope, WireIdDomain::Item, &[entry.id]);
                }
            }
            EntryKind::CompactionMarker => {
                scope.compaction_markers.remove(key);
            }
            EntryKind::WireId => {
                if let Some(entry) = scope.wire_ids.remove(key) {
                    scope.wire_upstream_index.remove(&entry.upstream_lookup);
                }
            }
        }
    }
}

fn entry_count(scope: &ScopeState, kind: EntryKind) -> usize {
    match kind {
        EntryKind::Installation => scope.scoped_installations.len(),
        EntryKind::Conversation => scope.conversations.len(),
        EntryKind::ChildThread => scope.child_threads.len(),
        EntryKind::Turn => scope.turns.len(),
        EntryKind::GeneratedItem => scope.generated_items.len(),
        EntryKind::CompactionMarker => scope.compaction_markers.len(),
        EntryKind::WireId => scope.wire_ids.len(),
    }
}

fn entry_keys(scope: &ScopeState, kind: EntryKind) -> impl Iterator<Item = &str> {
    let keys = match kind {
        EntryKind::Installation => scope.scoped_installations.keys().collect::<Vec<_>>(),
        EntryKind::Conversation => scope.conversations.keys().collect::<Vec<_>>(),
        EntryKind::ChildThread => scope.child_threads.keys().collect::<Vec<_>>(),
        EntryKind::Turn => scope.turns.keys().collect::<Vec<_>>(),
        EntryKind::GeneratedItem => scope.generated_items.keys().collect::<Vec<_>>(),
        EntryKind::CompactionMarker => scope.compaction_markers.keys().collect::<Vec<_>>(),
        EntryKind::WireId => scope.wire_ids.keys().collect::<Vec<_>>(),
    };
    keys.into_iter().map(String::as_str)
}

fn protected_turn_for_thread(
    scope: &ScopeState,
    scope_key: &str,
    thread_id: &str,
    protected: &ProtectedStateKeys,
) -> bool {
    scope.turns.iter().any(|(turn_key, turn)| {
        turn.thread_id == thread_id
            && protected
                .turns
                .contains(&(scope_key.to_string(), turn_key.clone()))
    })
}

fn remove_thread_graph(scope: &mut ScopeState, thread_id: &str) {
    let turn_ids = scope
        .turns
        .values()
        .filter(|turn| turn.thread_id == thread_id)
        .map(|turn| turn.id.clone())
        .collect::<Vec<_>>();
    let item_ids = scope
        .generated_items
        .values()
        .filter(|item| {
            item.turn_id
                .as_ref()
                .is_some_and(|id| turn_ids.contains(id))
        })
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    scope.turns.retain(|_, turn| turn.thread_id != thread_id);
    scope.generated_items.retain(|_, item| {
        item.turn_id
            .as_ref()
            .is_none_or(|id| !turn_ids.contains(id))
    });
    scope
        .compaction_markers
        .retain(|_, marker| marker.thread_id != thread_id);
    remove_wire_pairs(scope, WireIdDomain::Turn, &turn_ids);
    remove_wire_pairs(scope, WireIdDomain::Item, &item_ids);
}

fn clear_current_turn(scope: &mut ScopeState, turn_id: &str) {
    for entry in scope.conversations.values_mut() {
        if entry.current_turn_id.as_deref() == Some(turn_id) {
            entry.current_turn_id = None;
        }
    }
    for entry in scope.child_threads.values_mut() {
        if entry.current_turn_id.as_deref() == Some(turn_id) {
            entry.current_turn_id = None;
        }
    }
}

fn remove_wire_pairs(scope: &mut ScopeState, domain: WireIdDomain, upstream_ids: &[String]) {
    if upstream_ids.is_empty() {
        return;
    }
    let keys = scope
        .wire_ids
        .iter()
        .filter(|(_, entry)| entry.domain == domain && upstream_ids.contains(&entry.upstream_id))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        if let Some(entry) = scope.wire_ids.remove(&key) {
            scope.wire_upstream_index.remove(&entry.upstream_lookup);
        }
    }
}
