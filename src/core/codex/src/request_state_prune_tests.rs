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
    let (root_id, parent_thread_id, child_thread_id, root_turn_id, parent_turn_id, child_turn_id) = {
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
        scope
            .turns
            .values()
            .all(|turn| { ![&root_turn_id, &parent_turn_id, &child_turn_id].contains(&&turn.id) })
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
