use super::*;
use crate::request_state_types::WireIdDomain;
use serde_json::Value;

const HISTORY_SCOPE: &str = "psn_HHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHH";

#[tokio::test]
async fn lite_parallel_tool_calls_is_forced_off_for_both_profiles_and_transports() {
    for (profile_index, profile) in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ]
    .into_iter()
    .enumerate()
    {
        for (transport_index, transport) in
            [EmulationTransport::Http, EmulationTransport::WebSocket]
                .into_iter()
                .enumerate()
        {
            for (shape_index, already_shaped) in [false, true].into_iter().enumerate() {
                for (value_index, caller_value) in
                    [Value::Bool(true), Value::Null].into_iter().enumerate()
                {
                    let harness = CodexStateTestHarness::new();
                    let account_ref = format!(
                        "acct_lite_controls_{profile_index}_{transport_index}_{shape_index}_{value_index}"
                    );
                    let state_namespace = format!(
                        "namespace-lite-controls-{profile_index}-{transport_index}-{shape_index}-{value_index}"
                    );
                    let mut body = serde_json::json!({
                        "type":"response.create",
                        "model":if already_shaped { "gpt-5.4" } else { "gpt-5.6-sol" },
                        "input":[],
                        "parallel_tool_calls":caller_value
                    });
                    if already_shaped {
                        body["input"] = serde_json::json!([
                            {"type":"additional_tools","role":"developer","tools":[]}
                        ]);
                    }
                    let prepared = harness
                        .prepare(
                            profile,
                            transport,
                            &HeaderMap::new(),
                            Bytes::from(serde_json::to_vec(&body).expect("Lite request")),
                            1024 * 1024,
                            &account_ref,
                            &state_namespace,
                            HISTORY_SCOPE,
                        )
                        .await
                        .expect("stateful Lite request");
                    let value: Value =
                        serde_json::from_slice(&prepared.body).expect("prepared Lite request");
                    assert_eq!(value["parallel_tool_calls"], false);
                }
            }
        }
    }
}

#[tokio::test]
async fn store_false_history_strips_item_ids_but_keeps_calls_and_explicit_references() {
    for (profile_index, profile) in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ]
    .into_iter()
    .enumerate()
    {
        for (transport_index, transport) in
            [EmulationTransport::Http, EmulationTransport::WebSocket]
                .into_iter()
                .enumerate()
        {
            let harness = CodexStateTestHarness::new();
            let account_ref = format!("acct_history_{profile_index}_{transport_index}");
            let state_namespace = format!("namespace-history-{profile_index}-{transport_index}");
            let downstream_reference = harness
                .store
                .edit(&state_namespace, &account_ref, HISTORY_SCOPE, |editor| {
                    editor.wire_from_upstream(WireIdDomain::Item, "msg_provider_reference")
                })
                .await
                .expect("seed provider item reference");
            let body = serde_json::json!({
                "type":"response.create",
                "model":"gpt-5.4",
                "input":[
                    {"type":"reasoning","id":"rs_old","summary":[]},
                    {"type":"message","id":"msg_old","role":"assistant","content":[
                        {"type":"output_text","text":"old answer"}
                    ]},
                    {"type":"function_call","id":"fc_old","call_id":"call_old","name":"lookup","arguments":"{}"},
                    {"type":"function_call_output","id":"fco_old","call_id":"call_old","output":{
                        "id":"opaque_output_id","value":"done"
                    }},
                    {"type":"message","role":"user","content":[
                        {"type":"input_text","text":"continue"}
                    ]},
                    {"type":"item_reference","id":downstream_reference}
                ]
            });
            let prepared = harness
                .prepare(
                    profile,
                    transport,
                    &HeaderMap::new(),
                    Bytes::from(serde_json::to_vec(&body).expect("history request")),
                    1024 * 1024,
                    &account_ref,
                    &state_namespace,
                    HISTORY_SCOPE,
                )
                .await
                .expect("stateful history request");
            let value: Value =
                serde_json::from_slice(&prepared.body).expect("prepared history request");
            assert!(prepared.synthesized_item_ids.is_empty());
            let items = value["input"].as_array().expect("history input");

            for item in &items[..5] {
                assert!(item.get("id").is_none(), "inline history ID crossed");
            }
            assert_eq!(items[5]["id"], "msg_provider_reference");
            assert_ne!(items[2]["call_id"], "call_old");
            assert_eq!(items[2]["call_id"], items[3]["call_id"]);
            assert_eq!(items[3]["output"]["id"], "opaque_output_id");
        }
    }
}
