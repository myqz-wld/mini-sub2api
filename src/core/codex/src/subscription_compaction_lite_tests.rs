use super::*;

#[tokio::test]
async fn in_band_lite_compaction_retains_the_actual_optional_setup_block() {
    for developers in 0..=2 {
        let (_temp, store) = store();
        let mut source = vec![
            json!({"type":"additional_tools","role":"developer","tools":[
                {"type":"function","name":"probe","parameters":{"type":"object","properties":{}}}]
            }),
        ];
        for _ in 0..developers {
            source.push(message("developer", "caller prefix kept exactly"));
        }
        source.push(input("covered user"));
        let mut first = request(json!(source));
        first.as_object_mut().unwrap().remove("instructions");
        let first = prepare(&store, first).await.unwrap();
        let response = finish(
            &store,
            first,
            vec![
                compacted("inline Lite checkpoint"),
                message("assistant", "retained answer"),
            ],
            true,
        )
        .await;
        let cached = history(&store, &response).expect("Lite checkpoint preserves setup");
        assert_eq!(cached.len, developers + 3);
        let values = cached.values();
        assert_eq!(values[0]["type"], "additional_tools");
        for item in &values[1..=developers] {
            assert_caller_item_with_generated_metadata(
                item,
                &message("developer", "caller prefix kept exactly"),
            );
        }
        let mut next = delta(&response, vec![input("new user")]);
        next.as_object_mut().unwrap().remove("instructions");
        let prepared = prepare(&store, next).await.unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(wire.get("instructions").is_none() && wire.get("tools").is_none());
        let input = wire["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "additional_tools");
        assert_eq!(input[0]["tools"][0]["name"], "probe");
        assert_eq!(
            input
                .iter()
                .filter(|item| item["role"] == "developer" && item["type"] == "message")
                .count(),
            developers
        );
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "compaction")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn tool_free_lite_compaction_recovers_developer_only_and_empty_setup_on_http_and_ws() {
    for developers in 0..=2 {
        for websocket in [false, true] {
            let (_temp, store) = store();
            let setup: Vec<_> = (0..developers)
                .map(|_| message("developer", "caller setup"))
                .collect();
            let mut source = setup.clone();
            source.push(input("covered input"));
            let body = json!({"model":"gpt-6-astra","input":source});
            let first = prepare(&store, body).await.unwrap();
            let response = finish(
                &store,
                first,
                vec![compacted("checkpoint"), message("assistant", "after")],
                true,
            )
            .await;
            let cached = history(&store, &response).unwrap();
            assert_eq!(cached.len, developers + 2);
            let next = json!({"model":"gpt-6-astra","previous_response_id":response["id"],"input":[input("next")]});
            let prepared = if websocket {
                let socket = store.contexts.open_socket().unwrap();
                repair_tests::websocket_request(&store, next, &socket.id, None)
                    .await
                    .unwrap()
            } else {
                prepare(&store, next).await.unwrap()
            };
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            let items = wire["input"].as_array().unwrap();
            assert_eq!(items.len(), developers + 3);
            assert!(items.iter().all(|v| v["type"] != "additional_tools"));
            assert!(
                !items
                    .iter()
                    .any(|v| v["content"][0]["text"] == "covered input")
            );
            for (actual, expected) in items.iter().zip(&setup) {
                assert_caller_item_with_generated_metadata(actual, expected);
            }
        }
    }
}

fn declaration(id: &str, description: &str) -> Value {
    json!({"type":"additional_tools","id":id,"role":"developer","tools":[
        {"type":"namespace","name":"synthetic","description":description,"tools":[
            {"type":"function","name":"probe","description":"","strict":false,
                "parameters":{"type":"object","properties":{}}}]}]})
}

#[tokio::test]
async fn incremental_declarations_and_opaque_notices_survive_full_delta_and_compaction() {
    for explicit in [false, true] {
        for websocket in [false, true] {
            let (_temp, store) = store();
            let socket = store.contexts.open_socket().unwrap();
            let notice = message(
                "developer",
                "The following tools are no longer available. Do not call them:\nsynthetic.probe\nOpaque {{caller}} notice",
            );
            let setup = vec![
                declaration("at_first", "first"),
                message("developer", "base"),
                declaration("at_changed", "changed"),
                notice.clone(),
            ];
            let first = json!({"model":"gpt-6-astra","input":[setup[0],setup[1],input("first")]});
            let first = repair_tests::websocket_request(&store, first, &socket.id, None)
                .await
                .unwrap();
            let identity = first.resolved_identity.clone().unwrap();
            let response = finish_as(&store, first, vec![], true, "resp_first").await;
            let next = json!({"model":"gpt-6-astra","previous_response_id":response["id"],
                "input":[setup[2],notice,input("covered input")]});
            let updated =
                repair_tests::websocket_request(&store, next, &socket.id, Some(&identity))
                    .await
                    .unwrap();
            let wire: Value = serde_json::from_slice(&updated.body).unwrap();
            assert_eq!(wire["input"].as_array().unwrap().len(), 3);
            assert_eq!(wire["input"][0]["tools"][0]["description"], "changed");
            assert_eq!(wire["input"][1]["content"], notice["content"]);
            let response = finish_as(&store, updated, vec![], true, "resp_changed").await;
            let mut compact =
                json!({"model":"gpt-6-astra","previous_response_id":response["id"],"input":[]});
            if explicit {
                compact["input"] = json!([{"type":"compaction_trigger"}]);
            }
            let compact = prepare(&store, compact).await.unwrap();
            let response =
                finish(&store, compact, vec![compacted("updated checkpoint")], true).await;
            let cached = history(&store, &response).unwrap().values();
            let config: Vec<_> = cached
                .iter()
                .filter(|v| v["type"] == "additional_tools" || v["role"] == "developer")
                .collect();
            assert_eq!(config.len(), setup.len());
            for (actual, expected) in config.iter().zip(&setup) {
                assert_caller_item_with_generated_metadata(actual, expected);
            }
            let next = json!({"model":"gpt-6-astra","previous_response_id":response["id"],"input":[input("after compact")]});
            let prepared = if websocket {
                let reconnect = store.contexts.open_socket().unwrap();
                repair_tests::websocket_request(&store, next, &reconnect.id, None)
                    .await
                    .unwrap()
            } else {
                prepare(&store, next).await.unwrap()
            };
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert!(wire.get("previous_response_id").is_none());
            assert_eq!(
                wire["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|v| v["type"] == "additional_tools")
                    .count(),
                2
            );
            assert!(
                wire["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["content"] == notice["content"])
            );
        }
    }
}

#[tokio::test]
async fn malformed_incremental_declarations_and_unresolved_compaction_fail_closed() {
    for bad in [
        json!({"role":"user","tools":[]}),
        json!({"role":"developer","tools":null}),
        json!({"role":"developer","tools":[{"type":"function","name":""}]}),
    ] {
        let (_temp, store) = store();
        let mut bad = bad;
        bad["type"] = "additional_tools".into();
        assert!(
            prepare(
                &store,
                json!({"model":"gpt-6-astra","input":[input("x"),bad]})
            )
            .await
            .is_err()
        );
    }
    let (_temp, store) = store();
    let first = prepare(
        &store,
        json!({"model":"gpt-6-astra","input":[input("x"),
        {"type":"function_call","call_id":"call_pending","name":"probe","arguments":"{}"}]}),
    )
    .await
    .unwrap();
    let response = finish(&store, first, vec![compacted("unresolved")], true).await;
    assert!(history(&store, &response).is_none());
    assert!(
        prepare(
            &store,
            json!({"model":"gpt-6-astra","previous_response_id":response["id"],"input":[]})
        )
        .await
        .is_err()
    );
}
