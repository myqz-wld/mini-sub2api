use super::*;
use serde_json::json;

const START: i64 = 1_700_000_000_123;

fn request(turn: &str, start: Option<Value>) -> Value {
    let mut metadata = json!({"turn_id": turn});
    if let Some(start) = start {
        metadata["turn_started_at_unix_ms"] = start;
    }
    json!({"model":"gpt-5.4","input":"Synthetic turn timestamp observation",
        "client_metadata":{"session_id":"timestamp-session",
            "x-codex-turn-metadata":metadata.to_string()}})
}

async fn prepare_transport(
    store: &RequestStateStore,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Value,
) -> PreparedEmulatedRequest {
    prepare_identity_request(
        UpstreamProfile::CodexSubscription1560,
        transport,
        headers,
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
    .expect("timestamp request preparation")
}

fn timestamp(prepared: &PreparedEmulatedRequest) -> Option<i64> {
    let body = turn_metadata(&value(prepared));
    let header: Value =
        serde_json::from_str(prepared.headers["x-codex-turn-metadata"].to_str().unwrap()).unwrap();
    assert_eq!(
        body["turn_started_at_unix_ms"],
        header["turn_started_at_unix_ms"]
    );
    body["turn_started_at_unix_ms"].as_i64()
}

#[tokio::test]
async fn caller_turn_start_survives_correction_omission_restart_and_new_turn() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let (temp, store) = store();
        for (supplied, expected) in [
            (Some(json!(START)), START),
            (None, START),
            (Some(json!(START + 100)), START + 100),
            (None, START + 100),
        ] {
            let prepared = prepare_transport(
                &store,
                transport,
                &HeaderMap::new(),
                request("turn-one", supplied),
            )
            .await;
            assert_eq!(timestamp(&prepared), Some(expected));
        }
        let reopened = RequestStateStore::new(temp.path().join("accounts"));
        let repeated = prepare_transport(
            &reopened,
            transport,
            &HeaderMap::new(),
            request("turn-one", None),
        )
        .await;
        assert_eq!(timestamp(&repeated), Some(START + 100));
        let before = chrono::Utc::now().timestamp_millis();
        let next = prepare_transport(
            &reopened,
            transport,
            &HeaderMap::new(),
            request("turn-two", None),
        )
        .await;
        assert!(
            (before..=chrono::Utc::now().timestamp_millis()).contains(&timestamp(&next).unwrap())
        );
        let explicit = prepare_transport(
            &reopened,
            transport,
            &HeaderMap::new(),
            request("turn-two", Some(json!(START + 200))),
        )
        .await;
        assert_eq!(timestamp(&explicit), Some(START + 200));
        let root = prepare_transport(
            &reopened,
            transport,
            &HeaderMap::new(),
            request("turn-one", None),
        )
        .await;
        assert_eq!(timestamp(&root), Some(START + 100));
    }
}

#[tokio::test]
async fn turn_start_uses_body_then_http_only_header_fallback() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for body in [
            None,
            Some(json!({})),
            Some(json!({"turn_started_at_unix_ms":START})),
        ] {
            let (_temp, store) = store();
            let mut input = json!({"model":"gpt-5.4","input":"Synthetic carrier observation"});
            if let Some(body) = &body {
                input["client_metadata"] = json!({"x-codex-turn-metadata":body.to_string()});
            }
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-turn-metadata",
                json!({
                    "turn_id":"header-turn","turn_started_at_unix_ms":START + 1,
                })
                .to_string()
                .parse()
                .unwrap(),
            );
            let before = chrono::Utc::now().timestamp_millis();
            let prepared = prepare_transport(&store, transport, &headers, input).await;
            let actual = timestamp(&prepared).unwrap();
            if body
                .as_ref()
                .is_some_and(|body| body.get("turn_started_at_unix_ms").is_some())
            {
                assert_eq!(actual, START);
            } else if body.is_none() && transport == EmulationTransport::Http {
                assert_eq!(actual, START + 1);
            } else {
                assert!((before..=chrono::Utc::now().timestamp_millis()).contains(&actual));
            }
        }
    }
}

#[tokio::test]
async fn turn_start_ignores_invalid_values_and_preserves_integer_boundaries() {
    let (_temp, store) = store();
    let first = prepare(&store, &HeaderMap::new(), request("turn", None)).await;
    let fallback = timestamp(&first).unwrap();
    for invalid in [
        Value::Null,
        json!(-1),
        json!(1.5),
        json!("1700000000123"),
        json!(true),
        json!([]),
        json!({}),
        json!(u64::MAX),
    ] {
        let prepared = prepare(&store, &HeaderMap::new(), request("turn", Some(invalid))).await;
        assert_eq!(timestamp(&prepared), Some(fallback));
    }
    for valid in [0, START, i64::MAX] {
        let prepared = prepare(
            &store,
            &HeaderMap::new(),
            request("turn", Some(json!(valid))),
        )
        .await;
        assert_eq!(timestamp(&prepared), Some(valid));
        let repeated = prepare(&store, &HeaderMap::new(), request("turn", None)).await;
        assert_eq!(timestamp(&repeated), Some(valid));
    }
}

#[tokio::test]
async fn turn_start_stays_absent_for_memory_and_prewarm() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for kind in ["memory", "prewarm"] {
            let (_temp, store) = store();
            let metadata =
                json!({"request_kind":kind,"turn_id":"", "turn_started_at_unix_ms":START});
            let input = json!({"model":"gpt-5.4","input":[],"generate":false,
                "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}});
            let prepared = prepare_transport(&store, transport, &HeaderMap::new(), input).await;
            assert_eq!(timestamp(&prepared), None);
        }
    }
}

#[tokio::test]
async fn child_turn_start_does_not_replace_parent_time() {
    let (_temp, store) = store();
    let root = request("root-turn", Some(json!(START)));
    prepare(&store, &HeaderMap::new(), root).await;
    let metadata = json!({"turn_id":"child-turn","root_turn_id":"root-turn",
        "parent_turn_id":"root-turn","parent_thread_id":"timestamp-session",
        "turn_started_at_unix_ms":START + 1});
    let child = json!({"model":"gpt-5.4","input":"Synthetic child task",
        "client_metadata":{"session_id":"timestamp-session","thread_id":"child-thread",
            "x-codex-turn-metadata":metadata.to_string()}});
    let prepared = prepare(&store, &HeaderMap::new(), child).await;
    assert_eq!(timestamp(&prepared), Some(START + 1));
    let root = prepare(&store, &HeaderMap::new(), request("root-turn", None)).await;
    assert_eq!(timestamp(&root), Some(START));
}
