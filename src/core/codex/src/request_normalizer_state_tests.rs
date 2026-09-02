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
        UpstreamProfile::CodexSubscription149,
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
    prepare_stateful_codex_request(
        profile,
        EmulationTransport::Http,
        headers,
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
        1024 * 1024,
        CodexStateContext {
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
    assert_ne!(first_value["input"][0]["id"], "msg_downstream_real");

    let state = fs::read_to_string(store.state_path_for_test(NAMESPACE)).expect("state file");
    for raw in [
        "body-session-canonical",
        "body-thread-conflict",
        "turn-real",
        "downstream-installation",
    ] {
        assert!(
            state.contains(raw),
            "reversible identity pair missing: {raw}"
        );
    }
    for discarded_conflict in ["cache-conflict", "root-turn-conflict"] {
        assert!(!state.contains(discarded_conflict));
    }
    assert!(state.contains("msg_downstream_real"));
    assert!(
        !state.contains("hello"),
        "request content leaked into state"
    );
}

#[tokio::test]
async fn both_codex_profiles_reuse_the_same_identity_contract_after_reopen() {
    for (profile, account_ref, state_namespace) in [
        (
            UpstreamProfile::CodexOpenAi149,
            "acct_openai_stateful",
            "acct_openai_stateful",
        ),
        (
            UpstreamProfile::CodexSubscription149,
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
        assert_ne!(first["input"][0]["id"], "msg_downstream");
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

#[tokio::test]
async fn missing_turn_reuses_for_tool_roundtrip_and_changes_for_new_user() {
    let (_temp, store) = store();
    let headers = HeaderMap::new();
    let request = |input: Value| {
        serde_json::json!({
            "model":"gpt-5.4",
            "input":input,
            "client_metadata":{"session_id":"conversation-stable"}
        })
    };
    let first_input = serde_json::json!([{
        "type":"message","role":"user","content":[{"type":"input_text","text":"first"}]
    }]);
    let first = value(&prepare(&store, &headers, request(first_input.clone())).await);
    let first_turn = first["client_metadata"]["turn_id"]
        .as_str()
        .expect("first turn")
        .to_string();
    let first_user = first["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    let first_item_id = first_user["id"].as_str().expect("item id").to_string();
    let first_create_time =
        first_user["internal_chat_message_metadata_passthrough"]["create_time"].clone();

    let call = seed_upstream_wire(&store, WireIdDomain::Call, "call_provider").await;

    let tool_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":call,"output":"done"}
    ]);
    let tool = value(&prepare(&store, &headers, request(tool_input)).await);
    assert_eq!(tool["client_metadata"]["turn_id"], first_turn);
    let repeated_user = tool["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    assert_eq!(repeated_user["id"], first_item_id);
    assert_eq!(
        repeated_user["internal_chat_message_metadata_passthrough"]["create_time"],
        first_create_time
    );

    let minimal_call =
        seed_upstream_wire(&store, WireIdDomain::Call, "call_provider_minimal").await;
    let minimal_tool = value(
        &prepare(
            &store,
            &headers,
            request(serde_json::json!([{
                "type":"function_call_output",
                "call_id":minimal_call,
                "output":"done"
            }])),
        )
        .await,
    );
    assert_eq!(minimal_tool["client_metadata"]["turn_id"], first_turn);

    let next_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":call,"output":"done"},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}
    ]);
    let next = value(&prepare(&store, &headers, request(next_input)).await);
    assert_ne!(next["client_metadata"]["turn_id"], first_turn);
}

#[tokio::test]
async fn explicit_parent_lineage_keeps_root_session_and_distinct_child_thread() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":[{"type":"message","role":"user","content":"child work"}],
        "client_metadata":{
            "session_id":"root-session",
            "thread_id":"child-thread",
            "x-codex-turn-metadata":"{\"parent_thread_id\":\"root-session\",\"turn_id\":\"child-turn\",\"root_turn_id\":\"root-turn\"}"
        }
    });
    let prepared = prepare(&store, &HeaderMap::new(), body).await;
    let value = value(&prepared);
    let metadata = &value["client_metadata"];
    assert_ne!(metadata["session_id"], metadata["thread_id"]);
    assert_eq!(metadata["parent_thread_id"], metadata["session_id"],);
    assert_ne!(metadata["turn_id"], metadata["root_turn_id"]);
    assert_eq!(
        prepared.headers["x-codex-parent-thread-id"],
        metadata["session_id"].as_str().expect("session")
    );
}

