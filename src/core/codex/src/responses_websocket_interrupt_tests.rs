use super::*;

#[test]
fn interrupted_invalid_output_never_installs_a_socket_baseline() {
    let cases = crate::response_interrupt::fixtures::inconsistent_outputs("resp_integrity")
        .into_iter()
        .flat_map(|(name, events)| {
            [None, Some(json!([]))].map(|footer| (name.clone(), events.clone(), true, footer))
        })
        .chain(
            crate::response_interrupt::fixtures::inconsistent_terminals("resp_integrity")
                .into_iter()
                .map(|(name, events, footer)| (name, events, false, footer)),
        );
    for (name, events, early_failure, footer) in cases {
        let mut state = state();
        let initial = request(vec![message("user", "msg_user")]);
        state.plan_public_create(&initial);
        assert!(state.mark_public_create_attempted());
        state.observe_server_event(
            &json!({"type":"response.created","response":{"id":"resp_integrity"}}),
        );
        state.request_interrupt("resp_integrity").unwrap();
        for event in &events {
            state.observe_server_event(event);
        }
        assert_eq!(
            state.public_phase() == OperationPhase::Failed,
            early_failure,
            "{name}"
        );
        let mut terminal = json!({"type":"response.incomplete","response":{
                "id":"resp_integrity","status":"incomplete","incomplete_details":{"reason":"interrupted"}}});
        if let Some(output) = footer {
            terminal["response"]["output"] = output;
        }
        state.observe_server_event(&terminal);
        assert_eq!(state.public_phase(), OperationPhase::Failed, "{name}");
        assert!(state.baseline.is_none(), "{name}");
        let plan = state.plan_public_create(&initial);
        assert_ne!(plan.mode, PublicCreateMode::Incremental, "{name}");
    }
}

#[test]
fn interrupted_socket_baseline_keeps_done_items_and_omits_discarded_indexes() {
    let mut state = state();
    let initial = request(vec![message("user", "msg_user")]);
    state.plan_public_create(&initial);
    assert!(state.mark_public_create_attempted());
    state.observe_server_event(
        &json!({"type":"response.created","response":{"id":"resp_interrupt"}}),
    );
    assert!(state.request_interrupt("different-response").is_err());
    state.request_interrupt("resp_interrupt").unwrap();
    let kept = message("assistant", "msg_kept");
    state.observe_server_event(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"reason_partial"}}));
    state.observe_server_event(&json!({"type":"response.output_item.interrupted","response_id":"resp_interrupt","output_index":0,"item_id":"reason_partial"}));
    state.observe_server_event(
        &json!({"type":"response.output_item.done","output_index":1,"item":kept}),
    );
    state.observe_server_event(&json!({"type":"response.incomplete","response":{"id":"resp_interrupt","status":"incomplete","incomplete_details":{"reason":"interrupted"},"output":[kept]}}));
    assert_eq!(state.public_phase(), OperationPhase::Completed);
    let next = request(vec![
        message("user", "msg_user"),
        kept,
        message("user", "msg_next"),
    ]);
    let plan = state.plan_public_create(&next);
    assert_eq!(plan.mode, PublicCreateMode::Incremental);
    assert_eq!(plan.frame["previous_response_id"], "resp_interrupt");
    assert_eq!(plan.frame["input"], json!([message("user", "msg_next")]));
}
