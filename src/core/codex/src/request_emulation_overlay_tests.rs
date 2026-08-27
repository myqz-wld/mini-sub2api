use super::*;
use crate::request_profile::UpstreamProfile;
use pretty_assertions::assert_eq;

const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn official_explicit_fields_round_trip_and_unknown_top_level_fields_are_stripped() {
    let caller = serde_json::json!({
        "model": "gpt-5.4",
        "instructions": "caller instructions",
        "previous_response_id": "resp_http_explicit",
        "input": [],
        "tools": [{
            "type":"function",
            "name":"lookup",
            "description":"",
            "strict":false,
            "parameters":{"type":"object","x-schema-extension":{"opaque":true}},
            "unsupported_tool_member":{"enabled":true}
        }],
        "tool_choice": "none",
        "parallel_tool_calls": false,
        "reasoning": {"effort":"high","summary":"none"},
        "store": true,
        "stream": false,
        "stream_options": {"include_obfuscation":false},
        "include": ["file_search_call.results"],
        "service_tier": "flex",
        "prompt_cache_key": "caller-cache-key",
        "text": {"verbosity":"high"},
        "background": true,
        "context_management": [{"type":"compaction","compact_threshold":1200}],
        "conversation": "conv_explicit",
        "max_output_tokens": 2048,
        "max_tool_calls": 4,
        "metadata": {"public":"metadata"},
        "moderation": {"model":"omni-moderation-latest","policy":"default"},
        "prompt": {"id":"pmpt_explicit","variables":{"name":"value"}},
        "prompt_cache_options": {"mode":"explicit","ttl":"30m"},
        "prompt_cache_retention": "24h",
        "safety_identifier": "safety_explicit",
        "temperature": 0.25,
        "top_logprobs": 3,
        "top_p": 0.75,
        "truncation": "disabled",
        "user": "user_explicit",
        "future_top_level": {"opaque":[1,2,3]}
    });
    let normalized = prepare_openai(caller.clone(), EmulationTransport::Http);

    for (name, expected) in caller
        .as_object()
        .expect("caller object")
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "future_top_level" | "instructions" | "input" | "tools"
            )
        })
    {
        assert_eq!(&normalized[name], expected, "explicit field {name}");
    }
    assert_eq!(
        normalized["instructions"],
        crate::codex_instructions::for_model("gpt-5.4")
    );
    assert_eq!(normalized["input"][0]["role"], "developer");
    assert_eq!(
        normalized["input"][0]["content"][0]["text"],
        "caller instructions"
    );
    assert!(normalized.get("future_top_level").is_none());
    assert_eq!(normalized["metadata"]["public"], "metadata");
    assert!(
        normalized["tools"][0]
            .get("unsupported_tool_member")
            .is_none()
    );
    assert_eq!(
        normalized["tools"][0]["parameters"]["x-schema-extension"]["opaque"],
        true
    );
}

#[test]
fn explicit_previous_response_id_survives_http_and_websocket_for_both_profiles() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let caller = serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.4",
            "previous_response_id": "resp_explicit",
            "input": []
        });
        let openai = prepare_openai(caller.clone(), transport);
        let subscription = prepare_subscription(caller, transport);
        assert_eq!(openai["previous_response_id"], "resp_explicit");
        assert_eq!(subscription["previous_response_id"], "resp_explicit");
    }
}

#[test]
fn codex_defaults_only_fill_absent_members() {
    let explicit = serde_json::json!({
        "model": "gpt-5.6-sol",
        "input": [],
        "store": true,
        "stream": false,
        "tool_choice": null,
        "parallel_tool_calls": true,
        "reasoning": null,
        "include": ["file_search_call.results"],
        "service_tier": "default",
        "prompt_cache_key": "",
        "text": null,
        "previous_response_id": null
    });
    let normalized = prepare_openai(explicit.clone(), EmulationTransport::Http);
    for name in [
        "store",
        "stream",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "include",
        "service_tier",
        "prompt_cache_key",
        "text",
        "previous_response_id",
    ] {
        assert_eq!(normalized[name], explicit[name], "explicit field {name}");
    }

    let defaults = prepare_openai(
        serde_json::json!({"model":"gpt-5.4","input":[]}),
        EmulationTransport::Http,
    );
    assert_eq!(defaults["store"], false);
    assert_eq!(defaults["stream"], true);
    assert_eq!(defaults["tool_choice"], "auto");
    assert_eq!(defaults["parallel_tool_calls"], true);
    assert_eq!(defaults["reasoning"]["effort"], "medium");
    assert_eq!(
        defaults["include"],
        serde_json::json!(["reasoning.encrypted_content"])
    );
}

