use super::*;
use serde_json::json;

fn state() -> ResponsesWebSocketState {
    ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription149)
}

fn item(item_type: &str, id: &str) -> Value {
    json!({"type": item_type, "id": id})
}

fn message(role: &str, id: &str) -> Value {
    json!({"type": "message", "role": role, "id": id, "content": []})
}

fn request(input: Vec<Value>) -> Value {
    json!({
        "type": "response.create",
        "model": "gpt-5.4",
        "input": input,
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": {"effort": "medium"},
        "store": false,
        "stream": true,
        "stream_options": {"include_usage": true},
        "include": ["reasoning.encrypted_content"],
        "service_tier": "priority",
        "max_output_tokens": 128,
        "metadata": {"test_class": "state"},
        "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
        "safety_identifier": "test-safety-id",
        "client_metadata": {"caller_field": "stable"},
    })
}

fn completed(response_id: &str) -> Value {
    json!({"type": "response.completed", "response": {"id": response_id}})
}

fn finish_active(state: &mut ResponsesWebSocketState, response_id: &str, output: &[Value]) {
    for item in output {
        assert_ne!(
            state.observe_server_event(&json!({"type": "response.output_item.done", "item": item})),
            EventDisposition::Unassociated
        );
    }
    state.observe_server_event(&completed(response_id));
}

fn establish_public_baseline(
    state: &mut ResponsesWebSocketState,
    request: &Value,
    response_id: &str,
    output: &[Value],
) {
    let plan = state.plan_public_create(request);
    assert_eq!(plan.mode, PublicCreateMode::Full);
    assert!(state.mark_public_create_attempted());
    finish_active(state, response_id, output);
}

#[test]
fn ordinary_and_lite_hidden_prewarm_use_the_expected_stable_prefix() {
    let ordinary_request = request(vec![message("user", "user-1")]);
    let mut ordinary = state();
    let plan = ordinary
        .plan_hidden_setup(&ordinary_request, PrewarmMode::Ordinary)
        .expect("ordinary prewarm");
    assert_eq!(plan.frame["input"], json!([]));
    assert_eq!(plan.frame["generate"], false);
    assert!(!ordinary.public_create_attempted());

    let tools = json!({"type": "additional_tools", "role": "developer", "tools": []});
    let developer = message("developer", "developer-1");
    let user = message("user", "user-1");
    let lite_request = request(vec![tools.clone(), developer.clone(), user]);
    let mut lite = state();
    let plan = lite
        .plan_hidden_setup(&lite_request, PrewarmMode::ResponsesLite)
        .expect("lite prewarm");
    assert_eq!(plan.frame["input"], json!([tools, developer]));
    assert_eq!(plan.frame["generate"], false);
}

#[test]
fn first_and_second_turns_reuse_completed_response_and_output_prefixes() {
    let user_1 = message("user", "user-1");
    let assistant_1 = message("assistant", "assistant-1");
    let user_2 = message("user", "user-2");
    let first = request(vec![user_1.clone()]);
    let mut state = state();

    state
        .plan_hidden_setup(&first, PrewarmMode::Ordinary)
        .expect("prewarm");
    assert!(state.mark_hidden_setup_attempted());
    finish_active(&mut state, "response-prewarm", &[]);

    let first_plan = state.plan_public_create(&first);
    assert_eq!(first_plan.mode, PublicCreateMode::Incremental);
    assert_eq!(first_plan.frame["previous_response_id"], "response-prewarm");
    assert_eq!(first_plan.frame["input"], json!([user_1.clone()]));
    assert!(state.mark_public_create_attempted());
    finish_active(&mut state, "response-1", std::slice::from_ref(&assistant_1));

    let second = request(vec![user_1, assistant_1, user_2.clone()]);
    let second_plan = state.plan_public_create(&second);
    assert_eq!(second_plan.mode, PublicCreateMode::Incremental);
    assert_eq!(second_plan.frame["previous_response_id"], "response-1");
    assert_eq!(second_plan.frame["input"], json!([user_2]));
}

