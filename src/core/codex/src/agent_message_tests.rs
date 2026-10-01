use super::*;
use serde_json::json;

fn agent(author: &str, recipient: &str, content: Value) -> Value {
    json!({"type":"agent_message","author":author,"recipient":recipient,"content":content})
}

#[test]
fn only_visible_new_tasks_signal_an_implicit_turn() {
    for (text, starts) in [
        ("Message Type: NEW_TASK\nPayload:\n", true),
        ("Message Type: MESSAGE\nPayload:\n", false),
        ("Message Type: FINAL_ANSWER\nPayload:\n", false),
        ("Message Type: CHANNEL_POST\nPayload:\n", false),
        ("Message Type: NEW_TASK_OTHER\n", false),
        ("quoted Message Type: NEW_TASK\n", false),
    ] {
        let item = agent(
            "/root",
            "/root/worker",
            json!([
                {"type":"input_text","text":text},
                {"type":"encrypted_content","encrypted_content":"synthetic-ciphertext"}
            ]),
        );
        assert_eq!(starts_turn(&item), starts);
    }
    let encrypted = agent(
        "/root",
        "/root/worker",
        json!([
            {"type":"encrypted_content","encrypted_content":"Message Type: NEW_TASK\n"}
        ]),
    );
    assert!(!starts_turn(&encrypted));
}

#[test]
fn malformed_agent_items_cannot_qualify_as_native_input() {
    let valid = agent(
        "/root",
        "/root/worker",
        json!([
            {"type":"input_text","text":"Message Type: NEW_TASK\n"}
        ]),
    );
    for field in ["author", "recipient", "content"] {
        for value in [Value::Null, json!(17), json!({})] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(AgentMessage::read(&invalid).is_none());
            assert!(!starts_turn(&invalid));
        }
    }
    for content in [
        json!([{"type":"output_text","text":"unexpected"}]),
        json!([{"type":"input_text","text":null}]),
        json!([{"type":"encrypted_content"}]),
        json!([{"type":"input_image","image_url":"synthetic"}]),
    ] {
        assert!(AgentMessage::read(&agent("/root", "/root/worker", content)).is_none());
    }
}

#[test]
fn compaction_preserves_tasks_and_peer_mail_but_drops_descendant_progress_and_finals() {
    for (author, recipient, kind, retained) in [
        ("/root", "/root/worker", "NEW_TASK", true),
        ("/root/worker", "/root", "MESSAGE", false),
        ("/root/worker", "/root", "CHANNEL_POST", false),
        ("/root/worker", "/root/peer", "MESSAGE", true),
        ("/root/worker", "/root/work", "MESSAGE", true),
        ("/root", "/root/worker", "MESSAGE", true),
        ("/root/worker", "/root/peer", "FINAL_ANSWER", false),
        ("/root/worker", "/root", "FINAL_ANSWER", false),
    ] {
        let item = agent(
            author,
            recipient,
            json!([
                {"type":"input_text","text":format!("Message Type: {kind}\nPayload:\n")},
                {"type":"encrypted_content","encrypted_content":"synthetic-ciphertext"}
            ]),
        );
        assert_eq!(
            AgentMessage::read(&item).unwrap().retain_for_compaction(),
            retained
        );
    }
}

#[test]
fn compaction_agent_budget_counts_utf8_routing_and_estimated_ciphertext() {
    // Routing consumes 17 bytes. The native per-agent bound is 40,000 visible bytes.
    for (text, retained) in [
        ("x".repeat(39_983), true),
        ("x".repeat(39_984), false),
        (format!("{}x", "é".repeat(19_991)), true),
        ("é".repeat(19_992), false),
    ] {
        let item = agent(
            "/root",
            "/root/worker",
            json!([{"type":"input_text","text":text}]),
        );
        assert_eq!(
            AgentMessage::read(&item).unwrap().retain_for_compaction(),
            retained
        );
    }
    for (encoded_bytes, retained) in [(71_080, true), (71_081, false)] {
        let item = agent(
            "/root",
            "/root/worker",
            json!([
                {"type":"encrypted_content","encrypted_content":"x".repeat(encoded_bytes)}
            ]),
        );
        assert_eq!(
            AgentMessage::read(&item).unwrap().retain_for_compaction(),
            retained
        );
    }
}