#[test]
fn image_detail_defaults_are_profile_aware_and_explicit_values_are_authoritative() {
    let input = serde_json::json!([
        {"type":"message","role":"user","content":[
            {"type":"input_image","image_url":"normal-missing"},
            {"type":"input_image","image_url":"explicit-low","detail":"low"}
        ]},
        {"type":"function_call_output","call_id":"call_1","output":[
            {"type":"input_image","image_url":"structured-missing"},
            {"type":"input_image","image_url":"explicit-null","detail":null}
        ]}
    ]);
    let normal = prepare_openai(
        serde_json::json!({"model":"gpt-5.4","input":input.clone()}),
        EmulationTransport::Http,
    );
    assert_image_detail(&normal, "normal-missing", Some(&serde_json::json!("high")));
    assert_image_detail(
        &normal,
        "structured-missing",
        Some(&serde_json::json!("high")),
    );
    assert_image_detail(&normal, "explicit-low", Some(&serde_json::json!("low")));
    assert_image_detail(&normal, "explicit-null", Some(&Value::Null));

    let lite = prepare_openai(
        serde_json::json!({"model":"gpt-5.6-sol","input":input}),
        EmulationTransport::Http,
    );
    assert_image_detail(&lite, "normal-missing", None);
    assert_image_detail(&lite, "structured-missing", None);
    assert_image_detail(&lite, "explicit-low", Some(&serde_json::json!("low")));
    assert_image_detail(&lite, "explicit-null", Some(&Value::Null));
}

#[test]
fn lite_relocation_preserves_controls_and_filters_structured_tools() {
    let normalized = prepare_openai(
        serde_json::json!({
            "model": "gpt-5.6-sol",
            "instructions": "developer guidance",
            "input": [{"role":"user","content":"hello"}],
            "tools": [
                {"type":"namespace","name":"functions","description":"existing","tools":[],"future_namespace":9},
                {"type":"function","name":"lookup","future_function":{"opaque":true}},
                {"type":"custom","name":"exec","description":"","future_custom":true,
                    "format":{"type":"grammar","syntax":"lark","definition":"start: WORD","future_format":true}}
            ],
            "store": true,
            "stream": false,
            "parallel_tool_calls": true,
            "future_top_level": "strip-me"
        }),
        EmulationTransport::Http,
    );
    assert!(normalized.get("tools").is_none());
    assert!(normalized.get("instructions").is_none());
    assert_eq!(normalized["input"][0]["type"], "additional_tools");
    let namespace = &normalized["input"][0]["tools"][0];
    assert!(namespace.get("future_namespace").is_none());
    assert!(namespace["tools"][0].get("future_function").is_none());
    assert!(namespace["tools"][1].get("future_custom").is_none());
    assert!(
        namespace["tools"][1]["format"]
            .get("future_format")
            .is_none()
    );
    assert_eq!(namespace["tools"][1]["format"]["definition"], "start: WORD");
    assert_eq!(
        normalized["input"][1]["content"][0]["text"],
        crate::codex_instructions::for_model("gpt-5.6-sol")
    );
    assert_eq!(normalized["input"][2]["role"], "developer");
    assert_eq!(
        normalized["input"][2]["content"][0]["text"],
        "developer guidance"
    );
    assert_eq!(normalized["input"][3]["role"], "user");
    assert_eq!(normalized["store"], true);
    assert_eq!(normalized["stream"], false);
    assert_eq!(normalized["parallel_tool_calls"], true);
    assert!(normalized.get("future_top_level").is_none());
}

