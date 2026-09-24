use super::integration_support::{call_core_with_headers, subscription_state};
use super::*;
use crate::test_support::spawn_loopback;
use axum::extract::State as AxumState;
use axum::routing::post;
use bytes::Bytes;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;

async fn context_upstream(
    AxumState(captures): AxumState<Arc<Mutex<Vec<Value>>>>,
    body: Bytes,
) -> Response<Body> {
    let decoded = zstd::stream::decode_all(body.as_ref()).expect("Subscription zstd");
    let value: Value = serde_json::from_slice(&decoded).expect("upstream JSON");
    assert!(
        value.get("previous_response_id").is_none(),
        "HTTP must carry full context"
    );
    let mut captures = captures.lock().await;
    captures.push(value);
    let id = format!("resp_context_{}", captures.len());
    let output = json!({"type":"message","id":format!("msg_output_{}",captures.len()),"role":"assistant","status":"completed","content":[{"type":"output_text","text":"answer"}]});
    let events = [
        json!({"type":"response.created","response":{"id":id}}),
        json!({"type":"response.output_item.done","output_index":0,"item":output}),
        json!({"type":"response.completed","response":{"id":id,"output":[output]}}),
    ];
    let sse = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(sse))
        .unwrap()
}

fn user(text: &str) -> Value {
    json!({"role":"user","content":text})
}

async fn request(state: &AppState, account: &str, value: Value, headers: HeaderMap) -> Value {
    let response = call_core_with_headers(
        state,
        account,
        Bytes::from(serde_json::to_vec(&value).unwrap()),
        headers,
    )
    .await
    .expect("Subscription response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("public JSON response")
}

#[tokio::test]
async fn http_full_delta_normal_and_lite_reconstruct_exact_parent_and_reconcile_output_once() {
    for (model, native_lite) in [
        ("gpt-5.4", false),
        ("gpt-5.6-sol", false),
        ("gpt-5.6-sol", true),
    ] {
        let captures = Arc::new(Mutex::new(Vec::new()));
        let upstream = spawn_loopback(
            Router::new()
                .route("/responses", post(context_upstream))
                .with_state(captures.clone()),
        )
        .await;
        let (state, account, _temp) = subscription_state(&upstream.base_url).await;
        let input = if native_lite {
            json!([
                {"type":"additional_tools","role":"developer","tools":[]},
                {"role":"developer","content":"native base"}, user("first")
            ])
        } else {
            json!([user("first")])
        };
        let first = request(
            &state,
            &account,
            json!({"model":model,"instructions":" caller base ","input":input,"stream":false}),
            HeaderMap::new(),
        )
        .await;
        let second = request(&state, &account, json!({"model":model,"previous_response_id":first["id"],"input":[user("first")],"stream":false}), HeaderMap::new()).await;
        assert_ne!(first["id"], second["id"]);
        // Fork from the earlier response, preserving intentional repeated text as appended input.
        request(&state, &account, json!({"model":model,"previous_response_id":first["id"],"input":[user("fork")],"stream":false}), HeaderMap::new()).await;
        let captures = captures.lock().await;
        for index in [1, 2] {
            let items = captures[index]["input"].as_array().unwrap();
            assert_eq!(
                items.iter().filter(|i| i["role"] == "assistant").count(),
                1,
                "output was duplicated"
            );
            assert_eq!(
                items.iter().filter(|i| i["role"] == "user").count(),
                2,
                "parent selection lost or duplicated input"
            );
            if model == "gpt-5.6-sol" {
                assert_eq!(
                    items
                        .iter()
                        .filter(|i| i["type"] == "additional_tools")
                        .count(),
                    1
                );
                assert_eq!(
                    items
                        .iter()
                        .filter(|i| i["role"] == "developer" && i["type"] == "message")
                        .count(),
                    if native_lite { 2 } else { 0 }
                );
            }
        }
        assert_eq!(
            captures[2]["input"].as_array().unwrap().last().unwrap()["content"][0]["text"],
            "fork"
        );
    }
}

#[tokio::test]
async fn anonymous_prefix_association_excludes_explicit_sessions_and_scopes_and_uses_completed_output()
 {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    let mut assistant = first["output"][0].clone();
    assistant.as_object_mut().unwrap().remove("id");
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first"),assistant,user("second")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    let mut explicit = HeaderMap::new();
    explicit.insert("session-id", "named".parse().unwrap());
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first"),assistant,user("third")],"stream":false}),
        explicit,
    )
    .await;
    let mut other_scope = HeaderMap::new();
    other_scope.insert(
        PSEUDONYM_SCOPE_HEADER,
        "psn_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBA"
            .parse()
            .unwrap(),
    );
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first"),assistant,user("fourth")],"stream":false}),
        other_scope,
    )
    .await;
    let captures = captures.lock().await;
    let session = |index: usize| &captures[index]["client_metadata"]["session_id"];
    assert_eq!(session(0), session(1));
    assert_ne!(session(0), session(2));
    assert_ne!(session(0), session(3));
    assert_ne!(
        captures[0]["client_metadata"]["turn_id"],
        captures[1]["client_metadata"]["turn_id"]
    );
}