#[test]
fn completed_response_output_can_supply_the_reuse_baseline() {
    let first = request(vec![message("user", "user-1")]);
    let assistant = message("assistant", "assistant-1");
    let next = request(vec![
        message("user", "user-1"),
        assistant.clone(),
        message("user", "user-2"),
    ]);
    let mut state = state();
    state.plan_public_create(&first);
    state.mark_public_create_attempted();
    state.observe_server_event(&json!({
        "type": "response.completed",
        "response": {"id": "response-1", "output": [assistant]}
    }));

    let plan = state.plan_public_create(&next);
    assert_eq!(plan.mode, PublicCreateMode::Incremental);
    assert_eq!(plan.frame["input"], json!([message("user", "user-2")]));
}

#[test]
fn explicit_previous_conversation_and_generate_carriers_take_precedence() {
    let cases = [
        ("previous_response_id", json!("caller-response")),
        ("conversation", json!("conversation-1")),
        ("generate", json!(false)),
        ("stream_id", json!("stream-caller")),
    ];
    for (field, value) in cases {
        let user = message("user", "user-1");
        let output = message("assistant", "assistant-1");
        let mut explicit = request(vec![
            user.clone(),
            output.clone(),
            message("user", "user-2"),
        ]);
        explicit[field] = value;
        let mut state = state();
        assert!(
            state
                .plan_hidden_setup(&explicit, PrewarmMode::Ordinary)
                .is_none()
        );
        establish_public_baseline(
            &mut state,
            &request(vec![user]),
            "internal-response",
            std::slice::from_ref(&output),
        );
        let plan = state.plan_public_create(&explicit);
        assert_eq!(plan.mode, PublicCreateMode::ExplicitState, "{field}");
        assert_eq!(plan.frame, explicit, "{field}");
    }
}

#[test]
fn automatic_prewarm_is_limited_to_bare_subscription_callers() {
    let request = request(vec![message("user", "user-1")]);
    let cases = [
        (
            CallerKind::Codex,
            UpstreamProfile::CodexSubscription149,
            PublicCreateMode::Full,
        ),
        (
            CallerKind::Bare,
            UpstreamProfile::BareOpenAi,
            PublicCreateMode::Passthrough,
        ),
        (
            CallerKind::Codex,
            UpstreamProfile::CodexOpenAi149,
            PublicCreateMode::Full,
        ),
    ];
    for (caller, profile, expected_mode) in cases {
        let mut state = ResponsesWebSocketState::new(caller, profile);
        assert!(
            state
                .plan_hidden_setup(&request, PrewarmMode::Ordinary)
                .is_none()
        );
        assert_eq!(state.plan_public_create(&request).mode, expected_mode);
    }
}

#[test]
fn every_retained_non_input_property_change_forces_a_full_frame() {
    let changes = [
        ("model", json!("gpt-5.5")),
        ("tools", json!([{"type": "function", "name": "changed"}])),
        ("reasoning", json!({"effort": "high"})),
        ("service_tier", json!("default")),
        ("max_output_tokens", json!(256)),
        ("metadata", json!({"test_class": "changed"})),
        (
            "prompt_cache_options",
            json!({"mode": "implicit", "ttl": "1h"}),
        ),
        ("safety_identifier", json!("changed-test-safety-id")),
    ];
    for (field, value) in changes {
        let user = message("user", "user-1");
        let output = message("assistant", "assistant-1");
        let first = request(vec![user.clone()]);
        let mut state = state();
        establish_public_baseline(
            &mut state,
            &first,
            "response-1",
            std::slice::from_ref(&output),
        );
        let mut next = request(vec![user, output, message("user", "user-2")]);
        next[field] = value;

        let plan = state.plan_public_create(&next);
        assert_eq!(plan.mode, PublicCreateMode::Full, "{field}");
        assert!(plan.frame.get("previous_response_id").is_none(), "{field}");
        assert_eq!(plan.frame["input"], next["input"], "{field}");
    }
}