#[tokio::test]
async fn oversized_schema_id_is_an_invalid_request_not_a_state_outage() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":"hello",
        "client_metadata":{"session_id":"x".repeat(513)}
    });
    let error = prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
        1024 * 1024,
        CodexStateContext {
            account_ref: ACCOUNT_REF,
            state_namespace: NAMESPACE,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store: &store,
        },
        false,
    )
    .await
    .expect_err("oversized ID must fail");
    assert_eq!(error, StatefulPrepareError::InvalidRequest);
    assert!(!store.state_path_for_test(NAMESPACE).exists());
}

#[tokio::test]
async fn responses_conversation_anchors_fallback_but_carrier_free_calls_stay_distinct() {
    let (_temp, store) = store();
    let conversation_a =
        seed_upstream_wire(&store, WireIdDomain::Conversation, "conv-provider-a").await;
    let conversation_b =
        seed_upstream_wire(&store, WireIdDomain::Conversation, "conv-provider-b").await;
    let request = |conversation: Option<&str>, text: &str| {
        let mut value = serde_json::json!({
            "model":"gpt-5.4",
            "input":[{"type":"message","role":"user","content":text}]
        });
        if let Some(conversation) = conversation {
            value
                .as_object_mut()
                .expect("request")
                .insert("conversation".to_string(), conversation.into());
        }
        value
    };
    let a = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_a), "same"),
        )
        .await,
    );
    let b = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_b), "same"),
        )
        .await,
    );
    let a_next = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_a), "different"),
        )
        .await,
    );
    assert_ne!(
        a["client_metadata"]["session_id"],
        b["client_metadata"]["session_id"]
    );
    assert_eq!(
        a["client_metadata"]["session_id"],
        a_next["client_metadata"]["session_id"]
    );

    let first_free = value(&prepare(&store, &HeaderMap::new(), request(None, "identical")).await);
    let second_free = value(&prepare(&store, &HeaderMap::new(), request(None, "identical")).await);
    assert_ne!(
        first_free["client_metadata"]["session_id"],
        second_free["client_metadata"]["session_id"]
    );
}

#[tokio::test]
async fn previous_response_alias_restores_its_conversation_and_thread_owner() {
    let (temp, store) = store();
    let first = prepare(
        &store,
        &HeaderMap::new(),
        serde_json::json!({
            "model":"gpt-5.4",
            "input":[{"type":"message","id":"msg_first","role":"user","content":"first"}]
        }),
    )
    .await;
    let first_value = value(&first);
    let first_identity = first.resolved_identity.as_ref().expect("identity");
    let response_state = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        Some(first_identity),
        None,
    );
    let translated = response_state
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_provider_first","output":[]}
        }))
        .await
        .expect("translate response");
    let alias = translated["response"]["id"]
        .as_str()
        .expect("response alias");
    assert_ne!(alias, "resp_provider_first");

    let reopened = RequestStateStore::new(temp.path().join("accounts"));
    let next = value(
        &prepare(
            &reopened,
            &HeaderMap::new(),
            serde_json::json!({
                "model":"gpt-5.4",
                "previous_response_id":alias,
                "input":[{"type":"message","id":"msg_next","role":"user","content":"next"}]
            }),
        )
        .await,
    );
    assert_eq!(
        next["client_metadata"]["session_id"],
        first_value["client_metadata"]["session_id"]
    );
    assert_eq!(
        next["client_metadata"]["thread_id"],
        first_value["client_metadata"]["thread_id"]
    );
    assert_ne!(
        next["client_metadata"]["turn_id"],
        first_value["client_metadata"]["turn_id"]
    );
}