#[test]
fn structured_objects_strip_unknown_members_but_free_form_values_remain_opaque() {
    let normalized = prepare_openai(
        serde_json::json!({
            "model":"gpt-5.4",
            "input":[
                {"type":"function_call","name":"lookup","call_id":"call_1",
                    "arguments":{"arbitrary":{"nested":true}},"unsupported_item":true},
                {"type":"custom_tool_call","name":"exec","call_id":"call_2",
                    "input":{"arbitrary_input":[1,2]},"unsupported_item":true},
                {"type":"function_call_output","call_id":"call_1",
                    "output":{"arbitrary_output":{"keep":true}},"unsupported_item":true}
            ],
            "tools":[
                {"type":"custom","name":"exec","description":"","unsupported_tool":true,
                    "format":{"type":"grammar","syntax":"lark","definition":"start: WORD","unsupported_format":true}},
                {"type":"function","name":"lookup","description":"","strict":false,
                    "parameters":{"type":"object","x-schema-extension":{"keep":true}},"unsupported_tool":true}
            ],
            "metadata":{"arbitrary_metadata":{"keep":true}},
            "prompt":{"id":"prompt_1","version":"1","variables":{"arbitrary_variable":{"keep":true}},"unsupported_prompt":true},
            "prompt_cache_options":{"mode":"explicit","ttl":"30m","unsupported_cache":true},
            "conversation":{"id":"conversation_1","unsupported_conversation":true},
            "moderation":{"model":"omni-moderation-latest","policy":"default","unsupported_moderation":true},
            "context_management":[{"type":"compaction","compact_threshold":1200,"unsupported_context":true}],
            "tool_choice":{"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"lookup","unsupported_ref":true}],"unsupported_choice":true},
            "reasoning":{"effort":"high","mode":"standard","unsupported_reasoning":true},
            "stream_options":{"include_obfuscation":false,"reasoning_summary_delivery":"sequential_cutoff","unsupported_stream":true},
            "text":{"verbosity":"high","unsupported_text":true,"format":{
                "type":"json_schema","name":"caller_schema","strict":false,"description":"schema",
                "schema":{"type":"object","x-schema-extension":{"keep":true}},"unsupported_format":true
            }},
            "client_metadata":{
                "session_id":"session_1",
                "x-codex-turn-metadata":"{\"request_kind\":\"turn\",\"unsupported_turn\":true}",
                "unsupported_client":true
            }
        }),
        EmulationTransport::Http,
    );

    for path in [
        normalized["tools"][0].get("unsupported_tool"),
        normalized["tools"][0]["format"].get("unsupported_format"),
        normalized["tools"][1].get("unsupported_tool"),
        normalized["prompt"].get("unsupported_prompt"),
        normalized["prompt_cache_options"].get("unsupported_cache"),
        normalized["conversation"].get("unsupported_conversation"),
        normalized["moderation"].get("unsupported_moderation"),
        normalized["context_management"][0].get("unsupported_context"),
        normalized["tool_choice"].get("unsupported_choice"),
        normalized["tool_choice"]["tools"][0].get("unsupported_ref"),
        normalized["reasoning"].get("unsupported_reasoning"),
        normalized["stream_options"].get("unsupported_stream"),
        normalized["text"].get("unsupported_text"),
        normalized["text"]["format"].get("unsupported_format"),
    ] {
        assert!(path.is_none());
    }
    assert_eq!(normalized["metadata"]["arbitrary_metadata"]["keep"], true);
    assert_eq!(
        normalized["prompt"]["variables"]["arbitrary_variable"]["keep"],
        true
    );
    assert_eq!(
        normalized["tools"][1]["parameters"]["x-schema-extension"]["keep"],
        true
    );
    assert_eq!(
        normalized["text"]["format"]["schema"]["x-schema-extension"]["keep"],
        true
    );
    assert_eq!(
        normalized["input"][0]["arguments"]["arbitrary"]["nested"],
        true
    );
    assert_eq!(normalized["input"][1]["input"]["arbitrary_input"][1], 2);
    assert_eq!(
        normalized["input"][2]["output"]["arbitrary_output"]["keep"],
        true
    );
    assert_eq!(normalized["client_metadata"]["unsupported_client"], true);
    for item in normalized["input"].as_array().expect("items") {
        assert!(item.get("unsupported_item").is_none());
    }
    let turn: Value = serde_json::from_str(
        normalized["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    assert!(turn.get("unsupported_turn").is_none());
}

#[test]
fn bare_profile_and_mismatched_subscription_identity_fail_closed() {
    let body = Bytes::from_static(br#"{"model":"gpt-5.4","input":[]}"#);
    assert!(
        prepare_emulated_request(
            UpstreamProfile::BareOpenAi,
            EmulationTransport::Http,
            &HeaderMap::new(),
            body.clone(),
            16 * 1024,
            None,
        )
        .is_err()
    );
    assert!(
        prepare_emulated_request(
            UpstreamProfile::CodexSubscription149,
            EmulationTransport::Http,
            &HeaderMap::new(),
            body,
            16 * 1024,
            None,
        )
        .is_err()
    );
}

#[test]
fn system_message_roles_are_rewritten_only_for_subscription() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let caller = serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.4",
            "input": [
                {"role":"system","content":"system rules"},
                {"type":"message","role":"developer","content":"developer rules"},
                {"type":"function_call","call_id":"call_1","name":"lookup",
                    "arguments":{"role":"system"}}
            ]
        });

        let openai = prepare_openai(caller.clone(), transport);
        assert_eq!(openai["input"][0]["role"], "system");
        assert_eq!(openai["input"][1]["role"], "developer");
        assert_eq!(openai["input"][2]["arguments"]["role"], "system");

        let subscription = prepare_subscription(caller, transport);
        assert_eq!(subscription["input"][0]["role"], "developer");
        assert_eq!(subscription["input"][1]["role"], "developer");
        assert_eq!(subscription["input"][2]["arguments"]["role"], "system");
        assert_eq!(
            subscription["input"][0]["content"][0]["text"],
            "system rules"
        );
    }

    let lite = prepare_subscription(
        serde_json::json!({
            "type":"response.create",
            "model":"gpt-5.6-sol",
            "previous_response_id":"resp_lite",
            "input":[
                {"type":"additional_tools","role":"developer","tools":[]},
                {"type":"message","role":"system","content":[
                    {"type":"input_text","text":"lite system rules"}
                ]}
            ]
        }),
        EmulationTransport::WebSocket,
    );
    assert_eq!(lite["input"][1]["role"], "developer");
    assert_eq!(lite["input"][1]["content"][0]["text"], "lite system rules");
}