#[test]
fn volatile_metadata_stream_options_and_synthesized_wire_ids_do_not_block_reuse() {
    let first_user = json!({
        "type":"message", "id":"generated-first", "role":"user", "content":[],
        "internal_chat_message_metadata_passthrough":{"turn_id":"turn-first","create_time":1}
    });
    let assistant = json!({
        "type":"message", "id":"provider-first", "role":"assistant", "content":[],
        "internal_chat_message_metadata_passthrough":{"turn_id":"turn-first"}
    });
    let mut first = request(vec![first_user]);
    first["client_metadata"] = json!({"turn_id":"turn-first"});
    first["stream_options"] = json!({"include_obfuscation":true});
    let mut state = state();
    let first_plan =
        state.plan_public_create_with_synthesized_ids(&first, &["generated-first".to_string()]);
    assert_eq!(first_plan.mode, PublicCreateMode::Full);
    assert!(state.mark_public_create_attempted());
    finish_active(&mut state, "response-1", std::slice::from_ref(&assistant));

    let mut next = request(vec![
        json!({
            "type":"message", "id":"generated-second", "role":"user", "content":[],
            "internal_chat_message_metadata_passthrough":{"turn_id":"turn-second","create_time":2}
        }),
        json!({
            "type":"message", "id":"provider-first", "role":"assistant", "content":[],
            "internal_chat_message_metadata_passthrough":{"turn_id":"turn-second"}
        }),
        message("user", "new-user"),
    ]);
    next["client_metadata"] = json!({"turn_id":"turn-second"});
    next["stream_options"] = json!({"include_obfuscation":false});

    let plan =
        state.plan_public_create_with_synthesized_ids(&next, &["generated-second".to_string()]);
    assert_eq!(plan.mode, PublicCreateMode::Incremental);
    assert_eq!(plan.frame["previous_response_id"], "response-1");
    assert_eq!(plan.frame["input"], json!([message("user", "new-user")]));
}

#[test]
fn explicit_input_or_output_item_id_changes_force_full_fallback() {
    for (name, next_user_id, next_output_id) in [
        ("input", "user-changed", "assistant-stable"),
        ("output", "user-stable", "assistant-changed"),
    ] {
        let first = request(vec![message("user", "user-stable")]);
        let assistant = message("assistant", "assistant-stable");
        let mut state = state();
        establish_public_baseline(
            &mut state,
            &first,
            "response-1",
            std::slice::from_ref(&assistant),
        );
        let next = request(vec![
            message("user", next_user_id),
            message("assistant", next_output_id),
            message("user", "user-new"),
        ]);
        assert_eq!(
            state.plan_public_create(&next).mode,
            PublicCreateMode::Full,
            "{name}"
        );
    }
}

#[test]
fn cumulative_output_budget_disables_reuse_without_retaining_more_items() {
    let first = request(vec![message("user", "user-1")]);
    let output = message("assistant", "assistant-1");
    let item_bytes = crate::responses_websocket_projection::encoded_len_within(&output, usize::MAX)
        .expect("encoded item size");
    let mut state = ResponsesWebSocketState::with_output_limits(
        CallerKind::Bare,
        UpstreamProfile::CodexSubscription149,
        8,
        item_bytes + 1,
    );
    let plan = state.plan_public_create(&first);
    assert_eq!(plan.mode, PublicCreateMode::Full);
    assert!(state.mark_public_create_attempted());
    finish_active(&mut state, "response-1", &[output.clone(), output.clone()]);

    let next = request(vec![
        message("user", "user-1"),
        output.clone(),
        output,
        message("user", "user-2"),
    ]);
    assert_eq!(state.plan_public_create(&next).mode, PublicCreateMode::Full);
}

#[test]
fn tool_output_prefix_mismatch_and_unknown_items_fall_back_to_full() {
    let user = message("user", "user-1");
    let call = item("function_call", "call-1");
    let first = request(vec![user.clone()]);
    let mut state = state();
    establish_public_baseline(
        &mut state,
        &first,
        "response-1",
        std::slice::from_ref(&call),
    );
    let mismatched = request(vec![
        user,
        item("function_call_output", "output-1"),
        message("user", "user-2"),
    ]);
    assert_eq!(
        state.plan_public_create(&mismatched).mode,
        PublicCreateMode::Full
    );

    let unknown = request(vec![item("future_unknown_item", "unknown-1")]);
    let plan = state.plan_public_create(&unknown);
    assert_eq!(plan.mode, PublicCreateMode::Full);
    assert_eq!(plan.frame, unknown);
}

