use super::*;
use crate::codex_instructions;
use pretty_assertions::assert_eq;

#[test]
fn normal_codex_profiles_pin_base_and_append_custom_as_developer() {
    for profile in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ] {
        for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
            let normalized = prepare(
                profile,
                serde_json::json!({
                    "type": "response.create",
                    "model": "gpt-5.4",
                    "instructions": "caller custom instructions",
                    "input": [{"role":"user","content":"hello"}],
                    "tools": []
                }),
                transport,
            )
            .expect("normalized request");

            assert_eq!(
                normalized["instructions"],
                codex_instructions::for_model("gpt-5.4")
            );
            assert_developer_text(&normalized["input"][0], "caller custom instructions");
            assert_eq!(normalized["input"][1]["role"], "user");
        }
    }
}

#[test]
fn normal_profile_does_not_duplicate_known_codex_prompts() {
    let caller_base_with_custom = format!(
        "{}{}",
        codex_instructions::for_model("gpt-5.4"),
        "caller suffix after known base"
    );
    let normalized = prepare(
        UpstreamProfile::CodexOpenAi149,
        serde_json::json!({
            "model": "gpt-5.4-mini",
            "instructions": caller_base_with_custom,
            "input": [
                developer_message(codex_instructions::for_model("gpt-5.2")),
                developer_message("existing custom developer message"),
                {"role":"user","content":"hello"}
            ],
            "tools": []
        }),
        EmulationTransport::Http,
    )
    .expect("normalized request");

    assert_eq!(
        normalized["instructions"],
        codex_instructions::for_model("gpt-5.4-mini")
    );
    assert_eq!(normalized["input"].as_array().expect("input").len(), 3);
    assert_developer_text(&normalized["input"][0], "caller suffix after known base");
    assert_developer_text(&normalized["input"][1], "existing custom developer message");
    assert_eq!(normalized["input"][2]["role"], "user");
}

#[test]
fn lite_codex_profiles_use_tools_base_custom_then_caller_input() {
    for profile in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ] {
        for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
            let normalized = prepare(
                profile,
                serde_json::json!({
                    "type": "response.create",
                    "model": "gpt-5.6-sol",
                    "instructions": "caller custom instructions",
                    "input": [{"role":"user","content":"hello"}],
                    "tools": [{"type":"function","name":"lookup"}]
                }),
                transport,
            )
            .expect("normalized request");

            assert!(normalized.get("instructions").is_none());
            assert_eq!(normalized["input"][0]["type"], "additional_tools");
            assert_developer_text(
                &normalized["input"][1],
                codex_instructions::for_model("gpt-5.6-sol"),
            );
            assert_developer_text(&normalized["input"][2], "caller custom instructions");
            assert_eq!(normalized["input"][3]["role"], "user");
        }
    }
}

#[test]
fn already_shaped_lite_request_replaces_known_base_and_preserves_custom() {
    let old_base_with_custom = format!(
        "{}{}",
        codex_instructions::for_model("gpt-5.4"),
        "custom suffix from old base carrier"
    );
    let normalized = prepare(
        UpstreamProfile::CodexOpenAi149,
        serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.6-terra",
            "input": [
                {"type":"additional_tools","role":"developer","tools":[]},
                developer_message(&old_base_with_custom),
                developer_message("existing custom developer message"),
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"hello"}
                ]}
            ]
        }),
        EmulationTransport::WebSocket,
    )
    .expect("normalized request");

    assert!(normalized.get("instructions").is_none());
    assert_eq!(normalized["input"][0]["type"], "additional_tools");
    assert_developer_text(
        &normalized["input"][1],
        codex_instructions::for_model("gpt-5.6-terra"),
    );
    assert_developer_text(
        &normalized["input"][2],
        "custom suffix from old base carrier",
    );
    assert_developer_text(&normalized["input"][3], "existing custom developer message");
    assert_eq!(normalized["input"][4]["role"], "user");
}

#[test]
fn lite_incremental_websocket_request_does_not_repeat_base_prompt() {
    let normalized = prepare(
        UpstreamProfile::CodexOpenAi149,
        serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.6-luna",
            "previous_response_id": "resp_previous",
            "input": []
        }),
        EmulationTransport::WebSocket,
    )
    .expect("normalized request");

    assert!(normalized.get("instructions").is_none());
    assert_eq!(normalized["input"], serde_json::json!([]));
}

#[test]
fn custom_instructions_fail_closed_when_input_cannot_hold_developer_message() {
    let result = prepare(
        UpstreamProfile::CodexOpenAi149,
        serde_json::json!({
            "model": "gpt-5.4",
            "instructions": "caller custom instructions",
            "input": {"invalid":"shape"}
        }),
        EmulationTransport::Http,
    );
    assert!(result.is_err());
}

fn prepare(
    profile: UpstreamProfile,
    caller: Value,
    transport: EmulationTransport,
) -> Result<Value, ()> {
    let prepared = prepare_codex_overlay_for_test(
        profile,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&caller).expect("caller JSON")),
        1024 * 1024,
    )?;
    serde_json::from_slice(&prepared.body).map_err(|_| ())
}

fn developer_message(text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "developer",
        "content": [{"type":"input_text","text":text}]
    })
}

fn assert_developer_text(item: &Value, expected: &str) {
    assert_eq!(item["type"], "message");
    assert_eq!(item["role"], "developer");
    assert_eq!(item["content"][0]["type"], "input_text");
    assert_eq!(item["content"][0]["text"], expected);
}
