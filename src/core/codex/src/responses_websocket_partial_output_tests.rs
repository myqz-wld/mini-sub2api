use super::*;

#[test]
fn unfinished_output_never_becomes_a_public_or_hidden_prewarm_baseline() {
    for hidden in [false, true] {
        let mut state = state();
        let request = request(vec![message("user", "user-1")]);
        if hidden {
            state
                .plan_hidden_setup(&request, PrewarmMode::Ordinary)
                .unwrap();
            assert!(state.mark_hidden_setup_attempted());
        } else {
            state.plan_public_create(&request);
            assert!(state.mark_public_create_attempted());
        }
        state.observe_server_event(&json!({"type":"response.output_item.done","output_index":0,"item":message("assistant","msg_done")}));
        state.observe_server_event(&json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_partial","delta":"unfinished"}));
        state.observe_server_event(&completed("resp_partial"));
        assert!(state.baseline.is_none());
        assert_eq!(
            if hidden {
                state.setup_phase
            } else {
                state.public_phase
            },
            OperationPhase::Failed
        );
        assert_eq!(
            state.plan_public_create(&request).mode,
            PublicCreateMode::Full
        );
    }
}
