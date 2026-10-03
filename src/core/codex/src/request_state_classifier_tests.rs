use crate::fingerprint::FingerprintMode;
use crate::request_normalizer::{
    CodexStateContext, EmulationTransport, PreparedEmulatedRequest, StatefulPrepareError,
    prepare_identity_request,
};
use crate::request_profile::UpstreamProfile;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::{PersistedRequestState, WireIdOwner};
use bytes::Bytes;
use http::HeaderMap;
use serde_json::{Value, json};
use std::fs;

const ACCOUNT: &str = "acct_classifier_test";
const NAMESPACE: &str = "classifier-account";
const KEY: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn store() -> (tempfile::TempDir, RequestStateStore) {
    let temp = tempfile::tempdir().unwrap();
    let accounts = temp.path().join("accounts");
    fs::create_dir(&accounts).unwrap();
    let store = RequestStateStore::new(accounts);
    (temp, store)
}

async fn prepare(
    store: &RequestStateStore,
    transport: EmulationTransport,
    body: Value,
    key: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_identity_request(
        UpstreamProfile::CodexSubscription1592,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            binding: None,
            socket_id: None,
            account_ref: ACCOUNT,
            state_namespace: NAMESPACE,
            downstream_scope: key,
            fingerprint_mode: FingerprintMode::Device,
            store,
        },
        false,
    )
    .await
}

fn request(session: &str, thread: &str, turn: &str) -> Value {
    json!({"model":"gpt-5.5","input":"synthetic input",
        "client_metadata":{"session_id":session,"thread_id":thread,"turn_id":turn}})
}

fn child(thread: &str, turn: &str, parent: &str, parent_turn: Option<&str>, root: &str) -> Value {
    let mut body = request("session", thread, turn);
    body["client_metadata"]["parent_thread_id"] = parent.into();
    body["client_metadata"]["root_turn_id"] = root.into();
    if let Some(parent) = parent_turn {
        body["client_metadata"]["parent_turn_id"] = parent.into();
    }
    body
}

fn classifier(lease: &str, source: &str, parent: &str, root: &str) -> Value {
    let mut body = request("session", lease, "classification");
    body["model"] = "gpt-5.6-luna".into();
    body["prompt_cache_key"] = format!("guardian-v2:{source}").into();
    body["client_metadata"]["parent_turn_id"] = parent.into();
    body["client_metadata"]["x-openai-subagent"] = "guardian".into();
    body["client_metadata"]["x-codex-turn-metadata"] = json!({
        "session_id":"session","thread_id":lease,"turn_id":"classification",
        "parent_turn_id":parent,"root_turn_id":root,
        "guardian_classifier_source_thread_id":source,
        "thread_source":"guardian_classifier","turn_trigger":"guardian_classifier"
    })
    .to_string()
    .into();
    body
}

fn state(store: &RequestStateStore) -> PersistedRequestState {
    serde_json::from_slice(&fs::read(store.state_path_for_test(NAMESPACE)).unwrap()).unwrap()
}

