use super::*;
use crate::request_state_types::WireIdDomain;
use serde_json::json;

const ACCOUNT: &str = "acct_release1560";
const NAMESPACE: &str = "release1560";
const SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[tokio::test]
async fn codex1560_generated_lite_setup_has_no_invented_turn_attribution() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let harness = CodexStateTestHarness::new();
        let body = json!({"model":"gpt-6-astra","instructions":"synthetic base",
            "input":"synthetic user", "tools":[]});
        let prepared = prepare(&harness, transport, &HeaderMap::new(), body, SCOPE)
            .await
            .unwrap();
        let body = value(&prepared);
        assert!(
            body["input"][0]
                .get("internal_chat_message_metadata_passthrough")
                .is_none()
        );
        assert!(
            body["input"][1]
                .get("internal_chat_message_metadata_passthrough")
                .is_none()
        );
        assert!(
            body["input"][2]["internal_chat_message_metadata_passthrough"]["create_time"]
                .is_number()
        );
        assert_eq!(
            body["client_metadata"]["guardian_credits_requested"],
            "true"
        );
    }
}

async fn prepare(
    harness: &CodexStateTestHarness,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Value,
    scope: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    harness
        .prepare(
            UpstreamProfile::CodexSubscription1560,
            transport,
            headers,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
            1024 * 1024,
            ACCOUNT,
            NAMESPACE,
            scope,
        )
        .await
}

fn value(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).unwrap()
}

#[tokio::test]
async fn codex1560_configuration_updates_and_tool_evidence_survive_both_transports() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-5.4", "gpt-6-astra"] {
            let harness = CodexStateTestHarness::new();
            let update = json!({"type":"configuration_update", "reasoning":{"effort":"high"}});
            let evidence = json!({"name":"functions.lookup","arguments":{"id":"opaque"},
                "tool_result_sources":[{"type":"url","url":"https://example.test"}],
                "tool_result_metadata":{"response_id":"opaque","nested":{"id":"opaque"}}});
            let body = json!({"model":model,"input":[update.clone(),
                {"type":"message","role":"user","content":"synthetic task",
                 "internal_chat_message_metadata_passthrough":{"executed_tool_calls":[evidence.clone()]}}],
                "client_metadata":{"x-codex-turn-metadata":json!({"analytics_enabled":false}).to_string()}});
            let prepared = prepare(&harness, transport, &HeaderMap::new(), body, SCOPE)
                .await
                .unwrap();
            let body = value(&prepared);
            let input = body["input"].as_array().unwrap();
            assert_eq!(
                input
                    .iter()
                    .find(|item| item["type"] == "configuration_update"),
                None
            );
            let message = input.iter().find(|item| item["role"] == "user").unwrap();
            assert_eq!(
                message["internal_chat_message_metadata_passthrough"]["executed_tool_calls"],
                json!([evidence])
            );
            let turn: Value = serde_json::from_str(
                body["client_metadata"]["x-codex-turn-metadata"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            let header: Value =
                serde_json::from_str(prepared.headers["x-codex-turn-metadata"].to_str().unwrap())
                    .unwrap();
            assert_eq!(turn["analytics_enabled"], false);
            assert_eq!(header["analytics_enabled"], false);
        }
    }
}

#[tokio::test]
async fn codex1560_cache_affinity_is_shared_without_merging_session_ownership() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let harness = CodexStateTestHarness::new();
        let mut headers = HeaderMap::new();
        headers.insert("session-id", "source".parse().unwrap());
        let request = |session: &str| {
            json!({"model":"gpt-5.4","input":"synthetic task",
            "prompt_cache_key":"source","client_metadata":{"session_id":session,"thread_id":session}})
        };
        // A fork can arrive before its cache source without claiming the source's session.
        let fork = prepare(&harness, transport, &headers, request("fork"), SCOPE)
            .await
            .unwrap();
        let root = prepare(&harness, transport, &headers, request("source"), SCOPE)
            .await
            .unwrap();
        let again = prepare(&harness, transport, &headers, request("fork"), SCOPE)
            .await
            .unwrap();
        assert_ne!(
            fork.resolved_identity.as_ref().unwrap().session_id,
            root.resolved_identity.as_ref().unwrap().session_id
        );
        assert_eq!(
            value(&fork)["prompt_cache_key"],
            value(&root)["prompt_cache_key"]
        );
        assert_eq!(
            value(&fork)["prompt_cache_key"],
            value(&again)["prompt_cache_key"]
        );
        assert_eq!(fork.headers["session-id"], root.headers["session-id"]);
        assert_ne!(
            value(&fork)["client_metadata"]["session_id"],
            value(&fork)["prompt_cache_key"]
        );
        let other = prepare(
            &harness,
            transport,
            &headers,
            request("fork"),
            "psn_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        )
        .await
        .unwrap();
        assert_ne!(
            value(&fork)["prompt_cache_key"],
            value(&other)["prompt_cache_key"]
        );
    }
}

#[tokio::test]
async fn codex1560_guardian_parent_is_a_required_scoped_provider_reference() {
    let harness = CodexStateTestHarness::new();
    let parent = harness
        .store
        .edit(NAMESPACE, ACCOUNT, SCOPE, |editor| {
            editor.wire_from_upstream(WireIdDomain::Response, "resp_provider_parent")
        })
        .await
        .unwrap();
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-guardian", "reviewer".parse().unwrap());
        let request = json!({"model":"codex-auto-review","input":"synthetic review",
            "client_metadata":{"parent_response_id":parent}});
        let prepared = prepare(&harness, transport, &headers, request.clone(), SCOPE)
            .await
            .unwrap();
        assert_eq!(
            value(&prepared)["client_metadata"]["parent_response_id"],
            "resp_provider_parent"
        );
        let error = prepare(
            &harness,
            transport,
            &headers,
            request,
            "psn_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        )
        .await
        .unwrap_err();
        assert_eq!(error, StatefulPrepareError::StateUnavailable);
    }
}

#[tokio::test]
async fn codex1560_memory_consolidation_preserves_its_turn_without_copying_raw_ids() {
    let harness = CodexStateTestHarness::new();
    let request = json!({"model":"gpt-5.4","input":"synthetic memory",
        "client_metadata":{"x-codex-turn-metadata":json!({"request_kind":"memory",
            "turn_id":"memory-turn","root_turn_id":"memory-turn",
            "turn_trigger":"memory_consolidation","analytics_enabled":false}).to_string()}});
    let prepared = prepare(
        &harness,
        EmulationTransport::Http,
        &HeaderMap::new(),
        request,
        SCOPE,
    )
    .await
    .unwrap();
    let body = value(&prepared);
    let turn: Value = serde_json::from_str(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(turn["turn_id"].is_string());
    assert_ne!(turn["turn_id"], "memory-turn");
    assert_eq!(turn["turn_id"], turn["root_turn_id"]);
    assert_eq!(turn["turn_trigger"], "memory_consolidation");
}
