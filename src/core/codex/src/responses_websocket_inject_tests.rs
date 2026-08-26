use super::ACCOUNT_NAMESPACE;
use super::CallerKind;
use super::HeaderMap;
use super::PSEUDONYM_SCOPE;
use super::ResponsesWebSocketState;
use super::UpstreamProfile;
use super::Value;
use super::device_fingerprint;
use super::prepare_client_text;

#[test]
fn bare_inject_is_byte_exact_but_emulated_inject_is_schema_filtered() {
    let original = r#" {"type":"response.inject","response_id":"resp_1","input":[{"type":"function_call_output","id":"fco_caller","call_id":"call_1","output":{"opaque":true,"unknown":true},"unsupported_item":true}],"unsupported_top":true} "#.to_string();
    let mut bare_headers = HeaderMap::new();
    let mut bare = ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::BareOpenAi);
    assert_eq!(
        prepare_client_text(
            original.clone(),
            &mut bare_headers,
            None,
            UpstreamProfile::BareOpenAi,
            PSEUDONYM_SCOPE,
            &device_fingerprint(),
            &mut bare,
        )
        .expect("bare inject"),
        original
    );

    for (profile, account_namespace) in [
        (UpstreamProfile::CodexOpenAi149, None),
        (
            UpstreamProfile::CodexSubscription149,
            Some(ACCOUNT_NAMESPACE),
        ),
    ] {
        let mut headers = HeaderMap::new();
        let mut continuation = ResponsesWebSocketState::new(CallerKind::Codex, profile);
        let got = prepare_client_text(
            original.clone(),
            &mut headers,
            account_namespace,
            profile,
            PSEUDONYM_SCOPE,
            &device_fingerprint(),
            &mut continuation,
        )
        .expect("emulated inject");
        let value: Value = serde_json::from_str(&got).expect("filtered inject JSON");
        assert_eq!(value["type"], "response.inject");
        assert_eq!(value["response_id"], "resp_1");
        assert!(value.get("unsupported_top").is_none());
        assert!(value["input"][0].get("unsupported_item").is_none());
        assert_eq!(value["input"][0]["output"]["opaque"], true);
        assert_eq!(value["input"][0]["output"]["unknown"], true);
    }
}