#[tokio::test]
async fn equal_user_content_uses_item_identity_and_tool_history_reuses_active_turn() {
    let (_temp, store) = store();
    let request = |input: Value| {
        serde_json::json!({
            "model":"gpt-5.4",
            "input":input,
            "client_metadata":{"session_id":"equal-content-session"}
        })
    };
    let first = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(serde_json::json!([{
                "type":"message","id":"msg_equal_1","role":"user","content":"repeat"
            }])),
        )
        .await,
    );
    let second_body = request(serde_json::json!([{
        "type":"message","id":"msg_equal_2","role":"user","content":"repeat"
    }]));
    let second = value(&prepare(&store, &HeaderMap::new(), second_body.clone()).await);
    let retry = value(&prepare(&store, &HeaderMap::new(), second_body).await);
    assert_ne!(
        first["client_metadata"]["turn_id"],
        second["client_metadata"]["turn_id"]
    );
    assert_eq!(
        second["client_metadata"]["turn_id"],
        retry["client_metadata"]["turn_id"]
    );

    let call = seed_upstream_wire(&store, WireIdDomain::Call, "call_provider_equal").await;
    let tool = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(serde_json::json!([
                {"type":"message","id":"msg_equal_2","role":"user","content":"repeat"},
                {"type":"function_call_output","call_id":call,"output":"done"}
            ])),
        )
        .await,
    );
    assert_eq!(
        tool["client_metadata"]["turn_id"],
        second["client_metadata"]["turn_id"]
    );
}

#[tokio::test]
async fn compaction_commits_only_on_completed_and_same_base_operations_converge() {
    let (_temp, store) = store();
    let request = |session: &str, turn: &str| {
        let metadata = serde_json::json!({
            "session_id":session,
            "thread_id":session,
            "turn_id":turn,
            "request_kind":"compaction",
            "compaction":{
                "trigger":"manual",
                "reason":"user_requested",
                "implementation":"responses_compaction_v2",
                "phase":"standalone_turn",
                "strategy":"memento"
            }
        });
        serde_json::json!({
            "model":"gpt-5.4",
            "input":[
                {"type":"message","role":"user","content":"history"},
                {"type":"compaction_trigger"}
            ],
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}
        })
    };
    let first = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    let first_value = value(&first);
    let first_pending = first
        .pending_compaction
        .as_ref()
        .expect("first pending compaction")
        .clone();
    let retry = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    let retry_value = value(&retry);
    assert_eq!(retry.pending_compaction.as_ref(), Some(&first_pending));
    let overlapping = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a2"),
    )
    .await;
    let overlapping_value = value(&overlapping);
    assert_eq!(
        overlapping
            .pending_compaction
            .as_ref()
            .expect("overlapping pending")
            .target_window,
        first_pending.target_window
    );
    let other = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-b", "compact-turn-b1"),
        )
        .await,
    );
    assert_eq!(
        first_value["client_metadata"]["x-codex-window-id"],
        retry_value["client_metadata"]["x-codex-window-id"]
    );
    assert!(
        first_value["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert!(
        overlapping_value["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert!(
        other["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );
    assert_ne!(
        first_value["client_metadata"]["session_id"],
        other["client_metadata"]["session_id"]
    );

    let failed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        first.resolved_identity.as_ref(),
        first.pending_compaction.as_ref(),
    );
    failed
        .translate_value(serde_json::json!({
            "type":"response.failed",
            "response":{"id":"resp_failed"}
        }))
        .await
        .expect("translate failed terminal");
    let after_failure = prepare(
        &store,
        &HeaderMap::new(),
        request("compact-session-a", "compact-turn-a1"),
    )
    .await;
    assert!(
        value(&after_failure)["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0"))
    );

    let completed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        after_failure.resolved_identity.as_ref(),
        after_failure.pending_compaction.as_ref(),
    );
    completed
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_completed"}
        }))
        .await
        .expect("commit completed terminal");
    let overlapping_completed = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        overlapping.resolved_identity.as_ref(),
        overlapping.pending_compaction.as_ref(),
    );
    overlapping_completed
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_overlapping"}
        }))
        .await
        .expect("converge overlapping completion");

    let committed_retry = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-a", "compact-turn-a1"),
        )
        .await,
    );
    assert!(
        committed_retry["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":0")),
        "a completed marker retry must reuse its original committed base"
    );

    let later = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request("compact-session-a", "compact-turn-a3"),
        )
        .await,
    );
    assert!(
        later["client_metadata"]["x-codex-window-id"]
            .as_str()
            .is_some_and(|window| window.ends_with(":1"))
    );
}

#[path = "request_normalizer_reference_tests.rs"]
mod reference_tests;