#[tokio::test]
async fn classifier_retry_preserves_turn_canonical_owner_and_each_response_owner_after_reopen() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let (temp, store) = store();
        prepare(
            &store,
            transport,
            request("session", "session", "root"),
            KEY,
        )
        .await
        .unwrap();
        prepare(
            &store,
            transport,
            child("source", "source-turn", "session", Some("root"), "root"),
            KEY,
        )
        .await
        .unwrap();
        let first = prepare(
            &store,
            transport,
            classifier("lease-one", "source", "source-turn", "root"),
            KEY,
        )
        .await
        .unwrap()
        .resolved_identity
        .unwrap();
        let first_owner = WireIdOwner {
            session_id: first.session_id.clone(),
            thread_id: first.thread_id.clone(),
        };
        let saved_owner = first_owner.clone();
        let response = store
            .edit(NAMESPACE, ACCOUNT, KEY, move |editor| {
                editor.wire_from_upstream_response("resp_first_attempt", Some(&saved_owner))
            })
            .await
            .unwrap();
        let reopened = RequestStateStore::new(temp.path().join("accounts"));
        let next = prepare(
            &reopened,
            transport,
            classifier("lease-two", "source", "source-turn", "root"),
            KEY,
        )
        .await
        .unwrap()
        .resolved_identity
        .unwrap();
        assert_eq!(first.turn_id, next.turn_id);
        assert_ne!(first.thread_id, next.thread_id);
        assert_eq!(first.parent_thread_id, next.parent_thread_id);
        let next_owner = WireIdOwner {
            session_id: next.session_id.clone(),
            thread_id: next.thread_id.clone(),
        };
        let saved_next = next_owner.clone();
        let (old, new) = reopened
            .edit(NAMESPACE, ACCOUNT, KEY, move |editor| {
                let next =
                    editor.wire_from_upstream_response("resp_second_attempt", Some(&saved_next))?;
                Ok((
                    editor.required_response_owner_from_downstream(&response)?,
                    editor.required_response_owner_from_downstream(&next)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(old, first_owner);
        assert_eq!(new, next_owner);
        let state = state(&reopened);
        state.validate().unwrap();
        let scope = state.scopes.values().next().unwrap();
        let turn = scope
            .turns
            .values()
            .find(|turn| Some(&turn.id) == first.turn_id.as_ref())
            .unwrap();
        assert!(turn.classifier);
        assert_eq!(turn.thread_id, first.thread_id);
        assert!(
            scope
                .child_threads
                .values()
                .filter(|thread| [first.thread_id.as_str(), next.thread_id.as_str()]
                    .contains(&thread.id.as_str()))
                .all(|thread| thread.current_turn_id.is_none())
        );
    }
}

#[tokio::test]
async fn classifier_retry_rejects_changed_source_parent_root_and_ordinary_role_without_writes() {
    let (_temp, store) = store();
    let transport = EmulationTransport::Http;
    for body in [
        request("session", "session", "root"),
        request("session", "session", "other-root"),
        child("source", "source-turn", "session", Some("root"), "root"),
        child(
            "source",
            "next-source-turn",
            "session",
            Some("root"),
            "root",
        ),
        child("sibling", "sibling-turn", "session", Some("root"), "root"),
        classifier("lease-one", "source", "source-turn", "root"),
    ] {
        prepare(&store, transport, body, KEY).await.unwrap();
    }
    let before = fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    for body in [
        classifier("lease-two", "sibling", "sibling-turn", "root"),
        classifier("lease-two", "source", "next-source-turn", "root"),
        classifier("lease-two", "source", "source-turn", "other-root"),
        child(
            "lease-one",
            "classification",
            "source",
            Some("source-turn"),
            "root",
        ),
        child(
            "lease-two",
            "classification",
            "source",
            Some("source-turn"),
            "root",
        ),
    ] {
        assert!(matches!(
            prepare(&store, transport, body, KEY).await,
            Err(StatefulPrepareError::InvalidRequest)
        ));
        assert_eq!(
            fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn classifier_legacy_state_is_compatible_without_authorizing_cross_lease_adoption() {
    let (_temp, store) = store();
    let transport = EmulationTransport::Http;
    prepare(
        &store,
        transport,
        request("session", "session", "root"),
        KEY,
    )
    .await
    .unwrap();
    prepare(
        &store,
        transport,
        classifier("lease-one", "session", "root", "root"),
        KEY,
    )
    .await
    .unwrap();
    let mut legacy = serde_json::to_value(state(&store)).unwrap();
    for scope in legacy["scopes"].as_object_mut().unwrap().values_mut() {
        for turn in scope["turns"].as_object_mut().unwrap().values_mut() {
            turn.as_object_mut().unwrap().remove("classifier");
        }
    }
    let state: PersistedRequestState = serde_json::from_value(legacy.clone()).unwrap();
    state.validate().unwrap();
    assert!(
        state
            .scopes
            .values()
            .flat_map(|scope| scope.turns.values())
            .all(|turn| !turn.classifier)
    );
    fs::write(
        store.state_path_for_test(NAMESPACE),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let before = fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    assert!(matches!(
        prepare(
            &store,
            transport,
            classifier("lease-two", "session", "root", "root"),
            KEY
        )
        .await,
        Err(StatefulPrepareError::InvalidRequest)
    ));
    assert_eq!(
        fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
        before
    );
    // Same-owner replay can establish positive producer provenance without reparenting.
    prepare(
        &store,
        transport,
        classifier("lease-one", "session", "root", "root"),
        KEY,
    )
    .await
    .unwrap();
    prepare(
        &store,
        transport,
        classifier("lease-two", "session", "root", "root"),
        KEY,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn ordinary_turn_cannot_move_to_a_classifier_lease_without_same_owner_provenance() {
    let (_temp, store) = store();
    let transport = EmulationTransport::Http;
    let ordinary = child(
        "ordinary-owner",
        "classification",
        "session",
        Some("root"),
        "root",
    );
    for body in [request("session", "session", "root"), ordinary.clone()] {
        prepare(&store, transport, body, KEY).await.unwrap();
    }
    let before = fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    assert!(matches!(
        prepare(
            &store,
            transport,
            classifier("new-lease", "session", "root", "root"),
            KEY
        )
        .await,
        Err(StatefulPrepareError::InvalidRequest)
    ));
    assert_eq!(
        fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
        before
    );
    // Legacy records have no producer tag. A classified same-owner request may establish
    // provenance while preserving its exact parent/root; it grants no reparenting exception.
    prepare(
        &store,
        transport,
        classifier("ordinary-owner", "session", "root", "root"),
        KEY,
    )
    .await
    .unwrap();
    assert!(matches!(
        prepare(&store, transport, ordinary, KEY).await,
        Err(StatefulPrepareError::InvalidRequest)
    ));
}

#[tokio::test]
async fn child_root_and_guardian_omitted_root_use_the_known_ancestor_owner() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let (_temp, store) = store();
        let source = prepare(
            &store,
            transport,
            child("source", "source-root", "session", None, "source-root"),
            KEY,
        )
        .await
        .unwrap()
        .resolved_identity
        .unwrap();
        for reviewer in [false, true] {
            let mut body = if reviewer {
                child(
                    "reviewer",
                    "review-turn",
                    "source",
                    Some("source-root"),
                    "source-root",
                )
            } else {
                classifier("lease", "source", "source-root", "source-root")
            };
            body["client_metadata"]
                .as_object_mut()
                .unwrap()
                .remove("root_turn_id");
            let mut turn = if reviewer {
                json!({"thread_source":"guardian_review"})
            } else {
                serde_json::from_str(
                    body["client_metadata"]["x-codex-turn-metadata"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap()
            };
            turn.as_object_mut().unwrap().remove("root_turn_id");
            body["client_metadata"]["x-codex-turn-metadata"] = turn.to_string().into();
            let identity = prepare(&store, transport, body, KEY)
                .await
                .unwrap()
                .resolved_identity
                .unwrap();
            assert_eq!(identity.root_turn_id, source.turn_id);
            assert_eq!(identity.parent_turn_id, source.turn_id);
            assert_eq!(identity.parent_thread_id, Some(source.thread_id.clone()));
        }
    }
}

#[tokio::test]
async fn child_roots_reject_cross_session_sibling_nonroot_and_changed_ownership() {
    let (_temp, store) = store();
    let transport = EmulationTransport::Http;
    for body in [
        child("source", "source-root", "session", None, "source-root"),
        child("sibling", "sibling-root", "session", None, "sibling-root"),
        child(
            "source",
            "source-next",
            "session",
            Some("source-root"),
            "source-root",
        ),
        request("foreign", "foreign", "foreign-root"),
    ] {
        prepare(&store, transport, body, KEY).await.unwrap();
    }
    let before = fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    for body in [
        child(
            "descendant",
            "next",
            "source",
            Some("foreign-root"),
            "foreign-root",
        ),
        child(
            "descendant",
            "next",
            "source",
            Some("sibling-root"),
            "sibling-root",
        ),
        child(
            "descendant",
            "next",
            "source",
            Some("source-next"),
            "source-next",
        ),
        child("sibling", "source-root", "session", None, "source-root"),
        child(
            "source",
            "source-root",
            "session",
            Some("source-root"),
            "source-root",
        ),
    ] {
        assert!(matches!(
            prepare(&store, transport, body, KEY).await,
            Err(StatefulPrepareError::InvalidRequest)
        ));
        assert_eq!(
            fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            before
        );
    }
}
