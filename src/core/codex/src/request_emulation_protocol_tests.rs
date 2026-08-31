use super::*;
use crate::request_profile::UpstreamProfile;
use serde_json::Value;
use serde_json::json;

const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn transport_allowlists_and_routing_hint_are_profile_specific() {
    let caller = json!({
        "type":"response.create",
        "generate":false,
        "stream_id":"stream-caller",
        "background":true,
        "stream":false,
        "state":{"unsupported":true},
        "model":"gpt-5.4",
        "input":[]
    });
    let http = prepare_value(
        UpstreamProfile::CodexOpenAi149,
        EmulationTransport::Http,
        caller.clone(),
    );
    assert!(http.value.get("type").is_none());
    assert!(http.value.get("generate").is_none());
    assert!(http.value.get("stream_id").is_none());
    assert!(http.value.get("state").is_none());
    assert_eq!(http.value["background"], true);
    assert_eq!(http.value["stream"], true);
    assert!(http.headers.get("x-codex-routing-hint").is_none());

    let websocket = prepare_value(
        UpstreamProfile::CodexOpenAi149,
        EmulationTransport::WebSocket,
        caller.clone(),
    );
    assert_eq!(websocket.value["type"], "response.create");
    assert_eq!(websocket.value["generate"], false);
    assert_eq!(websocket.value["stream_id"], "stream-caller");
    assert!(websocket.value.get("state").is_none());
    assert!(websocket.value.get("background").is_none());
    assert!(websocket.value.get("stream").is_none());
    assert!(websocket.headers.get("x-codex-routing-hint").is_none());

    let subscription = prepare_value(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::WebSocket,
        caller,
    );
    assert_eq!(
        subscription
            .headers
            .get("x-codex-routing-hint")
            .and_then(|value| value.to_str().ok()),
        Some("model=gpt-5.4")
    );
    assert_eq!(subscription.value["stream_id"], "stream-caller");
    assert!(subscription.value.get("background").is_none());
    assert!(subscription.value.get("stream").is_none());
}

#[test]
fn web_and_file_search_filters_use_disjoint_documented_schemas() {
    let prepared = prepare_value(
        UpstreamProfile::CodexOpenAi149,
        EmulationTransport::Http,
        json!({
            "model":"gpt-5.4",
            "input":[],
            "tools":[
                {"type":"web_search","filters":{
                    "allowed_domains":["example.test"],
                    "key":"file_kind","type":"eq","value":"manual","unknown":true
                }},
                {"type":"file_search","vector_store_ids":["vs_1"],"filters":{
                    "type":"and","filters":[
                        {"type":"eq","key":"kind","value":"manual","allowed_domains":["wrong.test"],"unknown":true}
                    ],"allowed_domains":["wrong.test"],"unknown":true
                }},
                {"type":"web_search_preview","filters":{"allowed_domains":["wrong.test"]},
                    "external_web_access":false,"search_context_size":"low"}
            ]
        }),
    );
    let tools = prepared.value["tools"].as_array().expect("tools");
    assert_eq!(
        tools[0]["filters"],
        json!({"allowed_domains":["example.test"]})
    );
    assert_eq!(
        tools[1]["filters"],
        json!({"type":"and","filters":[{"type":"eq","key":"kind","value":"manual"}]})
    );
    assert!(tools[2].get("filters").is_none());
    assert!(tools[2].get("external_web_access").is_none());
    assert_eq!(tools[2]["search_context_size"], "low");
}