#[test]
fn output_cap_and_sampling_controls_are_filtered_only_for_subscription() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let caller = serde_json::json!({
            "type":"response.create",
            "model":"gpt-5.4",
            "input":[],
            "max_output_tokens":2048,
            "max_tool_calls":3,
            "temperature":0.2,
            "top_p":0.9
        });
        let openai = prepare_openai(caller.clone(), transport);
        assert_eq!(openai["max_output_tokens"], 2048);
        assert_eq!(openai["temperature"], 0.2);
        assert_eq!(openai["top_p"], 0.9);

        let subscription = prepare_subscription(caller, transport);
        for field in ["max_output_tokens", "temperature", "top_p"] {
            assert!(subscription.get(field).is_none(), "field {field} crossed");
        }
        assert_eq!(subscription["max_tool_calls"], 3);
    }
}

fn prepare_openai(caller: Value, transport: EmulationTransport) -> Value {
    prepare(UpstreamProfile::CodexOpenAi149, caller, transport, None)
}

fn prepare_subscription(caller: Value, transport: EmulationTransport) -> Value {
    prepare(
        UpstreamProfile::CodexSubscription149,
        caller,
        transport,
        Some(SubscriptionIdentity {
            account_namespace: "account-test",
            downstream_scope: PSEUDONYM_SCOPE,
        }),
    )
}

fn prepare(
    profile: UpstreamProfile,
    caller: Value,
    transport: EmulationTransport,
    identity: Option<SubscriptionIdentity<'_>>,
) -> Value {
    let body = Bytes::from(serde_json::to_vec(&caller).expect("caller JSON"));
    let prepared = prepare_emulated_request(
        profile,
        transport,
        &HeaderMap::new(),
        body,
        1024 * 1024,
        identity,
    )
    .expect("emulated request");
    serde_json::from_slice(&prepared.body).expect("emulated JSON")
}

fn assert_image_detail(request: &Value, image_url: &str, expected: Option<&Value>) {
    let image = find_image(request, image_url).expect("image content");
    assert_eq!(image.get("detail"), expected, "image {image_url}");
}

fn find_image<'a>(value: &'a Value, image_url: &str) -> Option<&'a serde_json::Map<String, Value>> {
    match value {
        Value::Array(values) => values.iter().find_map(|value| find_image(value, image_url)),
        Value::Object(object) => {
            if object.get("image_url").and_then(Value::as_str) == Some(image_url) {
                return Some(object);
            }
            object
                .values()
                .find_map(|value| find_image(value, image_url))
        }
        _ => None,
    }
}
