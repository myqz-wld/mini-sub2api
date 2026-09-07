use super::*;
use serde_json::json;

fn request(thread: &str, input: Value) -> Value {
    json!({"type":"response.create","model":"gpt-5.4","input":input,
        "client_metadata":{"thread_id":thread}})
}

#[test]
fn automatic_reuse_never_carries_another_threads_baseline() {
    let mut state =
        ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
    let first = request(
        "first",
        json!([{"type":"message","role":"user","content":"seed"}]),
    );
    state.plan_public_create(&first);
    assert!(state.mark_public_create_attempted());
    state.observe_server_event(
        &json!({"type":"response.completed","response":{"id":"response-first","output":[]}}),
    );
    let next_input = json!([{"type":"message","role":"user","content":"seed"}, {"type":"message","role":"user","content":"next"}]);
    let plan = state.plan_public_create(&request("second", next_input));
    assert_eq!(plan.mode, PublicCreateMode::Full);
    assert!(plan.frame.get("previous_response_id").is_none());
}

#[test]
fn reconstructed_explicit_reference_disables_hidden_setup_and_automatic_reuse_once() {
    let mut state =
        ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
    let full = request("thread", json!([]));
    state.mark_rebuilt_reference(true);
    assert!(
        state
            .plan_hidden_setup(&full, PrewarmMode::Ordinary)
            .is_none()
    );
    let plan = state.plan_public_create(&full);
    assert_eq!(plan.mode, PublicCreateMode::ExplicitState);
    assert!(plan.frame.get("previous_response_id").is_none());
    assert!(state.mark_public_create_attempted());
    state.observe_server_event(
        &json!({"type":"response.completed","response":{"id":"rebuilt","output":[]}}),
    );
    assert_eq!(state.plan_public_create(&full).mode, PublicCreateMode::Full);
    assert!(state.mark_public_create_attempted());
    state.observe_server_event(
        &json!({"type":"response.completed","response":{"id":"ordinary","output":[]}}),
    );
    assert_eq!(
        state.plan_public_create(&full).mode,
        PublicCreateMode::Incremental
    );
}

#[test]
fn hidden_setup_token_requires_completion_and_does_not_survive_failure_or_reconnect() {
    for outcome in [
        "response.failed",
        "response.incomplete",
        "error",
        "completed-reset",
    ] {
        let mut state =
            ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
        state
            .plan_hidden_setup(&request("thread", json!([])), PrewarmMode::Ordinary)
            .unwrap();
        assert!(state.mark_hidden_setup_attempted());
        for token in ["first-token", "later-token"] {
            state.observe_server_event(
                &json!({"type":"response.metadata","headers":{"x-codex-turn-state":token}}),
            );
        }
        assert!(state.setup_turn_state().is_none());
        if outcome == "completed-reset" {
            state.observe_server_event(
                &json!({"type":"response.completed","response":{"id":"prewarm","output":[]}}),
            );
            assert_eq!(state.setup_turn_state(), Some("first-token"));
            state.reset_for_reconnect();
        } else {
            state.observe_server_event(&json!({"type":outcome}));
        }
        assert!(state.setup_turn_state().is_none(), "{outcome}");
    }
}

#[test]
fn final_only_compaction_never_installs_a_socket_baseline() {
    let mut state =
        ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
    let first = request("thread", json!([]));
    let pending = PendingCompaction {
        marker_key: "marker".into(),
        thread_id: "thread".into(),
        target_window: 1,
        requires_compaction_item: true,
    };
    state.plan_public_create_with_state(&first, &[], Some(pending));
    assert!(state.mark_public_create_attempted());
    let compacted = json!({"type":"compaction","encrypted_content":"opaque"});
    state.observe_server_event(&json!({"type":"response.completed","response":{"id":"compacted","output":[compacted.clone()]}}));
    let next = request(
        "thread",
        json!([compacted,{"type":"message","role":"user","content":"next"}]),
    );
    assert_eq!(state.plan_public_create(&next).mode, PublicCreateMode::Full);
}
