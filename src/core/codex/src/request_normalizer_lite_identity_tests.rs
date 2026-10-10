use super::*;
use serde_json::json;

fn native_request(thread: &str, base: &str, tools: Value) -> Value {
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, thread.as_bytes());
    let tool_id = Uuid::new_v5(&namespace, &serde_json::to_vec(&tools).unwrap());
    let base_id = Uuid::new_v5(&namespace, base.as_bytes());
    json!({"model":"gpt-5.6-sol", "client_metadata":{"session_id":thread,"thread_id":thread},
    "input":[
        {"type":"additional_tools","id":format!("at_{tool_id}"),"tools":tools},
        {"type":"message","id":format!("msg_{base_id}"),"role":"developer",
         "content":[{"type":"input_text","text":base}]},
        {"role":"user","content":"synthetic"}
    ]})
}

fn assert_ids(body: &Value) {
    let namespace = Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        body["client_metadata"]["thread_id"]
            .as_str()
            .unwrap()
            .as_bytes(),
    );
    let tools = serde_json::to_vec(&body["input"][0]["tools"]).unwrap();
    let base = body["input"][1]["content"][0]["text"].as_str().unwrap();
    assert!(body["input"][0]["id"] == format!("at_{}", Uuid::new_v5(&namespace, &tools)));
    assert!(body["input"][1]["id"] == format!("msg_{}", Uuid::new_v5(&namespace, base.as_bytes())));
}

#[tokio::test]
async fn native_lite_prefixes_follow_projected_thread_payload_and_restart() {
    let (temp, store) = store();
    let tools = json!([
        {"type":"function","name":"a","parameters":{"type":"object","properties":{"z":{"type":"string"},"a":{"type":"string"}}}},
        {"type":"function","name":"b","parameters":{"type":"object","properties":{}}}
    ]);
    let raw = native_request("native-thread", "  基础 {{literal}}\n", tools.clone());
    let first = value(&prepare(&store, &HeaderMap::new(), raw.clone()).await);
    assert_ids(&first);
    for index in [0, 1] {
        assert!(
            first["input"][index]
                .get("internal_chat_message_metadata_passthrough")
                .is_none()
        );
    }
    let reopened = RequestStateStore::new(temp.path().join("accounts"));
    let repeated = value(&prepare(&reopened, &HeaderMap::new(), raw.clone()).await);
    for index in [0, 1] {
        assert!(first["input"][index]["id"] == repeated["input"][index]["id"]);
    }
    let changed = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            native_request("native-thread", "基础 {{literal}}\n", tools.clone()),
        )
        .await,
    );
    assert_ids(&changed);
    assert!(first["input"][0]["id"] == changed["input"][0]["id"]);
    assert!(first["input"][1]["id"] != changed["input"][1]["id"]);
    let reordered = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            native_request(
                "native-thread",
                "  基础 {{literal}}\n",
                json!([tools[1], tools[0]]),
            ),
        )
        .await,
    );
    assert_ids(&reordered);
    assert!(first["input"][0]["id"] != reordered["input"][0]["id"]);
    let other = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            native_request("another-thread", "  基础 {{literal}}\n", tools),
        )
        .await,
    );
    assert_ids(&other);
    assert!(first["input"][1]["id"] != other["input"][1]["id"]);
    let raw_id = raw["input"][0]["id"].as_str().unwrap().to_owned();
    let mapped = store
        .edit(NAMESPACE, ACCOUNT_REF, SCOPE, move |editor| {
            editor.required_wire_from_downstream(WireIdDomain::Item, &raw_id, false)
        })
        .await
        .unwrap();
    assert!(first["input"][0]["id"] == mapped);
}

#[tokio::test]
async fn native_lite_prefixes_keep_legacy_aliases_and_require_native_proof() {
    let (_temp, store) = store();
    let raw = native_request("native-thread", "base", json!([]));
    let raw_id = raw["input"][0]["id"].as_str().unwrap().to_owned();
    let legacy = store
        .edit(NAMESPACE, ACCOUNT_REF, SCOPE, move |editor| {
            editor.wire_from_downstream(WireIdDomain::Item, &raw_id)
        })
        .await
        .unwrap();
    let current = value(&prepare(&store, &HeaderMap::new(), raw).await);
    assert!(current["input"][0]["id"] == legacy);
    assert!(
        current["input"][0]
            .get("internal_chat_message_metadata_passthrough")
            .is_none()
    );
    let mut arbitrary = native_request("native-thread", "base", json!([]));
    arbitrary["input"][0]["id"] = json!("at_caller_opaque");
    arbitrary["input"][1]["id"] = json!("msg_caller_opaque");
    let first = value(&prepare(&store, &HeaderMap::new(), arbitrary.clone()).await);
    arbitrary["input"][1]["content"][0]["text"] = json!("edited caller text");
    let edited = value(&prepare(&store, &HeaderMap::new(), arbitrary).await);
    assert!(first["input"][1]["id"] != edited["input"][1]["id"]);
}

