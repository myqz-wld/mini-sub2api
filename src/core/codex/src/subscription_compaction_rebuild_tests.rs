use super::*;

// Codex 0.159.2 compact.rs rebuilds media/truncated user messages while retaining
// their IDs. A complete client replacement must supersede the cached old body.
#[tokio::test]
async fn compacted_same_id_messages_replace_content_and_survive_continuation() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for lite in [false, true] {
            for remote in [false, true] {
                for media in [false, true] {
                    let (_temp, store) = store();
                    let text = "keep thirteen";
                    let before = if media {
                        json!([
                            {"type":"input_image","image_url":"data:image/png;base64,c3ludGhldGlj"},
                            {"type":"input_audio","audio_url":"data:audio/wav;base64,c3ludGhldGlj"},
                            {"type":"input_image","image_url":"data:image/png;base64,c3ludGhldGlj"},
                            {"type":"input_text","text":text}
                        ])
                    } else {
                        json!([{"type":"input_text","text":"word ".repeat(64)}])
                    };
                    let after = json!([{"type":"input_text","text":if media {
                        text
                    } else {
                        "word…75 tokens truncated…word"
                    }}]);
                    let mut original = message("user", text);
                    original["id"] = "msg_retained_user".into();
                    original["content"] = before;
                    original["internal_chat_message_metadata_passthrough"] =
                        json!({"turn_id":"source-turn","create_time":1.0});
                    let seed = rebuild_request(vec![original.clone()], lite, "source-turn");
                    let seed = prepare_transport(&store, seed, transport).await;
                    let wire: Value = serde_json::from_slice(&seed.body).unwrap();
                    let upstream_id = wire["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|item| item["role"] == "user")
                        .unwrap()["id"]
                        .clone();
                    let seed_identity = seed.resolved_identity.as_ref().unwrap().clone();
                    let response = finish_as(
                        &store,
                        seed,
                        vec![message("assistant", "old assistant")],
                        true,
                        "resp_before_rebuild",
                    )
                    .await;

                    let mut items = vec![original.clone(), response["output"][0].clone()];
                    if remote {
                        items.push(json!({"type":"compaction_trigger"}));
                    }
                    let mut compact = rebuild_request(items, lite, "compact-turn");
                    compact["client_metadata"]["x-codex-turn-metadata"] = json!({
                        "request_kind":"compaction","compaction":{"implementation":if remote {
                            "responses_compaction_v2"
                        } else {
                            "responses"
                        }}
                    })
                    .to_string()
                    .into();
                    let compact = prepare_transport(&store, compact, transport).await;
                    let checkpoint = if remote {
                        compacted("synthetic checkpoint")
                    } else {
                        message("assistant", "synthetic summary")
                    };
                    let compact = finish_as(
                        &store,
                        compact,
                        vec![checkpoint],
                        true,
                        "resp_compact_rebuild",
                    )
                    .await;

                    let mut rebuilt = original;
                    rebuilt["content"] = after.clone();
                    let summary = if remote {
                        compact["output"][0].clone()
                    } else {
                        message(
                            "user",
                            "Synthetic client summary wrapper\nsynthetic summary",
                        )
                    };
                    let next = rebuild_request(
                        vec![rebuilt, summary, input("continue")],
                        lite,
                        "after-compact-turn",
                    );
                    let next = prepare_transport(&store, next, transport).await;
                    let identity = next.resolved_identity.as_ref().unwrap();
                    assert_eq!(identity.session_id, seed_identity.session_id);
                    assert_eq!(identity.thread_id, seed_identity.thread_id);
                    assert_eq!(identity.window_number, 1);
                    let rebuilt_wire: Value = serde_json::from_slice(&next.body).unwrap();
                    let rebuilt_id = rebuilt_wire["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|item| item["role"] == "user")
                        .unwrap()["id"]
                        .clone();
                    assert_ne!(
                        rebuilt_id, upstream_id,
                        "changed local content receives a new pseudonym"
                    );
                    assert_rebuilt_content(&next, &rebuilt_id, &after);
                    let next = finish_as(
                        &store,
                        next,
                        vec![message("assistant", "new answer")],
                        true,
                        "resp_after_rebuild",
                    )
                    .await;

                    // Reconnect through HTTP with only a response reference: reconstruct from
                    // the accepted replacement, without reviving the old media/text blocks.
                    let next = prepare(&store, delta(&next, vec![input("later")]))
                        .await
                        .unwrap();
                    assert_rebuilt_content(&next, &rebuilt_id, &after);
                }
            }
        }
    }
}

fn rebuild_request(items: Vec<Value>, lite: bool, turn: &str) -> Value {
    let mut body = request(json!(items));
    if lite {
        body["model"] = "gpt-5.6-sol".into();
    }
    body["client_metadata"] = json!({"session_id":"rebuild-session","turn_id":turn});
    body
}

async fn prepare_transport(
    store: &RequestStateStore,
    body: Value,
    transport: EmulationTransport,
) -> PreparedEmulatedRequest {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1592,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store,
            state_namespace: NAMESPACE,
            account_ref: OWNER,
            downstream_scope: KEY,
            fingerprint_mode: FingerprintMode::Device,
            binding: None,
            socket_id: None,
        },
        false,
    )
    .await
    .expect("client-owned replacement history must remain valid")
}

fn assert_rebuilt_content(prepared: &PreparedEmulatedRequest, id: &Value, content: &Value) {
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert!(wire.get("previous_response_id").is_none());
    let messages: Vec<_> = wire["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item.get("id") == Some(id))
        .collect();
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0]["content"] == *content,
        "rebuilt content was replaced or duplicated"
    );
}