#[test]
fn failed_incomplete_and_error_events_clear_reuse() {
    for event_type in ["response.failed", "response.incomplete", "error"] {
        let first = request(vec![message("user", "user-1")]);
        let mut state = state();
        establish_public_baseline(&mut state, &first, "response-1", &[]);
        let next = request(vec![message("user", "user-1"), message("user", "user-2")]);
        assert_eq!(
            state.plan_public_create(&next).mode,
            PublicCreateMode::Incremental
        );
        state.mark_public_create_attempted();
        assert_eq!(
            state.observe_server_event(&json!({"type": event_type})),
            EventDisposition::ForwardPublic
        );
        assert_eq!(state.public_phase(), OperationPhase::Failed);
        assert_eq!(
            state.plan_public_create(&next).mode,
            PublicCreateMode::Full,
            "{event_type}"
        );
    }
}

#[test]
fn setup_failure_transport_failure_reconnect_and_reset_clear_state() {
    let first = request(vec![message("user", "user-1")]);
    let next = request(vec![message("user", "user-1"), message("user", "user-2")]);
    let mut state = state();
    state
        .plan_hidden_setup(&first, PrewarmMode::Ordinary)
        .expect("prewarm");
    state.mark_hidden_setup_attempted();
    state.fail_hidden_setup();
    assert_eq!(state.setup_phase(), OperationPhase::Failed);
    assert!(!state.public_create_attempted());
    assert_eq!(
        state.plan_public_create(&first).mode,
        PublicCreateMode::Full
    );
    state.mark_public_create_attempted();
    finish_active(&mut state, "response-1", &[]);

    assert_eq!(
        state.plan_public_create(&next).mode,
        PublicCreateMode::Incremental
    );
    state.mark_public_create_attempted();
    state.fail_public_create();
    assert_eq!(state.public_phase(), OperationPhase::Failed);
    assert_eq!(state.plan_public_create(&next).mode, PublicCreateMode::Full);

    state.mark_public_create_attempted();
    finish_active(&mut state, "response-2", &[]);
    state.reset_for_reconnect();
    assert_eq!(state.setup_phase(), OperationPhase::Idle);
    assert_eq!(state.public_phase(), OperationPhase::Idle);
    assert_eq!(state.plan_public_create(&next).mode, PublicCreateMode::Full);
    state.reset();
    assert_eq!(state.public_phase(), OperationPhase::Idle);
}

#[test]
fn hidden_setup_observation_never_marks_or_routes_as_public_delivery() {
    let request = request(vec![message("user", "user-1")]);
    let mut state = state();
    state
        .plan_hidden_setup(&request, PrewarmMode::Ordinary)
        .expect("prewarm");
    assert_eq!(state.setup_phase(), OperationPhase::Planned);
    assert_eq!(state.public_phase(), OperationPhase::Idle);
    assert!(!state.public_create_attempted());
    assert!(state.mark_hidden_setup_attempted());
    assert_eq!(
        state.observe_server_event(&json!({"type": "response.created"})),
        EventDisposition::ConsumeHiddenSetup
    );
    assert_eq!(state.setup_phase(), OperationPhase::ResponseObserved);
    assert_eq!(state.public_phase(), OperationPhase::Idle);
    assert!(!state.public_create_attempted());
    state.observe_server_event(&completed("response-prewarm"));
    assert_eq!(state.setup_phase(), OperationPhase::Completed);

    state.plan_public_create(&request);
    assert_eq!(state.public_phase(), OperationPhase::Planned);
    assert!(!state.public_create_attempted());
    assert!(state.mark_public_create_attempted());
    assert!(state.public_create_attempted());
    assert_eq!(
        state.observe_server_event(&json!({"type": "response.created"})),
        EventDisposition::ForwardPublic
    );
}
