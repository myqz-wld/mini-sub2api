use super::turn_key_for_raw;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_state_editor::{RequestStateEditor, TurnAssignment};
use crate::request_state_types::WireIdDomain;
use anyhow::Result;

pub(super) struct ResolvedTurn {
    pub(super) turn_id: Option<String>,
    pub(super) root_turn_id: Option<String>,
    pub(super) parent_turn_id: Option<String>,
    pub(super) started_at_unix_ms: Option<i64>,
}

pub(super) fn resolve_turn(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    turn_key: &str,
    thread_id: &str,
    session_id: &str,
    reserved_turn: Option<&str>,
) -> Result<ResolvedTurn> {
    if evidence.is_memory() && evidence.turn.is_none() {
        return Ok(ResolvedTurn {
            turn_id: None,
            root_turn_id: None,
            parent_turn_id: None,
            started_at_unix_ms: None,
        });
    }
    if evidence.is_prewarm() {
        return Ok(ResolvedTurn {
            turn_id: Some(String::new()),
            root_turn_id: None,
            parent_turn_id: None,
            started_at_unix_ms: None,
        });
    }
    let child_lineage = evidence.parent_turn.is_some()
        || (evidence.explicit_thread_lineage && evidence.root_turn.is_some());
    if !child_lineage {
        let turn = admit_turn(
            editor,
            evidence,
            turn_key,
            thread_id,
            None,
            None,
            reserved_turn,
        )?;
        return Ok(resolved(turn));
    }

    // Guardian root metadata is optional. Recover known ancestry without assuming its
    // root was started on the session's root thread; a child can own an independent task.
    let inherited_root = if (evidence.is_classifier() || evidence.is_reviewer())
        && evidence.root_turn.is_none()
        && let Some(parent) = evidence.parent_turn.as_deref()
    {
        let key = turn_key_for_raw(editor, parent)?;
        editor.existing_turn(&key).map(|turn| turn.root_turn_id)
    } else {
        None
    };
    let root_raw = evidence
        .root_turn
        .as_deref()
        .or(inherited_root.as_deref())
        .or(evidence.parent_turn.as_deref())
        .unwrap_or(turn_key);
    let root_key = turn_key_for_raw(editor, root_raw)?;
    let root_alias = editor.existing_wire_from_downstream(WireIdDomain::Turn, root_raw)?;
    if turn_key == root_key {
        anyhow::ensure!(
            evidence.parent_turn.is_none(),
            "self-rooted turn has a parent"
        );
        return Ok(resolved(admit_turn(
            editor,
            evidence,
            turn_key,
            thread_id,
            None,
            None,
            reserved_turn,
        )?));
    }
    let root = if let Some(known) = editor.existing_turn(&root_key) {
        anyhow::ensure!(
            known.id == known.root_turn_id
                && known.parent_turn_id.is_none()
                && editor.thread_is_ancestor(&known.thread_id, thread_id),
            "root turn belongs to another lineage"
        );
        known
    } else {
        editor.turn_with_id(&root_key, session_id, None, None, root_alias.as_deref())?
    };
    let parent = evidence
        .parent_turn
        .as_deref()
        .map(|raw| {
            let key = turn_key_for_raw(editor, raw)?;
            if key == root_key {
                Ok(root.id.clone())
            } else if let Some(existing) = editor.existing_turn(&key) {
                anyhow::ensure!(
                    existing.root_turn_id == root.id,
                    "parent turn crosses roots"
                );
                Ok(existing.id)
            } else {
                let alias = editor.existing_wire_from_downstream(WireIdDomain::Turn, raw)?;
                editor
                    .turn_with_id(
                        &key,
                        session_id,
                        Some(&root.id),
                        Some(&root.id),
                        alias.as_deref(),
                    )
                    .map(|turn| turn.id)
            }
        })
        .transpose()?;
    Ok(resolved(admit_turn(
        editor,
        evidence,
        turn_key,
        thread_id,
        Some(&root.id),
        parent.as_deref(),
        reserved_turn,
    )?))
}

fn admit_turn(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    key: &str,
    thread: &str,
    root: Option<&str>,
    parent: Option<&str>,
    reserved: Option<&str>,
) -> Result<TurnAssignment> {
    if evidence.is_classifier() {
        editor.classifier_turn_with_id(key, thread, root, parent, reserved)
    } else {
        editor.turn_with_id(key, thread, root, parent, reserved)
    }
}

fn resolved(turn: TurnAssignment) -> ResolvedTurn {
    ResolvedTurn {
        turn_id: Some(turn.id),
        root_turn_id: Some(turn.root_turn_id),
        parent_turn_id: turn.parent_turn_id,
        started_at_unix_ms: Some(turn.started_at_unix_ms),
    }
}
