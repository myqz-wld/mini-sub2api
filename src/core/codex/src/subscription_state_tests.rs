use crate::fingerprint::FingerprintMode;
use crate::request_normalizer::{
    CodexStateContext, EmulationTransport, PreparedEmulatedRequest, StatefulPrepareError,
    prepare_stateful_codex_request,
};
use crate::request_profile::UpstreamProfile;
use crate::request_state_store::RequestStateStore;
use crate::response_translation::ResponseStateContext;
use crate::subscription_context::ContextStore;
use crate::subscription_request::Evidence;
use bytes::Bytes;
use http::HeaderMap;
use mini_sub2api_protocol_v1::limits::InferenceLimits;
use serde_json::{Value, json};

#[path = "subscription_repair_tests.rs"]
mod repair_tests;

#[path = "subscription_history_tests.rs"]
mod history_tests;

#[path = "subscription_compaction_tests.rs"]
mod compaction_tests;

#[path = "subscription_reasoning_tests.rs"]
mod reasoning_tests;

const NAMESPACE: &str = "context-admission";
const OWNER: &str = "acct_context_tests";
const KEY: &str = "isolated-key";

fn store() -> (tempfile::TempDir, RequestStateStore) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = RequestStateStore::new(temp.path().to_path_buf());
    let mut limits = InferenceLimits::load().unwrap();
    limits.global_bytes = 1024 * 1024;
    limits.key_bytes = 512 * 1024;
    limits.session_bytes = 256 * 1024;
    limits.output_items = 4;
    limits.output_bytes = 1024;
    store.contexts = ContextStore::new(limits);
    (temp, store)
}

fn input(text: &str) -> Value {
    json!({"role":"user","content":text})
}
fn request(items: Value) -> Value {
    json!({"model":"gpt-5.4","instructions":"base","input":items})
}

async fn prepare(
    store: &RequestStateStore,
    body: Value,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1534,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
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
}

async fn publish(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
    id: &str,
    output: Value,
) -> Value {
    let state = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation);
    let created = state
        .translate_value(json!({"type":"response.created","response":{"id":id}}))
        .await
        .unwrap();
    let mut early = request(json!([]));
    early["previous_response_id"] = created["response"]["id"].clone();
    assert!(
        matches!(
            prepare(store, early).await,
            Err(StatefulPrepareError::StateUnavailable)
        ),
        "created is not a completed baseline"
    );
    let completed = state
        .translate_value(json!({"type":"response.completed","response":{"id":id,"output":output}}))
        .await
        .unwrap();
    completed["response"].clone()
}

#[tokio::test]
async fn active_turn_is_exclusive_and_abandoned_admission_releases_capacity() {
    let (_temp, store) = store();
    let mut body = request(json!([input("hello")]));
    body["client_metadata"] = json!({"session_id":"session","turn_id":"turn"});
    let first = prepare(&store, body.clone()).await.unwrap();
    let ledger = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    let mut conflicting = body.clone();
    conflicting["input"] = json!([input("different while busy")]);
    assert!(matches!(
        prepare(&store, conflicting).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    assert!(
        std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap() == ledger,
        "rejected admission committed speculative identity changes"
    );
    assert!(store.contexts.inner.lock().unwrap().reservations.is_empty());
    assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
    drop(first);
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
    assert!(prepare(&store, body).await.is_ok());
}

#[tokio::test]
async fn completed_body_overflow_preserves_delivery_but_never_publishes_partial_history() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let output = json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"x".repeat(2048)}]}]);
    let response = publish(&store, first, "resp_overflow", output.clone()).await;
    assert!(
        response["output"] == output,
        "valid response delivery changed"
    );
    let mut next = request(json!([input("next")]));
    next["previous_response_id"] = response["id"].clone();
    assert!(matches!(
        prepare(&store, next).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    assert!(
        prepare(&store, request(json!([input("full rebuild")])))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn completion_capacity_pressure_cannot_evict_its_own_session_publication() {
    let (_temp, mut store) = store();
    std::sync::Arc::make_mut(&mut store.contexts.limits).output_bytes = 128 * 1024;
    let mut body = request(json!([input("first")]));
    body["client_metadata"] = json!({"session_id":"pressure-owner"});
    let first = prepare(&store, body).await.unwrap();
    let identity = first.resolved_identity.as_ref().unwrap().clone();
    let output = json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"x".repeat(60 * 1024)}]}]);
    let response = publish(&store, first, "resp_pressure", output.clone()).await;
    assert!(
        response["output"] == output,
        "delivery changed under cache pressure"
    );
    let inner = store.contexts.inner.lock().unwrap();
    let scope = &inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)];
    assert!(
        scope.sessions.contains_key(&identity.session_id),
        "completion evicted its session"
    );
    assert_eq!(
        scope.aliases.get("pressure-owner"),
        Some(&identity.session_id)
    );
    let record = &scope.records[response["id"].as_str().unwrap()];
    assert!(record.completed && record.history.is_none());
    assert!(inner.operations.is_empty() && inner.reservations.is_empty());
}

