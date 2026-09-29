use super::*;
use crate::tool_observation_budget::{self as budget, META};

fn large_store() -> (tempfile::TempDir, RequestStateStore) {
    let (temp, mut store) = store();
    let limits = std::sync::Arc::make_mut(&mut store.contexts.limits);
    limits.global_bytes = 1024 * 1024 * 1024;
    limits.key_bytes = 768 * 1024 * 1024;
    limits.session_bytes = 512 * 1024 * 1024;
    (temp, store)
}

fn observed(text: &str, bytes: usize) -> Value {
    json!({"id":format!("msg_{text}"),"role":"user","content":text,META:{"executed_tool_calls":[{
        "name":"tools.probe","arguments":{},"tool_result_metadata":{"large":"x".repeat(bytes)}}],
        "tool_calls_complete":true}})
}

#[tokio::test]
async fn explicit_ws_delta_budgets_known_full_prefix_and_preserves_remote_only_history() {
    for cached in [true, false] {
        for suffix_bytes in [1024, 1024 * 1024] {
            let (_temp, store) = large_store();
            let socket = store.contexts.open_socket().unwrap();
            let first = super::repair_tests::websocket_request(
                &store,
                request(json!([observed("prefix", 1536 * 1024)])),
                &socket.id,
                None,
            )
            .await
            .unwrap();
            let identity = first.resolved_identity.clone().unwrap();
            let response = publish(&store, first, "resp_observed_prefix", json!([])).await;
            if !cached {
                let mut inner = store.contexts.inner.lock().unwrap();
                let scope = inner
                    .scopes
                    .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
                    .unwrap();
                scope
                    .records
                    .get_mut(response["id"].as_str().unwrap())
                    .unwrap()
                    .history = None;
            }
            let mut next = request(json!([observed("suffix", suffix_bytes)]));
            next["previous_response_id"] = response["id"].clone();
            let prepared =
                super::repair_tests::websocket_request(&store, next, &socket.id, Some(&identity))
                    .await
                    .unwrap();
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            let rebuild = cached && suffix_bytes == 1024 * 1024;
            assert_eq!(prepared.rebuilt_reference, rebuild);
            assert_eq!(wire.get("previous_response_id").is_none(), rebuild);
            let items = wire["input"].as_array().unwrap();
            assert_eq!(items.len(), if rebuild { 2 } else { 1 });
            assert!(
                items.iter().map(budget::observation_bytes).sum::<usize>() <= budget::PROMPT_BYTES
            );
            if rebuild {
                assert!(
                    items[0][META]["executed_tool_calls"][0]
                        .pointer("/tool_result_metadata/large")
                        .is_none()
                );
                assert_eq!(items[0][META]["tool_calls_complete"], true);
            }
        }
    }
}

#[tokio::test]
async fn http_message_budget_ignores_removed_configuration_bytes() {
    let (_temp, store) = large_store();
    let mut item = observed("body", 0);
    item[META]["executed_tool_calls"][0]["arguments"] = json!({"data":"a".repeat(4096)});
    let body = request(json!([
        {"type":"configuration_update","reasoning":{"effort":"x".repeat(budget::MESSAGE_BYTES)}},
        item
    ]));
    let prepared = prepare(&store, body).await.unwrap();
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    let items = wire["input"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0][META]["tool_calls_complete"], true);
    assert_eq!(
        items[0][META]["executed_tool_calls"][0]["arguments"]["data"]
            .as_str()
            .unwrap()
            .len(),
        4096
    );
    assert!(prepared.body.len() < 8192);
}
