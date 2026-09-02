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
                !protected_conversation_graph(scope, scope_key, id, protected)
            }
            EntryKind::ChildThread => {
                let id = &scope.child_threads[key].id;
                !protected_thread_graph(scope, scope_key, id, protected)
            }
            EntryKind::Turn => {
                let id = &scope.turns[key].id;
                !protected_turn_graph(scope, scope_key, id, protected)
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
                    remove_conversation_graph(scope, &entry.id);
                }
            }
            EntryKind::ChildThread => {
                if let Some(entry) = scope.child_threads.get(key) {
                    let root = entry.id.clone();
                    remove_child_thread_graph(scope, &root);
                }
            }
            EntryKind::Turn => {
                if let Some(entry) = scope.turns.get(key) {
                    let root = entry.id.clone();
                    remove_turn_graph(scope, &root);
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

fn protected_conversation_graph(
    scope: &ScopeState,
    scope_key: &str,
    session_id: &str,
    protected: &ProtectedStateKeys,
) -> bool {
    let thread_ids = std::iter::once(session_id.to_string())
        .chain(
            scope
                .child_threads
                .values()
                .filter(|thread| thread.session_id == session_id)
                .map(|thread| thread.id.clone()),
        )
        .collect::<Vec<_>>();
    protected_thread_ids(scope, scope_key, &thread_ids, protected)
        || protected_owned_response(scope, scope_key, session_id, None, protected)
}

fn protected_thread_graph(
    scope: &ScopeState,
    scope_key: &str,
    thread_id: &str,
    protected: &ProtectedStateKeys,
) -> bool {
    let thread_ids = descendant_thread_ids(scope, thread_id);
    protected_thread_ids(scope, scope_key, &thread_ids, protected)
        || thread_ids
            .iter()
            .any(|id| protected_owned_response(scope, scope_key, "", Some(id), protected))
}

fn protected_thread_ids(
    scope: &ScopeState,
    scope_key: &str,
    thread_ids: &[String],
    protected: &ProtectedStateKeys,
) -> bool {
    scope.child_threads.iter().any(|(key, thread)| {
        thread_ids.contains(&thread.id)
            && protected
                .child_threads
                .contains(&(scope_key.to_string(), key.clone()))
    }) || scope.turns.iter().any(|(key, turn)| {
        thread_ids.contains(&turn.thread_id)
            && protected
                .turns
                .contains(&(scope_key.to_string(), key.clone()))
    }) || scope.generated_items.iter().any(|(key, item)| {
        item.turn_id.as_ref().is_some_and(|turn_id| {
            scope
                .turns
                .values()
                .any(|turn| turn.id == *turn_id && thread_ids.contains(&turn.thread_id))
        }) && protected
            .generated_items
            .contains(&(scope_key.to_string(), key.clone()))
    })
}

fn protected_turn_graph(
    scope: &ScopeState,
    scope_key: &str,
    turn_id: &str,
    protected: &ProtectedStateKeys,
) -> bool {
    let turn_ids = descendant_turn_ids(scope, turn_id);
    scope.turns.iter().any(|(key, turn)| {
        turn_ids.contains(&turn.id)
            && protected
                .turns
                .contains(&(scope_key.to_string(), key.clone()))
    }) || scope.generated_items.iter().any(|(key, item)| {
        item.turn_id
            .as_ref()
            .is_some_and(|id| turn_ids.contains(id))
            && protected
                .generated_items
                .contains(&(scope_key.to_string(), key.clone()))
    })
}

fn protected_owned_response(
    scope: &ScopeState,
    scope_key: &str,
    session_id: &str,
    thread_id: Option<&str>,
    protected: &ProtectedStateKeys,
) -> bool {
    scope.wire_ids.iter().any(|(key, entry)| {
        entry.owner.as_ref().is_some_and(|owner| {
            (session_id.is_empty() || owner.session_id == session_id)
                && thread_id.is_none_or(|thread| owner.thread_id == thread)
        }) && protected
            .wire_ids
            .contains(&(scope_key.to_string(), key.clone()))
    })
}

fn remove_conversation_graph(scope: &mut ScopeState, session_id: &str) {
    let thread_ids = std::iter::once(session_id.to_string())
        .chain(
            scope
                .child_threads
                .values()
                .filter(|thread| thread.session_id == session_id)
                .map(|thread| thread.id.clone()),
        )
        .collect::<Vec<_>>();
    scope
        .child_threads
        .retain(|_, thread| thread.session_id != session_id);
    remove_wire_pairs(scope, WireIdDomain::Session, &[session_id.to_string()]);
    remove_wire_pairs(scope, WireIdDomain::Thread, &thread_ids);
    for thread_id in &thread_ids {
        remove_thread_graph(scope, thread_id);
    }
    remove_owned_response_pairs(scope, |owner| owner.session_id == session_id);
}

fn remove_child_thread_graph(scope: &mut ScopeState, thread_id: &str) {
    let thread_ids = descendant_thread_ids(scope, thread_id);
    scope
        .child_threads
        .retain(|_, thread| !thread_ids.contains(&thread.id));
    remove_wire_pairs(scope, WireIdDomain::Thread, &thread_ids);
    for id in &thread_ids {
        remove_thread_graph(scope, id);
    }
    remove_owned_response_pairs(scope, |owner| thread_ids.contains(&owner.thread_id));
}

fn descendant_thread_ids(scope: &ScopeState, root: &str) -> Vec<String> {
    let mut ids = vec![root.to_string()];
    loop {
        let mut changed = false;
        for child in scope.child_threads.values() {
            if child
                .parent_thread_id
                .as_ref()
                .is_some_and(|parent| ids.contains(parent))
                && !ids.contains(&child.id)
            {
                ids.push(child.id.clone());
                changed = true;
            }
        }
        if !changed {
            return ids;
        }
    }
}

fn descendant_turn_ids(scope: &ScopeState, root: &str) -> Vec<String> {
    let mut ids = vec![root.to_string()];
    loop {
        let mut changed = false;
        for turn in scope.turns.values() {
            if (turn.root_turn_id == root
                || turn
                    .parent_turn_id
                    .as_ref()
                    .is_some_and(|parent| ids.contains(parent)))
                && !ids.contains(&turn.id)
            {
                ids.push(turn.id.clone());
                changed = true;
            }
        }
        if !changed {
            return ids;
        }
    }
}

fn remove_turn_graph(scope: &mut ScopeState, turn_id: &str) {
    let turn_ids = descendant_turn_ids(scope, turn_id);
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
    for id in &turn_ids {
        clear_current_turn(scope, id);
    }
    scope.turns.retain(|_, turn| !turn_ids.contains(&turn.id));
    scope.generated_items.retain(|_, item| {
        item.turn_id
            .as_ref()
            .is_none_or(|id| !turn_ids.contains(id))
    });
    remove_wire_pairs(scope, WireIdDomain::Turn, &turn_ids);
    remove_wire_pairs(scope, WireIdDomain::Item, &item_ids);
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

fn remove_owned_response_pairs(
    scope: &mut ScopeState,
    predicate: impl Fn(&crate::request_state_types::WireIdOwner) -> bool,
) {
    let keys = scope
        .wire_ids
        .iter()
        .filter(|(_, entry)| entry.owner.as_ref().is_some_and(&predicate))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        if let Some(entry) = scope.wire_ids.remove(&key) {
            scope.wire_upstream_index.remove(&entry.upstream_lookup);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_state_editor::RequestStateEditor;
    use crate::request_state_lookup::LookupKeyFactory;
    use std::collections::BTreeSet;

    #[test]
    fn protected_descendants_retain_ancestors_and_cascade_removal_never_dangles() {
        let owner = "acct_prune_graph";
        let namespace = "prune-graph-namespace";
        let scope_raw = "prune-graph-scope";
        let keys = LookupKeyFactory::new(namespace, scope_raw);
        let scope_key = keys.scope_key();
        let root_key = keys.identity("conversation", "root");
        let parent_thread_key = keys.identity("thread", "parent");
        let child_thread_key = keys.identity("thread", "child");
        let root_turn_key = keys.identity("turn", "root");
        let parent_turn_key = keys.identity("turn", "parent");
        let child_turn_key = keys.identity("turn", "child");
        let mut state = PersistedRequestState::new(BTreeSet::from([owner.to_string()]));
        let (
            root_id,
            parent_thread_id,
            child_thread_id,
            root_turn_id,
            parent_turn_id,
            child_turn_id,
        ) = {
            let mut editor =
                RequestStateEditor::new(&mut state, keys, owner, 1, 86_400_000).expect("editor");
            let root = editor.conversation(&root_key).expect("root");
            let parent = editor
                .child_thread(&parent_thread_key, &root.id, Some(&root.id))
                .expect("parent thread");
            let child = editor
                .child_thread(&child_thread_key, &root.id, Some(&parent.id))
                .expect("child thread");
            let root_turn = editor
                .turn(&root_turn_key, &root.id, None, None)
                .expect("root turn");
            let parent_turn = editor
                .turn(
                    &parent_turn_key,
                    &parent.id,
                    Some(&root_turn.id),
                    Some(&root_turn.id),
                )
                .expect("parent turn");
            let child_turn = editor
                .turn(
                    &child_turn_key,
                    &child.id,
                    Some(&root_turn.id),
                    Some(&parent_turn.id),
                )
                .expect("child turn");
            (
                root.id,
                parent.id,
                child.id,
                root_turn.id,
                parent_turn.id,
                child_turn.id,
            )
        };
        state.validate().expect("valid graph");

        let mut protected = ProtectedStateKeys::default();
        protected
            .child_threads
            .insert((scope_key.clone(), child_thread_key.clone()));
        protected
            .turns
            .insert((scope_key.clone(), child_turn_key.clone()));
        assert!(!state.removable(
            EntryKind::ChildThread,
            &scope_key,
            &parent_thread_key,
            &protected,
        ));
        assert!(!state.removable(EntryKind::Turn, &scope_key, &root_turn_key, &protected,));

        state.remove_entry(EntryKind::Turn, &scope_key, &root_turn_key);
        let scope = &state.scopes[&scope_key];
        assert!(
            scope.turns.values().all(|turn| {
                ![&root_turn_id, &parent_turn_id, &child_turn_id].contains(&&turn.id)
            })
        );
        assert!(
            scope
                .conversations
                .values()
                .any(|entry| entry.id == root_id)
        );
        state.validate().expect("valid after turn cascade");

        state.remove_entry(EntryKind::ChildThread, &scope_key, &parent_thread_key);
        let scope = &state.scopes[&scope_key];
        assert!(
            scope
                .child_threads
                .values()
                .all(|thread| { thread.id != parent_thread_id && thread.id != child_thread_id })
        );
        state.validate().expect("valid after thread cascade");
    }
}