#[tokio::test]
async fn admission_reclaims_idle_ws_comparison_without_closing_its_socket() {
    use crate::request_profile::CallerKind;
    use crate::responses_websocket_state::ResponsesWebSocketState;
    use std::sync::{Arc, Mutex};
    let (_temp, store) = store();
    let prepared = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let mut identity = prepared.resolved_identity.as_ref().unwrap().clone();
    publish(&store, prepared, "resp_baseline", json!([])).await;
    let socket = store.contexts.open_socket().unwrap();
    identity.connection_id = Some(socket.id.clone());
    let mut baseline =
        ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
    baseline.plan_public_create(&json!({"type":"response.create","model":"gpt-5.4",
        "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"x".repeat(40 * 1024)}]}]}));
    assert!(baseline.mark_public_create_attempted());
    baseline.observe_server_event(
        &json!({"type":"response.completed","response":{"id":"resp_baseline","output":[]}}),
    );
    assert!(baseline.retained_bytes() > 100 * 1024);
    let baseline = Arc::new(Mutex::new(baseline));
    let key = ContextStore::scope_key(NAMESPACE, KEY);
    store
        .contexts
        .track_baseline(key.clone(), &identity, &baseline);
    let mut inner = store.contexts.inner.lock().unwrap();
    assert!(inner.make_room(
        &store.contexts.limits,
        &key,
        &identity.session_id,
        200 * 1024
    ));
    assert_eq!(baseline.lock().unwrap().retained_bytes(), 0);
    assert!(inner.sockets.contains(&socket.id));
}

#[tokio::test]
async fn equal_anonymous_contexts_allow_omitted_message_ids_without_merging_active_owners() {
    let (_temp, store) = store();
    let output = |id: &str| json!([{"type":"message","id":id,"role":"assistant","content":[{"type":"output_text","text":"same"}]}]);
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let second = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    assert_ne!(
        first.resolved_identity.as_ref().unwrap().session_id,
        second.resolved_identity.as_ref().unwrap().session_id
    );
    let one = publish(&store, first, "resp_one", output("msg_one")).await;
    publish(&store, second, "resp_two", output("msg_two")).await;
    let mut assistant = one["output"][0].clone();
    assistant.as_object_mut().unwrap().remove("id");
    let body = request(json!([input("first"), assistant, input("next")]));
    let object = body.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap();
    let plan = store
        .contexts
        .plan(
            ContextStore::scope_key(NAMESPACE, KEY),
            object,
            evidence,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        plan.baseline
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
            .len,
        2
    );
    drop(plan);
    // Equivalent anonymous full histories share immutable content, with independent execution.
    let body = request(json!([input("first"), assistant]));
    let first = prepare(&store, body.clone()).await.unwrap();
    let second = prepare(&store, body).await.unwrap();
    let first_operation = first.operation.as_ref().unwrap();
    let second_operation = second.operation.as_ref().unwrap();
    store
        .contexts
        .learn_turn(first_operation, "private-first-branch-token")
        .unwrap();
    assert!(store.contexts.turn_token(second_operation).is_none());
    let inner = store.contexts.inner.lock().unwrap();
    assert_ne!(
        inner.operations[&first_operation.0.id].lane,
        inner.operations[&second_operation.0.id].lane
    );
}

#[tokio::test]
async fn ineligible_longer_terminal_does_not_veto_an_eligible_shorter_parent() {
    let (_temp, store) = store();
    let out = |id: &str| json!([{"type":"message","id":id,"role":"assistant","content":[{"type":"output_text","text":"answer"}]}]);
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let first = publish(&store, first, "resp_short", out("msg_short")).await;
    let history = json!([input("first"), first["output"][0], input("second")]);
    let second = prepare(&store, request(history.clone())).await.unwrap();
    let second = publish(&store, second, "resp_long", out("msg_long")).await;
    let mut items = history.as_array().unwrap().clone();
    let mut conflicting = second["output"][0].clone();
    conflicting["id"] = json!("explicit-other-id");
    items.push(conflicting);
    items.push(input("third"));
    let body = request(json!(items));
    let object = body.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap();
    let plan = store
        .contexts
        .plan(
            ContextStore::scope_key(NAMESPACE, KEY),
            object,
            evidence,
            None,
            None,
        )
        .unwrap();
    assert_eq!(plan.baseline.unwrap().history.unwrap().len, 2);
}

#[tokio::test]
async fn referenced_child_branch_and_window_survive_http_expansion() {
    let (_temp, store) = store();
    let mut body = request(json!([input("root")]));
    body["client_metadata"] = json!({"session_id":"root-session","turn_id":"root-turn"});
    let root = prepare(&store, body).await.unwrap();
    let root_id = root.resolved_identity.as_ref().unwrap().session_id.clone();
    publish(&store, root, "resp_root", json!([])).await;
    let mut child = request(json!([input("child")]));
    child["client_metadata"] = json!({"session_id":root_id,"thread_id":"child-thread","parent_thread_id":root_id,"turn_id":"child-turn", "x-codex-window-id":"child-thread:3"});
    let child = prepare(&store, child).await.unwrap();
    let identity = child.resolved_identity.as_ref().unwrap().clone();
    assert_ne!(identity.thread_id, identity.session_id);
    let response = publish(&store, child, "resp_child", json!([])).await;
    let mut delta = request(json!([input("continue child")]));
    delta["previous_response_id"] = response["id"].clone();
    let next = prepare(&store, delta).await.unwrap();
    let next = next.resolved_identity.unwrap();
    assert_eq!(next.session_id, identity.session_id);
    assert_eq!(next.thread_id, identity.thread_id);
    assert_eq!(next.parent_thread_id, identity.parent_thread_id);
    assert_eq!(next.window_number, 3);
}

#[tokio::test]
async fn an_explicit_claim_removes_the_session_from_the_anonymous_pool_at_admission() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let session = first.resolved_identity.as_ref().unwrap().session_id.clone();
    let response = publish(&store, first, "resp_claim", json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}])).await;
    let mut named = request(json!([
        input("first"),
        response["output"][0],
        input("named task")
    ]));
    named["client_metadata"] = json!({"session_id":session});
    let _active_named = prepare(&store, named).await.unwrap();
    let anonymous = prepare(
        &store,
        request(json!([
            input("first"),
            response["output"][0],
            input("anonymous task")
        ])),
    )
    .await
    .unwrap();
    assert_ne!(anonymous.resolved_identity.unwrap().session_id, session);
}

