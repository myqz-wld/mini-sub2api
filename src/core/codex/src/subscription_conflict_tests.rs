use super::*;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::subscription_context::Record;

#[path = "subscription_conflict_boundary_tests.rs"]
mod boundary_tests;

#[path = "subscription_conflict_lifecycle_tests.rs"]
mod lifecycle_tests;

async fn tool_history(store: &RequestStateStore, model: &str) -> (Value, Value) {
    let mut body = request(json!([input("synthetic tool task")]));
    body["model"] = model.into();
    let seed = prepare(store, body.clone()).await.unwrap();
    let response = publish(
        store,
        seed,
        "resp_fork_seed",
        json!([
            {"type":"function_call","id":"fc_seed","call_id":"call_seed",
             "name":"tool","arguments":"{}","status":"completed"}
        ]),
    )
    .await;
    body["input"] = json!([input("synthetic tool task"), response["output"][0],
        {"type":"function_call_output","call_id":response["output"][0]["call_id"],"output":"synthetic result"}]);
    (body, response)
}

fn active(store: &RequestStateStore, prepared: &PreparedEmulatedRequest) -> Record {
    store.contexts.inner.lock().unwrap().operations[&prepared.operation.as_ref().unwrap().0.id]
        .record
        .clone()
}

fn text_output(text: &str) -> Value {
    json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}])
}

#[allow(clippy::too_many_arguments)]
async fn scoped(
    store: &RequestStateStore,
    body: Value,
    headers: &HeaderMap,
    transport: EmulationTransport,
    binding: Option<&ResolvedRequestIdentity>,
    socket: Option<&str>,
    namespace: &str,
    key: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1620,
        transport,
        headers,
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store,
            state_namespace: namespace,
            account_ref: OWNER,
            downstream_scope: key,
            fingerprint_mode: FingerprintMode::Device,
            binding,
            socket_id: socket,
        },
        false,
    )
    .await
}

pub(super) async fn prepare_websocket(
    store: &RequestStateStore,
    body: Value,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    store.contexts.socket_open("synthetic-exclusive-socket")?;
    scoped(
        store,
        body,
        &HeaderMap::new(),
        EmulationTransport::WebSocket,
        None,
        Some("synthetic-exclusive-socket"),
        NAMESPACE,
        KEY,
    )
    .await
}

#[tokio::test]
async fn anonymous_http_overlaps_fork_without_merging_content_or_routing_tokens() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        for case in ["identical", "steering", "partial", "parallel"] {
            let (_temp, store) = store();
            let (body, _) = tool_history(&store, model).await;
            let mut original = body.clone();
            let mut incoming = body;
            if case == "parallel" {
                original["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(input("synthetic task A"));
                incoming["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(input("synthetic task B"));
            } else if case != "identical" {
                let items = incoming["input"].as_array_mut().unwrap();
                if case == "partial" {
                    items.push(json!({"role":"assistant","content":"synthetic partial answer"}));
                }
                items.extend([
                    input("[Request interrupted by user]"),
                    input("synthetic correction"),
                ]);
            }
            let first = prepare(&store, original).await.unwrap();
            let first_record = active(&store, &first);
            store
                .contexts
                .learn_turn(
                    first.operation.as_ref().unwrap(),
                    "synthetic-original-token",
                )
                .unwrap();
            let second = prepare(&store, incoming.clone()).await.unwrap();
            let second_record = active(&store, &second);
            assert_ne!(first_record.branch, second_record.branch, "{model} {case}");
            assert_eq!(first_record.identity, second_record.identity);
            assert!(second.headers.get("x-codex-turn-state").is_none());
            assert!(
                store
                    .contexts
                    .turn_token(second.operation.as_ref().unwrap())
                    .is_none()
            );
            store
                .contexts
                .learn_turn(second.operation.as_ref().unwrap(), "synthetic-fork-token")
                .unwrap();
            assert_eq!(
                store
                    .contexts
                    .turn_token(first.operation.as_ref().unwrap())
                    .as_deref(),
                Some("synthetic-original-token")
            );
            let actual = second_record.history.as_ref().unwrap().values();
            let expected =
                crate::subscription_request::normalize_input(incoming.get("input")).unwrap();
            for (actual, expected) in actual.iter().zip(&expected) {
                assert_caller_item_with_generated_metadata(actual, expected);
            }
            assert_eq!(actual.len(), expected.len());
            assert!(!second_record.dependencies.awaiting_tools());
            let inner = store.contexts.inner.lock().unwrap();
            assert_eq!(inner.operations.len(), 2);
            assert!(inner.reservations.is_empty());
        }
    }
}