#[test]
fn documented_replay_items_survive_while_structured_unknowns_are_stripped() {
    let prepared = prepare_value(
        UpstreamProfile::CodexOpenAi149,
        EmulationTransport::Http,
        json!({
            "model":"gpt-5.4",
            "input":[
                {"type":"message","role":"system","content":[
                    {"type":"input_text","text":"rules","prompt_cache_breakpoint":{"mode":"explicit","unknown":true}}
                ]},
                {"type":"message","id":"msg_prior","role":"assistant","status":"completed","content":[
                    {"type":"output_text","text":"prior","annotations":[
                        {"type":"url_citation","url":"https://example.test","title":"source","start_index":0,"end_index":5,"unknown":true}
                    ],"logprobs":[{"token":"x","bytes":[120],"logprob":-1.0,
                        "top_logprobs":[{"token":"y","bytes":[121],"logprob":-2.0,"unknown":true}],"unknown":true}]}
                ]},
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":{
                    "opaque":{"type":"input_image","image_url":"opaque","unknown":true}
                },"caller":{"type":"program","caller_id":"prog_1","unknown":true}},
                {"type":"function_call_output","call_id":"call_1","name":"lookup","namespace":"functions",
                    "output":[{"type":"input_image","image_url":"typed"}],
                    "caller":{"type":"direct","unknown":true}},
                {"type":"file_search_call","id":"fs_1","queries":["q"],"status":"completed",
                    "results":[{"file_id":"file_1","filename":"a.txt","score":0.8,"text":"hit",
                        "attributes":{"opaque":true},"unknown":true}]},
                {"type":"computer_call","id":"cc_1","call_id":"call_2","status":"completed",
                    "pending_safety_checks":[{"id":"safe_1","code":"policy","message":"review","unknown":true}],
                    "actions":[{"type":"type","text":"hello","unknown":true}]},
                {"type":"apply_patch_call","id":"ap_1","call_id":"call_3","status":"completed",
                    "operation":{"type":"update_file","path":"a.txt","diff":"@@","unknown":true},
                    "caller":{"type":"program","caller_id":"prog_1","unknown":true}},
                {"type":"mcp_list_tools","id":"mcp_1","server_label":"srv","tools":[{
                    "name":"lookup","description":"Lookup","input_schema":{"type":"object","x-extension":true},
                    "annotations":{"opaque":true},"unknown":true
                }]},
                {"type":"program","id":"prog_1","call_id":"call_4","code":"run()","status":"completed",
                    "environment":{"type":"local","unknown":true},"fingerprint":"fp_required","unknown":true}
            ]
        }),
    );
    let input = prepared.value["input"].as_array().expect("input items");
    assert_eq!(input[0]["role"], "system");
    assert!(
        input[0]["content"][0]["prompt_cache_breakpoint"]
            .get("unknown")
            .is_none()
    );
    assert!(
        input[1]["content"][0]["annotations"][0]
            .get("unknown")
            .is_none()
    );
    assert!(
        input[1]["content"][0]["logprobs"][0]
            .get("unknown")
            .is_none()
    );
    assert!(
        input[1]["content"][0]["logprobs"][0]["top_logprobs"][0]
            .get("unknown")
            .is_none()
    );
    assert_eq!(input[2]["caller"]["caller_id"], "prog_1");
    assert!(input[2]["caller"].get("unknown").is_none());
    assert_eq!(input[2]["arguments"]["opaque"]["unknown"], true);
    assert!(input[2]["arguments"]["opaque"].get("detail").is_none());
    assert_eq!(input[3]["name"], "lookup");
    assert_eq!(input[3]["namespace"], "functions");
    assert_eq!(input[3]["output"][0]["detail"], "high");
    assert_eq!(input[4]["results"][0]["attributes"]["opaque"], true);
    assert!(input[4]["results"][0].get("unknown").is_none());
    assert_eq!(input[5]["actions"][0]["text"], "hello");
    assert!(input[5]["actions"][0].get("unknown").is_none());
    assert_eq!(input[6]["operation"]["diff"], "@@");
    assert!(input[6]["operation"].get("unknown").is_none());
    assert_eq!(input[7]["tools"][0]["input_schema"]["x-extension"], true);
    assert_eq!(input[7]["tools"][0]["annotations"]["opaque"], true);
    assert!(input[7]["tools"][0].get("unknown").is_none());
    assert_eq!(input[8]["fingerprint"], "fp_required");
    assert!(input[8]["environment"].get("unknown").is_none());
    assert!(input[8].get("unknown").is_none());
}

