use super::*;
use std::time::{Duration, Instant};

fn agent(kind: &str) -> Value {
    json!({"type":"agent_message","id":"amsg_task","author":"/root","recipient":"/root/worker",
        "content":[{"type":"input_text","text":format!("Message Type: {kind}\nPayload:\n")},
        {"type":"encrypted_content","encrypted_content":"synthetic-agent-cipher"}]})
}

#[tokio::test]
async fn agent_tasks_start_turns_without_splitting_pending_tool_calls_or_progress() {
    for kind in ["NEW_TASK", "MESSAGE", "FINAL_ANSWER", "CHANNEL_POST"] {
        for pending_call in [false, true] {
            let (_temp, store) = store();
            let first = prepare(&store, request(json!([agent("NEW_TASK")])))
                .await
                .unwrap();
            let identity = first.resolved_identity.as_ref().unwrap().clone();
            let output = if pending_call {
                json!([{"type":"function_call","id":"fc_task","call_id":"call_task","name":"probe","arguments":"{}"}])
            } else {
                json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]}])
            };
            let previous = publish(&store, first, "resp_agent_task", output).await;
            let mut message = agent(kind);
            message["id"] = "amsg_followup".into();
            let mut body = request(json!([message]));
            body["previous_response_id"] = previous["id"].clone();
            let next = prepare(&store, body).await.unwrap();
            let current = next.resolved_identity.as_ref().unwrap();
            assert_eq!(current.thread_id, identity.thread_id);
            assert_eq!(
                current.turn_id == identity.turn_id,
                pending_call || kind != "NEW_TASK"
            );
        }
    }
}

#[tokio::test]
async fn agent_led_expired_history_import_still_enforces_ownership_and_content_shape() {
    for case in [
        "expired",
        "retained",
        "unrelated",
        "malformed",
        "missing-cipher",
        "open-call",
    ] {
        let (_temp, store) = store();
        let first = prepare(&store, request(json!([agent("NEW_TASK")])))
            .await
            .unwrap();
        let old = first.resolved_identity.as_ref().unwrap().clone();
        let response = publish(
            &store,
            first,
            "resp_agent_import",
            json!([
                {"type":"message","id":"msg_agent_answer","role":"assistant",
                 "content":[{"type":"output_text","text":"synthetic answer"}],
                 "internal_chat_message_metadata_passthrough":{"turn_id":old.turn_id}}
            ]),
        )
        .await;
        if case != "retained" {
            store
                .contexts
                .inner
                .lock()
                .unwrap()
                .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
        }
        let mut body = request(json!([
            agent("NEW_TASK"),
            response["output"][0],
            input("continue")
        ]));
        match case {
            "retained" => body["input"][0]["content"][0]["text"] = "different task".into(),
            "unrelated" => body["client_metadata"] = json!({"session_id":"unrelated-session"}),
            "malformed" => body["input"][0]["recipient"] = Value::Null,
            "missing-cipher" => body["input"].as_array_mut().unwrap().push(json!({"type":"reasoning","summary":[]})),
            "open-call" => body["input"].as_array_mut().unwrap().push(json!({"type":"function_call","call_id":"call_open","name":"probe","arguments":"{}"})),
            _ => {}
        }
        let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        let result = prepare(&store, body).await;
        if case == "expired" {
            let prepared = result.unwrap();
            assert_ne!(
                prepared.resolved_identity.as_ref().unwrap().thread_id,
                old.thread_id
            );
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert_eq!(wire["input"][0]["content"], agent("NEW_TASK")["content"]);
        } else {
            assert!(result.is_err(), "agent import unexpectedly accepted {case}");
            assert_eq!(
                std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
                before
            );
        }
    }
}
