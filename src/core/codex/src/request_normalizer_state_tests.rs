use super::*;
use crate::fingerprint::FingerprintMode;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::WireIdDomain;
use crate::response_translation::ResponseStateContext;
use http::HeaderValue;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;
use uuid::Uuid;

const ACCOUNT_REF: &str = "acct_stateful_normalizer";
const NAMESPACE: &str = "chatgpt-stateful-normalizer";
const SCOPE: &str = "psn_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";

fn store() -> (TempDir, RequestStateStore) {
    let temp = TempDir::new().expect("temp dir");
    let accounts = temp.path().join("accounts");
    fs::create_dir(&accounts).expect("accounts dir");
    (temp, RequestStateStore::new(accounts))
}

async fn prepare(
    store: &RequestStateStore,
    headers: &HeaderMap,
    body: Value,
) -> PreparedEmulatedRequest {
    prepare_profile(
        store,
        UpstreamProfile::CodexSubscription1534,
        ACCOUNT_REF,
        NAMESPACE,
        headers,
        body,
    )
    .await
}

async fn prepare_profile(
    store: &RequestStateStore,
    profile: UpstreamProfile,
    account_ref: &str,
    state_namespace: &str,
    headers: &HeaderMap,
    body: Value,
) -> PreparedEmulatedRequest {
    prepare_identity_request(
        profile,
        EmulationTransport::Http,
        headers,
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
        1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            binding: None,
            socket_id: None,
            account_ref,
            state_namespace,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store,
        },
        false,
    )
    .await
    .expect("stateful preparation")
}

fn value(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).expect("prepared JSON")
}

async fn seed_upstream_wire(
    store: &RequestStateStore,
    domain: WireIdDomain,
    upstream_id: &str,
) -> String {
    let upstream_id = upstream_id.to_string();
    store
        .edit(NAMESPACE, ACCOUNT_REF, SCOPE, move |editor| {
            editor.wire_from_upstream(domain, &upstream_id)
        })
        .await
        .expect("seed upstream wire mapping")
}

fn turn_metadata(value: &Value) -> Value {
    serde_json::from_str(
        value["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON")
}

fn assert_uuid_version(value: &str, version: usize) {
    assert_eq!(
        Uuid::parse_str(value).expect("UUID").get_version_num(),
        version
    );
}

#[tokio::test]
async fn conflicting_root_carriers_converge_and_persist_true_uuid_versions() {
    let (_temp, store) = store();
    let mut headers = HeaderMap::new();
    headers.insert(
        "session-id",
        HeaderValue::from_static("header-session-conflict"),
    );
    headers.insert(
        "thread-id",
        HeaderValue::from_static("header-thread-conflict"),
    );
    headers.insert(
        "x-client-request-id",
        HeaderValue::from_static("header-client-conflict"),
    );
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "prompt_cache_key":"cache-conflict",
        "input":[{
            "type":"message",
            "id":"msg_downstream_real",
            "role":"user",
            "content":[{"type":"input_text","text":"hello"}],
            "internal_chat_message_metadata_passthrough":{"turn_id":"turn-real"}
        }],
        "client_metadata":{
            "session_id":"body-session-canonical",
            "thread_id":"body-thread-conflict",
            "turn_id":"turn-real",
            "root_turn_id":"root-turn-conflict",
            "x-codex-installation-id":"downstream-installation",
            "x-codex-turn-metadata":"{\"session_id\":\"nested-session-conflict\",\"thread_id\":\"nested-thread-conflict\",\"turn_id\":\"turn-real\",\"root_turn_id\":\"root-turn-conflict\"}"
        }
    });

    let first = prepare(&store, &headers, body.clone()).await;
    let second = prepare(&store, &headers, body).await;
    let first_value = value(&first);
    let second_value = value(&second);
    let metadata = &first_value["client_metadata"];
    let session = metadata["session_id"].as_str().expect("session");
    let thread = metadata["thread_id"].as_str().expect("thread");
    let turn = metadata["turn_id"].as_str().expect("turn");
    let installation = metadata["x-codex-installation-id"]
        .as_str()
        .expect("installation");

    assert_eq!(session, thread);
    assert_eq!(first_value["prompt_cache_key"], session);
    assert_eq!(first.headers["session-id"], session);
    assert_eq!(first.headers["thread-id"], thread);
    assert_eq!(first.headers["x-client-request-id"], thread);
    assert_eq!(metadata["root_turn_id"], turn);
    assert_uuid_version(installation, 4);
    assert_uuid_version(session, 7);
    assert_uuid_version(turn, 7);
    assert_eq!(second_value["client_metadata"]["session_id"], session);
    assert_eq!(second_value["client_metadata"]["turn_id"], turn);
    assert_eq!(turn_metadata(&first_value)["session_id"], session);
    assert_eq!(turn_metadata(&first_value)["thread_id"], thread);
    assert_eq!(turn_metadata(&first_value)["turn_id"], turn);
    assert!(first_value["input"][0].get("id").is_some());

    let state = fs::read_to_string(store.state_path_for_test(NAMESPACE)).expect("state file");
    for raw in [
        "header-session-conflict",
        "body-thread-conflict",
        "turn-real",
        "downstream-installation",
    ] {
        assert!(
            state.contains(raw),
            "reversible identity pair missing: {raw}"
        );
    }
    for discarded_conflict in [
        "body-session-canonical",
        "cache-conflict",
        "root-turn-conflict",
    ] {
        assert!(!state.contains(discarded_conflict));
    }
    assert!(
        state.contains("msg_downstream_real"),
        "validated item alias must survive continuation"
    );
    assert!(
        !state.contains("hello"),
        "request content leaked into state"
    );
}