#[test]
fn agent_and_shell_replay_fields_are_filtered_at_documented_boundaries() {
    for profile in [
        UpstreamProfile::CodexOpenAi149,
        UpstreamProfile::CodexSubscription149,
    ] {
        for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
            let prepared = prepare_value(
                profile,
                transport,
                json!({
                    "type":"response.create",
                    "model":"gpt-5.4",
                    "input":[
                        {"type":"agent_message","content":[],"agent":{"agent_name":"worker","unknown":true}},
                        {"type":"file_search_call","id":"fs","queries":[],"status":"completed",
                            "agent":{"agent_name":"worker","unknown":true}},
                        {"type":"computer_call","id":"cc","call_id":"call_cc","status":"completed",
                            "agent":{"agent_name":"worker","unknown":true}},
                        {"type":"shell_call","id":"sh","call_id":"call_sh","status":"completed",
                            "action":{"commands":["true"],"max_output_length":1024,"timeout_ms":1000,"unknown":true},
                            "agent":{"agent_name":"worker","unknown":true}},
                        {"type":"shell_call_output","id":"sho","call_id":"call_sh","status":"completed",
                            "max_output_length":1024,"output":[{
                                "outcome":{"type":"exit","exit_code":0,"unknown":true},
                                "stdout":"ok","stderr":"","created_by":"worker","unknown":true
                            }],"agent":{"agent_name":"worker","unknown":true}},
                        {"type":"apply_patch_call","id":"ap","call_id":"call_ap","status":"completed",
                            "operation":{"type":"delete_file","path":"old.txt"},
                            "agent":{"agent_name":"worker","unknown":true}},
                        {"type":"web_search_call","id":"ws","status":"completed",
                            "action":{"type":"search","queries":[]},
                            "agent":{"agent_name":"worker","unknown":true}},
                        {"type":"compaction_trigger","agent":{"agent_name":"worker","unknown":true}},
                        {"type":"item_reference","id":"ref","agent":{"agent_name":"worker","unknown":true}}
                    ]
                }),
            );
            let input = prepared.value["input"].as_array().expect("input items");
            for item in input {
                if let Some(agent) = item.get("agent") {
                    assert_eq!(agent["agent_name"], "worker");
                    assert!(agent.get("unknown").is_none());
                }
            }
            let shell = item(input, "shell_call");
            assert_eq!(shell["action"]["max_output_length"], 1024);
            assert!(shell["action"].get("unknown").is_none());
            let output = &item(input, "shell_call_output")["output"][0];
            assert_eq!(output["created_by"], "worker");
            assert_eq!(output["outcome"]["exit_code"], 0);
            assert!(output.get("unknown").is_none());
            assert!(output["outcome"].get("unknown").is_none());
        }
    }
}

#[test]
fn moderation_policy_children_are_filtered_but_supported_modes_survive() {
    let prepared = prepare_value(
        UpstreamProfile::CodexOpenAi149,
        EmulationTransport::Http,
        json!({
            "model":"gpt-5.4",
            "input":[],
            "moderation":{
                "model":"omni-moderation-latest",
                "policy":{
                    "input":{"mode":"block","unknown":true},
                    "output":{"mode":"monitor","unknown":true},
                    "unknown":true
                }
            }
        }),
    );
    assert_eq!(
        prepared.value["moderation"]["policy"]["input"]["mode"],
        "block"
    );
    assert_eq!(
        prepared.value["moderation"]["policy"]["output"]["mode"],
        "monitor"
    );
    assert!(
        prepared.value["moderation"]["policy"]
            .get("unknown")
            .is_none()
    );
    assert!(
        prepared.value["moderation"]["policy"]["input"]
            .get("unknown")
            .is_none()
    );
}

struct PreparedValue {
    headers: HeaderMap,
    value: Value,
}

fn item<'a>(items: &'a [Value], kind: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some(kind))
        .expect("item kind")
}

fn prepare_value(
    profile: UpstreamProfile,
    transport: EmulationTransport,
    caller: Value,
) -> PreparedValue {
    let identity = profile
        .uses_codex_subscription()
        .then_some(SubscriptionIdentity {
            account_namespace: "account-test",
            downstream_scope: PSEUDONYM_SCOPE,
        });
    let prepared = prepare_emulated_request(
        profile,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&caller).expect("caller JSON")),
        1024 * 1024,
        identity,
    )
    .expect("emulated request");
    PreparedValue {
        headers: prepared.headers,
        value: serde_json::from_slice(&prepared.body).expect("emulated JSON"),
    }
}
