use super::*;

#[tokio::test]
async fn missing_provider_references_are_state_unavailable_for_both_codex_profiles() {
    for profile in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ] {
        for body in [
            serde_json::json!({
                "model":"gpt-5.4",
                "previous_response_id":"resp_missing",
                "input":"hello"
            }),
            serde_json::json!({
                "model":"gpt-5.4",
                "conversation":"conv_missing",
                "input":"hello"
            }),
        ] {
            let (_temp, store) = store();
            let error = prepare_stateful_codex_request(
                profile,
                EmulationTransport::Http,
                &HeaderMap::new(),
                Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
                1024 * 1024,
                CodexStateContext {
                    account_ref: ACCOUNT_REF,
                    state_namespace: NAMESPACE,
                    downstream_scope: SCOPE,
                    fingerprint_mode: FingerprintMode::Device,
                    store: &store,
                },
                false,
            )
            .await
            .expect_err("missing provider reference must fail closed");
            assert_eq!(error, StatefulPrepareError::StateUnavailable);
            assert!(!store.state_path_for_test(NAMESPACE).exists());
        }
    }
}
