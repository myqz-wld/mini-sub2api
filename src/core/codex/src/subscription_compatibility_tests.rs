use super::*;

#[tokio::test]
async fn error_stream_correlation_survives_translation_and_nullable_flat_errors() {
    use crate::request_state_types::WireIdDomain;
    let (_temp, store) = store();
    let upstream = store
        .edit(NAMESPACE, OWNER, KEY, |editor| {
            editor.wire_from_downstream(WireIdDomain::Stream, "caller-stream")
        })
        .await
        .unwrap();
    let state = ResponseStateContext::new(OWNER, NAMESPACE, KEY, &store, None, None);
    for nested in [false, true] {
        let mut event = json!({"type":"error","stream_id":upstream,"error":null,
            "code":"rate_limit_exceeded","message":"synthetic-private","debug":"synthetic-private"});
        if nested {
            event["error"] =
                json!({"code":"context_length_exceeded","message":"synthetic-private"});
        }
        let translated = state.translate_value(event).await.unwrap();
        assert_eq!(translated["stream_id"], "caller-stream");
        assert!(!translated.to_string().contains("synthetic-private"));
        if nested {
            assert_eq!(translated["error"]["code"], "context_length_exceeded");
            assert!(translated.get("code").is_none());
        } else {
            assert_eq!(translated["code"], "rate_limit_exceeded");
            assert_eq!(
                translated["message"],
                crate::response_privacy::FAILURE_MESSAGE
            );
            assert!(translated.get("error").is_none());
        }
    }
}

async fn prepare_for_transport(
    store: &RequestStateStore,
    body: Value,
    headers: &HeaderMap,
    transport: EmulationTransport,
) -> PreparedEmulatedRequest {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1560,
        transport,
        headers,
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
    .unwrap()
}

#[tokio::test]
async fn partial_turn_metadata_completes_model_and_effort_without_replacing_explicit_values() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for in_header in [false, true] {
            for partial in [
                json!({}),
                json!({"model":"caller-model"}),
                json!({"reasoning_effort":"low"}),
                json!({"model":"caller-model","reasoning_effort":"low"}),
            ] {
                let (_temp, store) = store();
                let mut body = request(json!([input("metadata")]));
                body["reasoning"] = json!({"effort":"high"});
                let mut headers = HeaderMap::new();
                if in_header {
                    headers.insert(
                        "x-codex-turn-metadata",
                        partial.to_string().parse().unwrap(),
                    );
                } else {
                    body["client_metadata"] = json!({"x-codex-turn-metadata":partial.to_string()});
                }
                let prepared = prepare_for_transport(&store, body, &headers, transport).await;
                let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
                let body_metadata: Value = serde_json::from_str(
                    wire["client_metadata"]["x-codex-turn-metadata"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap();
                let header_metadata: Value = serde_json::from_str(
                    prepared.headers["x-codex-turn-metadata"].to_str().unwrap(),
                )
                .unwrap();
                // Final identity projection uses body metadata for both carriers. Header-only
                // model/effort extras do not override that existing body-authoritative policy.
                let metadata = if in_header {
                    &header_metadata
                } else {
                    &body_metadata
                };
                let model = (!in_header).then(|| partial.get("model")).flatten();
                let effort = (!in_header)
                    .then(|| partial.get("reasoning_effort"))
                    .flatten();
                assert_eq!(
                    metadata["model"],
                    model.cloned().unwrap_or(json!("gpt-5.4"))
                );
                assert_eq!(
                    metadata["reasoning_effort"],
                    effort.cloned().unwrap_or(json!("high"))
                );
                assert_eq!(wire["model"], "gpt-5.4");
                assert_eq!(wire["reasoning"]["effort"], "high");
                assert!(
                    body_metadata.get("model").is_some()
                        && body_metadata.get("reasoning_effort").is_some()
                );
                assert!(
                    header_metadata.get("model").is_some()
                        && header_metadata.get("reasoning_effort").is_some()
                );
            }
        }
    }
}

#[tokio::test]
async fn local_function_output_schema_is_omitted_from_ordinary_and_lite_wire() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-5.4", "gpt-6-astra"] {
            let (_temp, store) = store();
            let mut body = request(json!([input("tools")]));
            body["model"] = model.into();
            body["tools"] = json!([{"type":"function","name":"lookup","output_schema":{"type":"string"},
                "parameters":{"type":"object","properties":{"output_schema":{"type":"string"}}}},
                {"type":"namespace","name":"other","tools":[{"type":"function","name":"nested","output_schema":{"type":"string"}}]}]);
            let prepared = prepare_for_transport(&store, body, &HeaderMap::new(), transport).await;
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            let tools = if model == "gpt-6-astra" {
                &wire["input"][0]["tools"]
            } else {
                &wire["tools"]
            };
            fn check(tools: &Value) -> usize {
                tools
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tool| {
                        if tool["type"] == "namespace" {
                            return check(&tool["tools"]);
                        }
                        assert!(tool.get("output_schema").is_none());
                        if tool["name"] == "lookup" {
                            assert_eq!(
                                tool["parameters"]["properties"]["output_schema"]["type"],
                                "string"
                            );
                        }
                        1
                    })
                    .sum()
            }
            assert_eq!(check(tools), 2);
        }
    }
}
