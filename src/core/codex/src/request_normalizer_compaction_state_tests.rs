use super::*;
use std::assert_eq;

#[tokio::test]
async fn compaction_commits_only_on_completed_and_same_base_operations_converge() {
    let (_temp, store) = store();
    let request = |session: &str, turn: &str| {
        let metadata = serde_json::json!({
            "session_id":session,
            "thread_id":session,
            "turn_id":turn,
            "request_kind":"compaction",
            "compaction":{
                "trigger":"manual",
                "reason":"user_requested",
                "implementation":"responses_compaction_v2",
                "phase":"standalone_turn",
                "strategy":"memento"
            }
        });
        serde_json::json!({
            "model":"gpt-5.4",
            "input":[
                {"type":"message","role":"user","content":"history"},
                {"type":"compaction_trigger"}
            ],
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}
        })
    };
    let first = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    let first_value = value(&first);
    let first_pending = first
        .pending_compaction
        .as_ref()
        .expect("first pending compaction")
        .clone();
    let retry = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    let retry_value = value(&retry);
    assert_eq!(retry.pending_compaction.as_ref(), Some(&first_pending));
    let overlapping = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a2"),
    )
    .await;
    let overlapping_value = value(&overlapping);
    assert_eq!(
        overlapping
            .pending_compaction
            .as_ref()
            .expect("overlapping pending")
            .target_window,
        first_pending.target_window
    );
    let other = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-b", "compact-turn-b1"),
        )
        .await,
    );
    assert_eq!(
        first_value["client_metadata"]["x-codex-window-id"],
        retry_value["client_metadata"]["x-codex-window-id"]
    );
    assert!(
        first_value["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert!(
        overlapping_value["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert!(
        other["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert_ne!(
        first_value["client_metadata"]["session_id"],
        other["client_metadata"]["session_id"]
    );

    let failed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        first.resolved_identity.as_ref(),
        first.pending_compaction.as_ref(),
    );
    failed
        .translate_value(serde_json::json!({
            "type":"response.failed",
            "response":{"id":"resp_failed"}
        }))
        .await
        .expect("translate failed terminal");
    let after_failure = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    assert!(
        value(&after_failure)["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );

    let completed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        after_failure.resolved_identity.as_ref(),
        after_failure.pending_compaction.as_ref(),
    );
    completed
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_completed","output":[{"type":"compaction","encrypted_content":"synthetic"}]}
        }))
        .await
        .expect("commit completed terminal");
    let overlapping_completed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        overlapping.resolved_identity.as_ref(),
        overlapping.pending_compaction.as_ref(),
    );
    overlapping_completed
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_overlapping","output":[{"type":"compaction","encrypted_content":"synthetic"}]}
        }))
        .await
        .expect("converge overlapping completion");

    let committed_retry = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-a", "compact-turn-a1"),
        )
        .await,
    );
    assert!(
        committed_retry["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0")),
        "a completed marker retry must reuse its original committed base"
    );

    let later = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-a", "compact-turn-a3"),
        )
        .await,
    );
    assert!(
        later["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":1"))
    );
}
