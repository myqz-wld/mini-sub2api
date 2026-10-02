use super::*;
use serde_json::json;

async fn project(
    store: &RequestStateStore,
    transport: EmulationTransport,
    input: Value,
) -> PreparedEmulatedRequest {
    let body = json!({"model":"gpt-5.4","input":input,
        "client_metadata":{"session_id":"reasoning-tail-session"}});
    prepare_identity_request(
        UpstreamProfile::CodexSubscription1592,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            binding: None,
            socket_id: None,
            account_ref: ACCOUNT_REF,
            state_namespace: NAMESPACE,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store,
        },
        false,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn implicit_turn_resolution_uses_the_agent_before_trailing_reasoning() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for kind in ["NEW_TASK", "MESSAGE", "FINAL_ANSWER"] {
            for trailing in [1, 3] {
                let (_temp, store) = store();
                let first = project(&store, transport, json!([
                    {"type":"message","id":"msg_original","role":"user","content":"Synthetic original task"}
                ])).await;
                let first_identity = first.resolved_identity.as_ref().unwrap();
                let mut items = vec![json!({"type":"agent_message","id":"amsg_followup",
                    "author":"/root","recipient":"/root/worker",
                    "content":[{"type":"input_text","text":format!("Message Type: {kind}\nPayload:\nSynthetic follow-up")}]})];
                items.extend((0..trailing).map(|index| json!({"type":"reasoning",
                    "id":format!("rs_tail_{index}"),"summary":[],"encrypted_content":"synthetic-cipher"})));
                let next = project(&store, transport, json!(items)).await;
                let next_identity = next.resolved_identity.as_ref().unwrap();
                assert_eq!(first_identity.session_id, next_identity.session_id);
                assert_eq!(
                    first_identity.turn_id == next_identity.turn_id,
                    kind != "NEW_TASK"
                );
                let wire = value(&next);
                assert_eq!(wire["input"][0]["content"], items[0]["content"]);
                assert_eq!(
                    wire["input"].as_array().unwrap().last().unwrap()["type"],
                    "reasoning"
                );
                let replay = project(&store, transport, json!(items)).await;
                assert_eq!(
                    next_identity.turn_id,
                    replay.resolved_identity.as_ref().unwrap().turn_id
                );
            }
        }
    }
}
