use super::*;

#[tokio::test]
async fn agent_compaction_retains_native_tasks_and_peer_mail_in_reconstructed_deltas() {
    for lite in [false, true] {
        let (_temp, store) = store();
        let mut items = Vec::new();
        for (author, recipient, kind) in [
            ("/root", "/root/worker", "NEW_TASK"),
            ("/root/worker", "/root", "MESSAGE"),
            ("/root/worker", "/root", "CHANNEL_POST"),
            ("/root/worker", "/root/peer", "MESSAGE"),
            ("/root/worker", "/root", "FINAL_ANSWER"),
        ] {
            items.push(
                json!({"type":"agent_message","author":author,"recipient":recipient,
                "content":[{"type":"input_text","text":format!("Message Type: {kind}\nPayload:\n")},
                {"type":"encrypted_content","encrypted_content":"synthetic-ciphertext"}]}),
            );
        }
        let expected = vec![items[0].clone(), items[3].clone()];
        let first = prepare(&store, compact_request(items, lite)).await.unwrap();
        let response = finish(&store, first, vec![compacted("agent checkpoint")], true).await;
        let cached =
            history(&store, &response).expect("agent compaction publishes a complete window");
        let cached_items = cached.values();
        let agents: Vec<_> = cached_items
            .iter()
            .filter(|item| item["type"] == "agent_message")
            .collect();
        assert_eq!(agents.len(), expected.len());
        for (actual, expected) in agents.into_iter().zip(&expected) {
            assert_caller_item_with_generated_metadata(actual, expected);
        }
        for round in 0..2 {
            let prepared = prepare(&store, delta(&response, vec![input("continue")]))
                .await
                .unwrap();
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            let wire_agents: Vec<_> = wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "agent_message")
                .collect();
            assert_eq!(wire_agents.len(), expected.len());
            for (actual, expected) in wire_agents.into_iter().zip(&expected) {
                assert_eq!(actual["content"], expected["content"]);
                assert_eq!(actual["author"], expected["author"]);
                assert_eq!(actual["recipient"], expected["recipient"]);
            }
            publish(
                &store,
                prepared,
                &format!("resp_agent_delta_{round}"),
                json!([]),
            )
            .await;
        }
    }
}
