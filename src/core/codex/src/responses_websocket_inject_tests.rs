use super::ACCOUNT_NAMESPACE;
use super::HeaderMap;
use super::PSEUDONYM_SCOPE;
use super::UpstreamProfile;
use super::Value;
use super::device_fingerprint;
use super::prepare_client_text;

#[tokio::test]
async fn bare_inject_is_byte_exact_but_emulated_inject_is_schema_filtered() {
    let original = r#" {"type":"response.inject","response_id":"resp_1","input":[{"type":"function_call_output","id":"fco_caller","call_id":"call_1","output":{"opaque":true,"unknown":true},"unsupported_item":true}],"unsupported_top":true} "#.to_string();
    let (_temp, store) = super::request_state_store();
    let mut identity = None;
    let mut bare_headers = HeaderMap::new();
    let got = prepare_client_text(
        original.clone(),
        &mut bare_headers,
        super::ACCOUNT_REF,
        None,
        UpstreamProfile::BareOpenAi,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("bare inject");
    assert_eq!(got.text, original);

    for (profile, account_namespace) in [
        (UpstreamProfile::CodexOpenAi149, Some(super::ACCOUNT_REF)),
        (
            UpstreamProfile::CodexSubscription149,
            Some(ACCOUNT_NAMESPACE),
        ),
    ] {
        let state_namespace = account_namespace.expect("stateful profile namespace");
        let (response_alias, call_alias) = store
            .edit(
                state_namespace,
                super::ACCOUNT_REF,
                PSEUDONYM_SCOPE,
                |editor| {
                    Ok((
                        editor.wire_from_upstream(
                            crate::request_state_types::WireIdDomain::Response,
                            "resp_provider",
                        )?,
                        editor.wire_from_upstream(
                            crate::request_state_types::WireIdDomain::Call,
                            "call_provider",
                        )?,
                    ))
                },
            )
            .await
            .expect("seed inject references");
        let emulated = serde_json::json!({
            "type":"response.inject",
            "response_id":response_alias,
            "input":[{
                "type":"function_call_output",
                "id":"fco_caller",
                "call_id":call_alias,
                "output":{"opaque":true,"unknown":true},
                "unsupported_item":true
            }],
            "unsupported_top":true
        })
        .to_string();
        let mut headers = HeaderMap::new();
        let mut identity = None;
        let got = prepare_client_text(
            emulated,
            &mut headers,
            super::ACCOUNT_REF,
            account_namespace,
            profile,
            PSEUDONYM_SCOPE,
            &device_fingerprint(),
            &store,
            &mut identity,
        )
        .await
        .expect("emulated inject");
        let value: Value = serde_json::from_str(&got.text).expect("filtered inject JSON");
        assert_eq!(value["type"], "response.inject");
        if profile.uses_identity_state() {
            assert_eq!(value["response_id"], "resp_provider");
            assert_ne!(value["input"][0]["id"], "fco_caller");
            assert_eq!(value["input"][0]["call_id"], "call_provider");
        } else {
            assert_eq!(value["response_id"], "resp_1");
            assert_eq!(value["input"][0]["id"], "fco_caller");
            assert_eq!(value["input"][0]["call_id"], "call_1");
        }
        assert!(value.get("unsupported_top").is_none());
        assert!(value["input"][0].get("unsupported_item").is_none());
        assert_eq!(value["input"][0]["output"]["opaque"], true);
        assert_eq!(value["input"][0]["output"]["unknown"], true);
    }
}