#[tokio::test]
async fn expired_http_context_rejects_delta_before_delivery_and_accepts_a_full_rebuild() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    state
        .vault
        .request_state()
        .contexts
        .inner
        .lock()
        .unwrap()
        .ttl = Duration::ZERO;
    let failure = call_core_with_headers(
        &state,
        &account,
        Bytes::from(
            serde_json::to_vec(&json!({
                "model":"gpt-5.4","previous_response_id":first["id"],"input":[user("second")]
            }))
            .unwrap(),
        ),
        HeaderMap::new(),
    )
    .await
    .expect_err("expired history");
    assert!(matches!(failure, CoreFailure::StateUnavailable));
    assert_eq!(captures.lock().await.len(), 1);
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("rebuild")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(captures.lock().await.len(), 2);
}

#[path = "server_native_metadata_tests.rs"]
mod native_metadata;

#[path = "server_routing_token_tests.rs"]
mod routing_tokens;

#[tokio::test]
async fn aggregated_http_retains_item_events_when_terminal_omits_output() {
    let captures = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = spawn_loopback(Router::new().route("/responses", post({
        let captures = captures.clone();
        move |body: Bytes| {
            let captures = captures.clone();
            async move {
                let decoded = zstd::stream::decode_all(body.as_ref()).unwrap();
                let value: Value = serde_json::from_slice(&decoded).unwrap();
                let mut captures = captures.lock().await;
                captures.push(value);
                let id = format!("resp_event_only_{}", captures.len());
                let events = [
                    json!({"type":"response.created","response":{"id":id}}),
                    json!({"type":"response.metadata","headers":{"x-codex-turn-state":"event-only-token"}}),
                    json!({"type":"response.output_item.done","output_index":0,
                        "item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"event answer"}]}}),
                    json!({"type":"response.completed","response":{"id":id}}),
                ];
                let sse = events.into_iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
                Response::builder().header("content-type","text/event-stream").body(Body::from(sse)).unwrap()
            }
        }
    }))).await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","instructions":"base",
        "input":[user("first")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","instructions":"base",
        "previous_response_id":first["id"],"input":[],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    let captures = captures.lock().await;
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[1]["input"].as_array().unwrap().len(), 2);
    assert_eq!(
        captures[1]["input"][1]["content"][0]["text"],
        "event answer"
    );
    assert!(
        captures[1]["client_metadata"]
            .get("x-codex-turn-state")
            .is_none()
    );
    assert!(captures[1].get("previous_response_id").is_none());
}

#[path = "server_native_fidelity_tests.rs"]
mod native_fidelity;
