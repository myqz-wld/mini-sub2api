use super::*;
use crate::request_profile::UpstreamProfile;
use serde_json::{Value, json};

#[test]
fn native_transport_roots_and_routing_hint() {
    let caller = json!({"type":"response.create","generate":false,"stream_id":"synthetic",
        "background":true,"stream":false,"model":"gpt-5.4","input":[]});
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let prepared = prepare_value(
            UpstreamProfile::CodexSubscription1560,
            transport,
            caller.clone(),
        );
        assert!(prepared.value.get("stream_id").is_none());
        assert!(prepared.value.get("background").is_none());
        assert_eq!(prepared.value["stream"], true);
        assert_eq!(
            prepared.value.get("generate").is_some(),
            transport == EmulationTransport::WebSocket
        );
        assert!(prepared.headers.get("x-codex-routing-hint").is_some());
    }
}

#[test]
fn tool_declarations_are_distinct_from_response_history() {
    let prepared = prepare_value(
        UpstreamProfile::CodexSubscription1560,
        EmulationTransport::Http,
        json!({
        "model":"gpt-5.4", "tools":[{"type":"web_search","filters":{"allowed_domains":["example.test"],"unknown":true}},
        {"type":"image_generation"},{"type":"local_shell"},{"type":"file_search"},{"type":"computer"},{"type":"mcp"}],
        "input":[{"type":"message","role":"user","content":"task"},
          {"type":"image_generation_call","id":"ig_synthetic","status":"completed","result":"synthetic"},
          {"type":"local_shell_call","status":"completed","action":{"type":"exec","command":["true"]},
            "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic"}},
          {"type":"file_search_call","id":"fs_synthetic"},{"type":"computer_call"},{"type":"shell_call"}]}),
    );
    assert_eq!(prepared.value["tools"].as_array().unwrap().len(), 1);
    assert_eq!(
        prepared.value["tools"][0]["filters"],
        json!({"allowed_domains":["example.test"]})
    );
    let input = prepared.value["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert_eq!(input[1]["type"], "image_generation_call");
    assert_eq!(input[2]["type"], "local_shell_call");
    assert_eq!(
        input[2]["internal_chat_message_metadata_passthrough"]["turn_id"],
        "synthetic"
    );
}

#[test]
fn typed_history_drops_non_model_decoration_and_keeps_null_reasoning_content() {
    let prepared = prepare_value(
        UpstreamProfile::CodexSubscription1560,
        EmulationTransport::Http,
        json!({
        "model":"gpt-5.4","input":[
        {"type":"message","role":"assistant","status":"completed","agent":{},"content":[{"type":"output_text","text":"answer","annotations":[],"logprobs":[]}]},
        {"type":"reasoning","summary":[],"content":[],"status":"completed","agent":{},"encrypted_content":"cipher"},
        {"type":"reasoning","summary":[],"content":null,"encrypted_content":"cipher"},
        {"type":"custom_tool_call","name":"tool","call_id":"call_synthetic","input":"opaque","status":"completed"}]}),
    );
    let input = prepared.value["input"].as_array().unwrap();
    for i in [0, 1] {
        assert!(input[i].get("status").is_none());
        assert!(input[i].get("agent").is_none());
    }
    assert_eq!(
        input[0]["content"],
        json!([{"type":"output_text","text":"answer"}])
    );
    assert!(input[1].get("content").is_none());
    assert_eq!(input[2].get("content"), Some(&Value::Null));
    assert_eq!(input[3]["status"], "completed");
}

struct PreparedValue {
    headers: HeaderMap,
    value: Value,
}

fn prepare_value(
    profile: UpstreamProfile,
    transport: EmulationTransport,
    caller: Value,
) -> PreparedValue {
    let prepared = prepare_codex_overlay_for_test(
        profile,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&caller).expect("caller JSON")),
        1024 * 1024,
    )
    .expect("emulated request");
    PreparedValue {
        headers: prepared.headers,
        value: serde_json::from_slice(&prepared.body).expect("emulated JSON"),
    }
}