#[cfg(unix)]
#[tokio::test]
async fn ledger_write_failure_releases_admission_without_publishing_cache_aliases() {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(std::path::PathBuf, std::fs::Permissions);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, self.1.clone());
        }
    }
    let (temp, store) = store();
    let mut seed = request(json!([input("seed")]));
    seed["client_metadata"] = json!({"session_id":"prior-session"});
    let seed = prepare(&store, seed).await.unwrap();
    publish(&store, seed, "resp_seed", json!([])).await;
    let key = ContextStore::scope_key(NAMESPACE, KEY);
    let aliases = store.contexts.inner.lock().unwrap().scopes[&key]
        .aliases
        .clone();
    let restore = Restore(
        temp.path().to_path_buf(),
        std::fs::metadata(temp.path()).unwrap().permissions(),
    );
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let probe = temp.path().join("permission-probe");
    if std::fs::write(&probe, b"").is_ok() {
        drop(restore);
        std::fs::remove_file(probe).unwrap();
        eprintln!("permission-enforcement test skipped for a privileged runtime");
        return;
    }
    let mut body = request(json!([input("new request")]));
    body["client_metadata"] = json!({"session_id":"failed-session"});
    let result = prepare(&store, body.clone()).await;
    drop(restore);
    assert!(matches!(
        result,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    {
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.operations.is_empty() && inner.reservations.is_empty());
        assert!(
            inner.scopes[&key].aliases == aliases,
            "failed ledger transaction published aliases"
        );
    }
    assert!(prepare(&store, body).await.is_ok());
}
