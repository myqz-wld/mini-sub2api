use super::*;

fn project(mut caller: Value, transport: EmulationTransport) -> (Value, Value) {
    caller["input"] = "Translate this synthetic sentence.".into();
    caller["instructions"] = "Only translate the supplied text.".into();
    let prepared = prepare_codex_overlay_for_test(
        UpstreamProfile::CodexSubscription1592,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&caller).unwrap()),
        1024 * 1024,
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&prepared.body).unwrap();
    let header: Value =
        serde_json::from_str(prepared.headers["x-codex-turn-metadata"].to_str().unwrap()).unwrap();
    (value, header)
}

#[test]
fn bare_model_defaults_do_not_invent_environment_or_tools() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for (model, needs_review) in [
            ("gpt-6-astra", true),
            ("gpt-6-astra-preview", true),
            ("vendor/gpt-6-astra-preview", true),
            ("gpt-6.1-sol", true),
            ("gpt-6.1-sol-preview", true),
            ("vendor/gpt-6.1-sol-preview", true),
            ("gpt-5.4", false),
            ("vendor/group/gpt-6-astra", false),
            ("vendor/group/gpt-6.1-sol", false),
        ] {
            let (value, header) = project(serde_json::json!({"model": model}), transport);
            let turn: Value = serde_json::from_str(
                value["client_metadata"]["x-codex-turn-metadata"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(turn, header);
            assert_eq!(turn["node_repl_auto_review_required"], needs_review);
            assert_eq!(turn["node_repl_disabled"], false);
            assert_eq!(turn["auto_review_enabled"], false);
            assert_eq!(turn["agent_name"], "/root");
            assert!(turn.get("workspaces").is_none());
            assert!(turn.get("tool_namespaces_info").is_none());
            let input = value["input"].as_array().unwrap();
            if input[0]["type"] == "additional_tools" {
                assert_eq!(input.len(), 3);
                assert_eq!(input[0]["tools"], serde_json::json!([]));
                assert_eq!(
                    input[1]["content"][0]["text"],
                    "Only translate the supplied text."
                );
            } else {
                assert_eq!(input.len(), 1);
                assert_eq!(value["instructions"], "Only translate the supplied text.");
                assert_eq!(value["tools"], serde_json::json!([]));
            }
            assert_eq!(input.last().unwrap()["role"], "user");
            assert_eq!(
                input.last().unwrap()["content"][0]["text"],
                "Translate this synthetic sentence."
            );
        }
    }
}

#[test]
fn partial_caller_metadata_keeps_explicit_execution_policy() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-6-astra", "gpt-6.1-sol", "gpt-5.4"] {
            let supplied = serde_json::json!({
                "node_repl_auto_review_required": false,
                "node_repl_disabled": true,
                "auto_review_enabled": true,
                "agent_name": "/root/translator",
                "workspaces": {"/caller/project": {"writable_roots": []}}
            });
            let (value, header) = project(
                serde_json::json!({
                    "model": model,
                    "client_metadata": {"x-codex-turn-metadata": supplied.to_string()}
                }),
                transport,
            );
            let turn: Value = serde_json::from_str(
                value["client_metadata"]["x-codex-turn-metadata"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            for (key, expected) in supplied.as_object().unwrap() {
                assert_eq!(&turn[key], expected);
                assert_eq!(&header[key], expected);
            }
        }
    }
}

#[test]
fn sol_61_uses_lite_defaults_without_inserting_model_instructions() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for (model, effort) in [
            ("gpt-6.1-sol", "low"),
            ("gpt-6.1-sol-preview", "low"),
            ("vendor/gpt-6.1-sol-preview", "low"),
            ("gpt-6-sol", "medium"),
        ] {
            let caller = serde_json::json!({
                "model": model,
                "input": [{"role": "user", "content": [
                    {"type": "input_text", "text": "Synthetic image probe"},
                    {"type": "input_image", "image_url": "data:image/png;base64,AA==", "detail": "original"}
                ]}],
                "tools": [{"type": "function", "name": "probe", "parameters": {"type": "object"}}]
            });
            let prepared = prepare_codex_overlay_for_test(
                UpstreamProfile::CodexSubscription1592,
                transport,
                &HeaderMap::new(),
                Bytes::from(serde_json::to_vec(&caller).unwrap()),
                1024 * 1024,
            )
            .unwrap();
            let value: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert_eq!(
                value["reasoning"],
                serde_json::json!({"effort": effort, "context": "all_turns"})
            );
            assert_eq!(value["text"]["verbosity"], "low");
            assert_eq!(value["parallel_tool_calls"], false);
            assert!(value.get("instructions").is_none());
            let input = value["input"].as_array().unwrap();
            assert_eq!(input.len(), 2, "model {model}");
            assert_eq!(input[0]["type"], "additional_tools");
            assert_eq!(input[0]["tools"][0]["name"], "functions");
            assert_eq!(input[0]["tools"][0]["tools"][0]["name"], "probe");
            assert_eq!(input[1]["role"], "user");
            assert_eq!(
                input[1]["content"][1]["image_url"],
                caller["input"][0]["content"][1]["image_url"]
            );
            assert!(input[1]["content"][1].get("detail").is_none());
        }
    }
}

#[test]
fn sol_61_maps_ultra_to_xhigh_and_preserves_priority_and_explicit_verbosity() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-6.1-sol", "gpt-6.1-sol-preview", "vendor/gpt-6.1-sol"] {
            let (value, _) = project(
                serde_json::json!({
                    "model": model,
                    "reasoning": {"effort": "ultra", "summary": "detailed"},
                    "service_tier": "priority",
                    "text": {"verbosity": "high"}
                }),
                transport,
            );
            assert_eq!(
                value["reasoning"],
                serde_json::json!({
                    "effort": "xhigh", "summary": "detailed", "context": "all_turns"
                })
            );
            assert_eq!(value["service_tier"], "priority");
            assert_eq!(value["text"]["verbosity"], "high");
            assert_eq!(
                value["input"][1]["content"][0]["text"],
                "Only translate the supplied text."
            );
        }
    }
}