#[tokio::test]
async fn both_codex_profiles_reuse_the_same_identity_contract_after_reopen() {
    for (profile, account_ref, state_namespace) in [
        (
            UpstreamProfile::CodexSubscription1534,
            "acct_openai_stateful",
            "acct_openai_stateful",
        ),
        (
            UpstreamProfile::CodexSubscription1534,
            "acct_subscription_stateful",
            "chatgpt-subscription-stateful",
        ),
    ] {
        let (temp, store) = store();
        let body = serde_json::json!({
            "model":"gpt-5.4",
            "response_id":"resp_downstream",
            "input":[{
                "type":"message",
                "id":"msg_downstream",
                "role":"user",
                "content":[{"type":"input_text","text":"hello"}]
            }],
            "client_metadata":{
                "session_id":"session_downstream",
                "thread_id":"thread_downstream",
                "turn_id":"turn_downstream"
            }
        });
        let first = value(
            &prepare_profile(
                &store,
                profile,
                account_ref,
                state_namespace,
                &HeaderMap::new(),
                body.clone(),
            )
            .await,
        );
        let reopened = RequestStateStore::new(temp.path().join("accounts"));
        let second = value(
            &prepare_profile(
                &reopened,
                profile,
                account_ref,
                state_namespace,
                &HeaderMap::new(),
                body,
            )
            .await,
        );

        assert_eq!(first, second, "profile {profile:?} changed after reopen");
        assert_uuid_version(
            first["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .expect("installation"),
            4,
        );
        for field in ["session_id", "thread_id", "turn_id"] {
            assert_uuid_version(
                first["client_metadata"][field].as_str().expect("identity"),
                7,
            );
        }
        assert_ne!(first["response_id"], "resp_downstream");
        assert!(first["input"][0].get("id").is_some());
    }
}

#[tokio::test]
async fn sandbox_is_derived_from_sidecar_platform_and_header_body_stay_in_sync() {
    let (_temp, store) = store();
    let expected_sandbox = match std::env::consts::OS {
        "macos" => "seatbelt",
        "linux" | "android" => "seccomp",
        "windows" => "windows_sandbox",
        other => panic!("unsupported test platform {other}"),
    };
    let mismatched = if expected_sandbox == "seatbelt" {
        "seccomp"
    } else {
        "seatbelt"
    };
    let workspaces = serde_json::json!({
        "/downstream/workspace": {"writable_roots":["/downstream/workspace"]}
    });
    let nested = serde_json::json!({
        "session_id":"sandbox-session",
        "thread_id":"sandbox-session",
        "turn_id":"sandbox-turn",
        "request_kind":"turn",
        "sandbox_mode":"workspace-write",
        "sandbox":mismatched,
        "workspaces":workspaces.clone()
    });
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":"hello",
        "client_metadata":{"x-codex-turn-metadata":nested.to_string()}
    });
    let prepared = prepare(&store, &HeaderMap::new(), body).await;
    let value = value(&prepared);
    let body_turn = turn_metadata(&value);
    let header_turn: Value = serde_json::from_str(
        prepared.headers["x-codex-turn-metadata"]
            .to_str()
            .expect("header metadata"),
    )
    .expect("header metadata JSON");
    for metadata in [&body_turn, &header_turn] {
        assert_eq!(metadata["sandbox_mode"], "workspace-write");
        assert_eq!(metadata["sandbox"], expected_sandbox);
        assert_eq!(metadata["workspaces"], workspaces);
    }
}

#[path = "request_normalizer_continuation_state_tests.rs"]
mod continuation_tests;

#[path = "request_normalizer_compaction_state_tests.rs"]
mod compaction_tests;

#[path = "request_normalizer_reference_tests.rs"]
mod reference_tests;
