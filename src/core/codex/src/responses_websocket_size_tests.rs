use super::*;
use pretty_assertions::assert_eq;

#[test]
fn oversized_incremental_continuation_falls_back_to_the_bounded_full_frame() {
    let first_item = serde_json::json!({"type":"message","role":"user","content":[]});
    let first = serde_json::json!({
        "type":"response.create", "model":"gpt-5.4", "input":[first_item.clone()]
    });
    let mut continuation =
        ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription149);
    continuation.plan_public_create(&first);
    assert!(continuation.mark_public_create_attempted());
    continuation.observe_server_event(&serde_json::json!({
        "type":"response.completed",
        "response":{"id":"r".repeat(1024)}
    }));
    let next = serde_json::json!({
        "type":"response.create", "model":"gpt-5.4", "input":[
            first_item,
            {"type":"message","role":"user","content":[]}
        ]
    });
    let maximum = serde_json::to_string(&next).expect("full frame").len();
    let encoded =
        crate::responses_websocket_emulation::plan_public_text(&mut continuation, &next, maximum)
            .expect("bounded full fallback");
    let value: Value = serde_json::from_str(&encoded).expect("fallback JSON");
    assert!(value.get("previous_response_id").is_none());
    assert_eq!(value["input"], next["input"]);
}

#[test]
fn synthesized_frame_encoder_enforces_the_final_message_limit() {
    let frame = serde_json::json!({"type":"response.create","generate":false,"input":[]});
    let length = serde_json::to_string(&frame).expect("frame").len();
    assert!(
        crate::responses_websocket_emulation::encode_frame_bounded(&frame, length - 1).is_err()
    );
    assert!(crate::responses_websocket_emulation::encode_frame_bounded(&frame, length).is_ok());
}
