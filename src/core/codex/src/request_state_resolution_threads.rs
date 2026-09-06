use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::{WireIdDomain, WireIdOwner};
use anyhow::Result;

pub(super) fn resolve_conversation(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
) -> Result<crate::request_state_editor::ConversationAssignment> {
    if let Some((_, assignment)) = editor.conversation_by_id(raw) {
        return Ok(assignment);
    }
    if let Some(projected) = editor.existing_wire_from_downstream(WireIdDomain::Session, raw)?
        && let Some((_, assignment)) = editor.conversation_by_id(&projected)
    {
        return Ok(assignment);
    }
    let key = editor.lookup("conversation", raw);
    let reserved = editor.existing_wire_from_downstream(WireIdDomain::Thread, raw)?;
    if let Some(id) = &reserved {
        anyhow::ensure!(
            editor.child_thread_by_id(id).is_none(),
            "root thread was an owning child"
        );
    }
    editor.conversation_with_id(&key, reserved.as_deref())
}

pub(super) fn resolve_thread(
    editor: &mut RequestStateEditor<'_>,
    evidence: &RequestIdentityEvidence,
    session_id: &str,
    previous_owner: Option<&WireIdOwner>,
) -> Result<(String, Option<String>, Option<String>, u64)> {
    let forked = evidence
        .forked_from_thread
        .as_deref()
        .map(|raw| resolve_fork_reference(editor, raw))
        .transpose()?;
    if !evidence.explicit_thread_lineage {
        if let Some(owner) = previous_owner
            && owner.thread_id != session_id
        {
            let (_, thread) = editor
                .child_thread_by_id(&owner.thread_id)
                .ok_or_else(|| anyhow::anyhow!("previous response thread is missing"))?;
            anyhow::ensure!(
                thread.session_id == session_id,
                "previous response thread crosses sessions"
            );
            return Ok((
                thread.id,
                thread.parent_thread_id,
                forked,
                thread.window_number,
            ));
        }
        let window = editor
            .window_number(session_id)
            .ok_or_else(|| anyhow::anyhow!("root conversation window is missing"))?;
        return Ok((session_id.to_string(), None, forked, window));
    }
    let parent_raw = evidence.parent_thread.as_deref();
    let parent = match parent_raw {
        Some(raw) if evidence.conversation.as_deref() == Some(raw) => session_id.to_string(),
        Some(raw) => resolve_thread_reference(editor, raw, session_id)?,
        None => session_id.to_string(),
    };
    let child = match evidence.thread.as_deref() {
        Some(raw) => match resolve_existing_child(editor, raw)? {
            Some(child) => {
                anyhow::ensure!(
                    child.session_id == session_id
                        && child.parent_thread_id.as_deref() == Some(parent.as_str()),
                    "child thread relationship changed"
                );
                child
            }
            None => {
                let key = editor.lookup("thread", raw);
                let reserved = editor.existing_wire_from_downstream(WireIdDomain::Thread, raw)?;
                if let Some(id) = &reserved {
                    anyhow::ensure!(
                        editor.conversation_by_id(id).is_none(),
                        "child thread was an independent root"
                    );
                }
                editor.child_thread_with_id(&key, session_id, Some(&parent), reserved.as_deref())?
            }
        },
        None => {
            let key = editor.derived_lookup(
                "thread-fallback",
                &[
                    session_id.as_bytes(),
                    parent.as_bytes(),
                    evidence.request_kind.as_bytes(),
                ],
            );
            editor.child_thread(&key, session_id, Some(&parent))?
        }
    };
    Ok((child.id, Some(parent), forked, child.window_number))
}

// A fork source describes already supplied history, not authority to continue that source's
// response or to join its session. Reserve an alias without inventing an owning thread when
// the source has not used this gateway yet; its later root/child assignment adopts that alias.
fn resolve_fork_reference(editor: &mut RequestStateEditor<'_>, raw: &str) -> Result<String> {
    if editor.conversation_by_id(raw).is_some() || editor.child_thread_by_id(raw).is_some() {
        return Ok(raw.to_string());
    }
    for domain in [WireIdDomain::Thread, WireIdDomain::Session] {
        if let Some(id) = editor.existing_wire_from_downstream(domain, raw)? {
            return Ok(id);
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    editor.bind_wire_pair(WireIdDomain::Thread, raw, &id)?;
    Ok(id)
}

fn resolve_thread_reference(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
    session_id: &str,
) -> Result<String> {
    if raw == session_id {
        return Ok(session_id.to_string());
    }
    if let Some(existing) = resolve_existing_child(editor, raw)? {
        anyhow::ensure!(
            existing.session_id == session_id,
            "thread reference crosses sessions"
        );
        return Ok(existing.id);
    }
    if let Some(root) = editor.existing_wire_from_downstream(WireIdDomain::Session, raw)? {
        anyhow::ensure!(root == session_id, "thread reference crosses sessions");
        return Ok(root);
    }
    let key = editor.lookup("thread", raw);
    if let Some(existing) = editor.existing_child_thread(&key) {
        anyhow::ensure!(
            existing.session_id == session_id,
            "thread reference crosses sessions"
        );
        return Ok(existing.id);
    }
    let reserved = editor.existing_wire_from_downstream(WireIdDomain::Thread, raw)?;
    editor
        .child_thread_with_id(&key, session_id, Some(session_id), reserved.as_deref())
        .map(|thread| thread.id)
}

fn resolve_existing_child(
    editor: &mut RequestStateEditor<'_>,
    raw: &str,
) -> Result<Option<crate::request_state_editor::ThreadAssignment>> {
    if let Some((_, assignment)) = editor.child_thread_by_id(raw) {
        return Ok(Some(assignment));
    }
    if let Some(projected) = editor.existing_wire_from_downstream(WireIdDomain::Thread, raw)?
        && let Some((_, assignment)) = editor.child_thread_by_id(&projected)
    {
        return Ok(Some(assignment));
    }
    Ok(None)
}