#[tokio::test]
async fn concurrent_anonymous_http_admission_gives_each_request_an_exclusive_lane() {
    let (_temp, store) = store();
    let (body, _) = tool_history(&store, "gpt-5.4").await;
    let requests = (0..8).map(|_| prepare(&store, body.clone()));
    let prepared: Vec<_> = futures_util::future::join_all(requests)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let inner = store.contexts.inner.lock().unwrap();
    let lanes: std::collections::HashSet<_> = inner.operations.values().map(|a| &a.lane).collect();
    assert_eq!(lanes.len(), 8);
    assert_eq!(inner.operations.len(), 8);
    assert!(inner.reservations.is_empty());
    drop(inner);
    drop(prepared);
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
}

#[tokio::test]
async fn completed_forks_archive_separately_and_resume_their_own_history() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        let (_temp, store) = store();
        let (body, _) = tool_history(&store, model).await;
        let first = prepare(&store, body.clone()).await.unwrap();
        let second = prepare(&store, body.clone()).await.unwrap();
        let branches = [
            active(&store, &first).branch,
            active(&store, &second).branch,
        ];
        let second = publish(
            &store,
            second,
            "resp_fork_second",
            text_output("synthetic second answer"),
        )
        .await;
        assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
        let first = publish(
            &store,
            first,
            "resp_fork_first",
            text_output("synthetic first answer"),
        )
        .await;
        for (response, branch) in [&first, &second].into_iter().zip(branches) {
            let mut next = body.clone();
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([response["output"][0].clone(), input("synthetic follow-up")]);
            let matched = prepare(&store, next).await.unwrap();
            assert_eq!(active(&store, &matched).branch, branch);
            drop(matched);
            let mut delta = request(json!([input("synthetic explicit follow-up")]));
            delta["model"] = model.into();
            delta["previous_response_id"] = response["id"].clone();
            let matched = prepare(&store, delta).await.unwrap();
            assert_eq!(active(&store, &matched).branch, branch);
        }
        assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
    }
}

#[tokio::test]
async fn dropping_one_fork_does_not_cancel_or_release_the_other() {
    let (_temp, store) = store();
    let (body, _) = tool_history(&store, "gpt-5.4").await;
    let first = prepare(&store, body.clone()).await.unwrap();
    let second = prepare(&store, body.clone()).await.unwrap();
    let second_id = second.operation.as_ref().unwrap().0.id.clone();
    drop(first);
    assert!(
        store
            .contexts
            .inner
            .lock()
            .unwrap()
            .operations
            .contains_key(&second_id)
    );
    let third = prepare(&store, body).await.unwrap();
    assert_ne!(
        active(&store, &second).branch,
        active(&store, &third).branch
    );
    drop(second);
    publish(
        &store,
        third,
        "resp_fork_survivor",
        text_output("synthetic done"),
    )
    .await;
    let inner = store.contexts.inner.lock().unwrap();
    assert!(inner.operations.is_empty());
    assert!(inner.reservations.is_empty());
}

#[tokio::test]
async fn admission_failure_keeps_the_active_branch_and_releases_fork_reservations() {
    let (_temp, store) = store();
    let (body, _) = tool_history(&store, "gpt-5.4").await;
    let first = prepare(&store, body.clone()).await.unwrap();
    let first_id = first.operation.as_ref().unwrap().0.id.clone();
    let path = store.state_path_for_test(NAMESPACE);
    let backup = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        prepare(&store, body).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(path, backup).unwrap();
    let inner = store.contexts.inner.lock().unwrap();
    assert_eq!(inner.operations.len(), 1);
    assert!(inner.operations.contains_key(&first_id));
    assert!(inner.reservations.is_empty());
}
