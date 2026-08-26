use super::*;

const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn easy_assistant_text_becomes_output_text_while_input_roles_stay_input_text() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.4",
            "input": [
                {"type": "message", "role": "system", "content": "system rules"},
                {"type": "message", "role": "user", "content": "first question"},
                {"type": "message", "role": "assistant", "content": "first answer"},
                {"type": "message", "role": "user", "content": "continue"}
            ],
            "tools": []
        }))
        .expect("request"),
    );

    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "chatgpt-account-test",
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");
    let input = value["input"].as_array().expect("input items");
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[1]["content"][0]["type"], "input_text");
    assert_eq!(input[2]["role"], "assistant");
    assert_eq!(input[2]["content"][0]["type"], "output_text");
    assert_eq!(input[2]["content"][0]["text"], "first answer");
    assert_eq!(input[3]["role"], "user");
    assert_eq!(input[3]["content"][0]["type"], "input_text");
}

#[test]
fn malformed_serialized_identity_fails_closed_before_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert("x-codex-turn-metadata", "not-json".parse().expect("header"));
    let body = Bytes::from_static(br#"{"model":"gpt-5.4"}"#);

    assert!(
        prepare_subscription_request(
            &headers,
            body,
            64 * 1024,
            "chatgpt-account-test",
            PSEUDONYM_SCOPE,
            "request-test",
        )
        .is_err()
    );
}