#[tokio::test]
async fn native_lite_prefixes_restore_thread_proof_from_previous_response() {
    let (_temp, store) = store();
    let mut raw = native_request("native-thread", "base", json!([]));
    let first = prepare(&store, &HeaderMap::new(), raw.clone()).await;
    let state = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        first.resolved_identity.as_ref(),
        None,
    );
    let response = state
        .translate_value(
            json!({"type":"response.completed","response":{"id":"resp_native_base","output":[]}}),
        )
        .await
        .unwrap();
    raw.as_object_mut().unwrap().remove("client_metadata");
    raw["previous_response_id"] = response["response"]["id"].clone();
    let restored = value(&prepare(&store, &HeaderMap::new(), raw).await);
    assert_ids(&restored);
    assert!(value(&first)["input"][0]["id"] == restored["input"][0]["id"]);
}

#[tokio::test]
async fn ordinary_and_tool_free_lite_base_ids_use_exact_native_thread_and_text_proof() {
    for model in ["gpt-5.5", "gpt-6-astra"] {
        for native in [false, true] {
            for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
                let harness = CodexStateTestHarness::new();
                let text = "  synthetic {{base}}\n";
                let ns = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"native-thread");
                let id = format!("msg_{}", Uuid::new_v5(&ns, text.as_bytes()));
                let developer = json!({"type":"message","role":"developer",
                    "content":[{"type":"input_text","text":text}]});
                let mut body = json!({"model":model,"client_metadata":{
                    "session_id":"native-thread","thread_id":"native-thread"},
                    "input":[developer, {"role":"user","content":"synthetic"}]});
                if native {
                    let mut base = developer.clone();
                    base["id"] = id.into();
                    body["input"].as_array_mut().unwrap().insert(0, base);
                } else {
                    body["instructions"] = text.into();
                }
                let headers = HeaderMap::new();
                let prepare = || {
                    harness.prepare(
                        UpstreamProfile::CodexSubscription1620,
                        transport,
                        &headers,
                        Bytes::from(serde_json::to_vec(&body).unwrap()),
                        1024 * 1024,
                        ACCOUNT_REF,
                        NAMESPACE,
                        SCOPE,
                    )
                };
                let first = value(&prepare().await.unwrap());
                let repeated = value(&prepare().await.unwrap());
                let projected_thread = first["client_metadata"]["thread_id"].as_str().unwrap();
                let ns = Uuid::new_v5(&Uuid::NAMESPACE_OID, projected_thread.as_bytes());
                assert_eq!(
                    first["input"][0]["id"],
                    format!("msg_{}", Uuid::new_v5(&ns, text.as_bytes()))
                );
                assert_eq!(first["input"][0], repeated["input"][0]);
                assert!(first.get("instructions").is_none());
                let metadata = first["input"][0].get("internal_chat_message_metadata_passthrough");
                if native {
                    assert!(metadata.is_none());
                } else {
                    assert_eq!(
                        metadata,
                        Some(&json!({"content_item_kinds":["model.base_instructions"]}))
                    );
                }
                assert_eq!(first["input"].as_array().unwrap().len(), 3);
                // Equal caller developer text is a distinct item, never collapsed into setup.
                assert_eq!(first["input"][1]["content"], developer["content"]);
                assert_ne!(first["input"][1]["id"], first["input"][0]["id"]);
                assert!(
                    first["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|v| v["type"] != "additional_tools")
                );
            }
        }
    }
}

#[tokio::test]
async fn native_base_classification_survives_prefix_projection_without_turn_stamps() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let harness = CodexStateTestHarness::new();
        let mut body = native_request("native-thread", "synthetic base", json!([]));
        let metadata =
            json!({"content_item_kinds":["model.base_instructions"],"opaque":{"keep":true}});
        body["input"][1]["internal_chat_message_metadata_passthrough"] = metadata.clone();
        let prepared = harness
            .prepare(
                UpstreamProfile::CodexSubscription1620,
                transport,
                &HeaderMap::new(),
                Bytes::from(serde_json::to_vec(&body).unwrap()),
                1024 * 1024,
                ACCOUNT_REF,
                NAMESPACE,
                SCOPE,
            )
            .await
            .unwrap();
        assert_eq!(
            value(&prepared)["input"][1]["internal_chat_message_metadata_passthrough"],
            metadata
        );
    }
}
