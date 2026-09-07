use super::*;
use crate::codex_instructions;
use pretty_assertions::assert_eq;

const PROFILES: [UpstreamProfile; 2] = [
    UpstreamProfile::CodexSubscription1534,
    UpstreamProfile::CodexSubscription1534,
];
const TRANSPORTS: [EmulationTransport; 2] =
    [EmulationTransport::Http, EmulationTransport::WebSocket];

#[test]
fn normal_and_converted_lite_only_use_caller_base_and_preserve_history() {
    for profile in PROFILES {
        for transport in TRANSPORTS {
            for (model, lite) in [("gpt-5.4", false), ("gpt-5.6-sol", true)] {
                for (instructions, expected) in instruction_cases() {
                    let history = caller_history();
                    let mut caller = serde_json::json!({
                        "type":"response.create","model":model,"input":history,
                        "tools":[{"type":"function","name":"lookup"}]
                    });
                    set_instructions(&mut caller, instructions);
                    let normalized =
                        prepare(profile, caller, transport).expect("normalized request");
                    let input = normalized["input"].as_array().expect("input");
                    let offset = if lite {
                        assert!(normalized.get("instructions").is_none());
                        assert!(normalized.get("tools").is_none());
                        assert_eq!(input[0]["type"], "additional_tools");
                        if let Some(base) = &expected {
                            assert_developer_text(&input[1], base);
                            2
                        } else {
                            1
                        }
                    } else {
                        if let Some(expected) = expected {
                            assert_eq!(normalized["instructions"], expected);
                        } else {
                            assert!(normalized.get("instructions").is_none());
                        }
                        0
                    };
                    assert_history(&input[offset..], &history, profile);
                }
            }
        }
    }
}

#[test]
fn native_lite_preserves_input_and_only_inserts_an_explicit_valid_base() {
    for profile in PROFILES {
        for transport in TRANSPORTS {
            // A leading additional_tools item also selects Lite for a normally non-Lite model.
            for model in ["gpt-5.6-terra", "gpt-5.4-mini"] {
                for (instructions, expected) in instruction_cases() {
                    // Native Lite must not invent an input base even when there is none already.
                    for history in [Vec::new(), caller_history()] {
                        let mut input = vec![serde_json::json!({
                            "type":"additional_tools","role":"developer","tools":[]
                        })];
                        input.extend(history.clone());
                        let mut caller = serde_json::json!({
                            "type":"response.create","model":model,"input":input
                        });
                        set_instructions(&mut caller, instructions.clone());
                        let normalized =
                            prepare(profile, caller, transport).expect("normalized request");
                        assert!(normalized.get("instructions").is_none());
                        assert!(normalized.get("tools").is_none());
                        let input = normalized["input"].as_array().expect("input");
                        assert_eq!(input[0]["type"], "additional_tools");
                        let offset = if let Some(base) = &expected {
                            assert_developer_text(&input[1], base);
                            2
                        } else {
                            1
                        };
                        assert_history(&input[offset..], &history, profile);
                    }
                }
            }
        }
    }
}

#[test]
fn lite_incremental_websocket_ignores_all_absent_or_invalid_top_level_bases() {
    for profile in PROFILES {
        for instructions in invalid_instructions() {
            for history in [Vec::new(), caller_history()] {
                let mut caller = serde_json::json!({
                    "type":"response.create","model":"gpt-5.6-luna",
                    "previous_response_id":"resp_previous","input":history
                });
                set_instructions(&mut caller, instructions.clone());
                let normalized = prepare(profile, caller, EmulationTransport::WebSocket)
                    .expect("normalized request");
                assert!(normalized.get("instructions").is_none());
                assert!(normalized.get("tools").is_none());
                assert_eq!(normalized["previous_response_id"], "resp_previous");
                assert_history(
                    normalized["input"].as_array().expect("input"),
                    &history,
                    profile,
                );
            }
        }
    }
}

#[test]
fn lite_base_is_only_inserted_for_explicit_valid_instructions() {
    for profile in PROFILES {
        for (transport, previous, tools, instructions, expected) in [
            (EmulationTransport::Http, true, false, Value::Null, None),
            (
                EmulationTransport::WebSocket,
                false,
                false,
                Value::Null,
                None,
            ),
            (EmulationTransport::WebSocket, true, true, Value::Null, None),
            (
                EmulationTransport::WebSocket,
                true,
                false,
                Value::String("  explicit continuation base\n".to_string()),
                Some("  explicit continuation base\n"),
            ),
        ] {
            let mut caller = serde_json::json!({
                "type":"response.create","model":"gpt-5.6-luna",
                "input":[],"instructions":instructions
            });
            if previous {
                caller["previous_response_id"] = Value::String("resp_previous".to_string());
            }
            if tools {
                caller["tools"] = serde_json::json!([]);
            }
            let normalized = prepare(profile, caller, transport).expect("normalized request");
            assert!(normalized.get("instructions").is_none());
            let input = normalized["input"].as_array().expect("input");
            assert_eq!(input.len(), 1 + usize::from(expected.is_some()));
            assert_eq!(input[0]["type"], "additional_tools");
            if let Some(expected) = expected {
                assert_developer_text(&input[1], expected);
            }
        }
    }
}

#[test]
fn normal_responses_continuation_omits_missing_base() {
    for profile in PROFILES {
        let normalized = prepare(
            profile,
            serde_json::json!({
                "type":"response.create","model":"gpt-5.4",
                "previous_response_id":"resp_previous","input":[]
            }),
            EmulationTransport::WebSocket,
        )
        .expect("normalized request");
        assert!(normalized.get("instructions").is_none());
        assert_eq!(normalized["input"], serde_json::json!([]));
    }
}

#[test]
fn base_selection_handles_missing_and_string_input_without_extra_developer_messages() {
    for profile in PROFILES {
        for transport in TRANSPORTS {
            for (model, lite) in [("gpt-5.4", false), ("gpt-5.6-sol", true)] {
                for input in [None, Some(Value::String("hello".to_string()))] {
                    let mut caller = serde_json::json!({
                        "model":model,"instructions":"caller base"
                    });
                    if let Some(input) = &input {
                        caller["input"] = input.clone();
                    }
                    let normalized =
                        prepare(profile, caller, transport).expect("normalized request");
                    if lite {
                        let items = normalized["input"].as_array().expect("input");
                        assert_eq!(items.len(), 2 + usize::from(input.is_some()));
                        assert_eq!(items[0]["type"], "additional_tools");
                        assert_developer_text(&items[1], "caller base");
                    } else {
                        assert_eq!(normalized["instructions"], "caller base");
                        assert_eq!(normalized.get("input").is_some(), input.is_some());
                    }
                    if input.is_some() {
                        let items = normalized["input"].as_array().expect("input");
                        let user = items.last().expect("user message");
                        assert_eq!(user["role"], "user");
                        assert_eq!(user["content"][0]["text"], "hello");
                    }
                }
            }
        }
    }
}

#[test]
fn invalid_input_still_fails_closed_and_caller_base_respects_request_size_limit() {
    for model in ["gpt-5.4", "gpt-5.6-sol"] {
        assert!(
            prepare(
                UpstreamProfile::CodexSubscription1534,
                serde_json::json!({
                    "model":model,"instructions":"caller base","input":{"invalid":"shape"}
                }),
                EmulationTransport::Http,
            )
            .is_err()
        );
        let caller = serde_json::json!({
            "model":model,"input":[],"instructions":"caller base".repeat(128)
        });
        let result = prepare_codex_overlay_for_test(
            UpstreamProfile::CodexSubscription1534,
            EmulationTransport::Http,
            &HeaderMap::new(),
            Bytes::from(serde_json::to_vec(&caller).expect("caller JSON")),
            512,
        );
        assert!(result.is_err());
    }
}

fn instruction_cases() -> Vec<(Option<Value>, Option<String>)> {
    let mut cases = invalid_instructions()
        .into_iter()
        .map(|value| (value, None))
        .collect::<Vec<_>>();
    for text in [
        "caller custom instructions".to_string(),
        " \t保留两端空白\r\n".to_string(),
        "caller template {{ personality }} and {{ untouched }}".to_string(),
        codex_instructions::for_model("gpt-5.4").to_string(),
        format!(
            "{}caller suffix after known base",
            codex_instructions::for_model("gpt-5.4")
        ),
        codex_instructions::for_model("gpt-5.6-sol").replacen(
            "# Personality",
            "# Caller personality",
            1,
        ),
    ] {
        cases.push((Some(Value::String(text.clone())), Some(text)));
    }
    cases
}

fn invalid_instructions() -> Vec<Option<Value>> {
    vec![
        None,
        Some(Value::Null),
        Some(serde_json::json!("")),
        Some(serde_json::json!(" \t\r\n")),
        Some(serde_json::json!("\u{00a0}\u{3000}")),
        Some(serde_json::json!(42)),
        Some(serde_json::json!(0)),
        Some(serde_json::json!(1.5)),
        Some(serde_json::json!(true)),
        Some(serde_json::json!(false)),
        Some(serde_json::json!([])),
        Some(serde_json::json!(["not a base"])),
        Some(serde_json::json!({})),
        Some(serde_json::json!({"text":"not a base"})),
    ]
}

fn caller_history() -> Vec<Value> {
    let known = codex_instructions::for_model("gpt-5.4");
    vec![
        developer_message(known),
        developer_message(&format!("{known}caller developer suffix")),
        serde_json::json!({"type":"message","role":"system","content":[
            {"type":"input_text","text":known}
        ]}),
        serde_json::json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":"first question"}
        ]}),
        serde_json::json!({"type":"message","role":"assistant","content":[
            {"type":"output_text","text":"first answer"}
        ]}),
        serde_json::json!({"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{}"}),
        developer_message("  keep repeated rule\n"),
        serde_json::json!({"type":"function_call_output","call_id":"call_1","output":"result"}),
        developer_message("  keep repeated rule\n"),
        serde_json::json!({"type":"message","role":"developer","content":[
            {"type":"input_text","text":known},{"type":"input_text","text":"second part"}
        ]}),
        serde_json::json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":"continue"}
        ]}),
    ]
}

fn assert_history(actual: &[Value], expected: &[Value], profile: UpstreamProfile) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        for (field, value) in expected.as_object().expect("input item") {
            if field == "role"
                && value == "system"
                && profile == UpstreamProfile::CodexSubscription1534
            {
                assert_eq!(actual[field], "developer");
            } else {
                assert_eq!(&actual[field], value, "history field {field}");
            }
        }
    }
}

fn set_instructions(caller: &mut Value, instructions: Option<Value>) {
    if let Some(instructions) = instructions {
        caller["instructions"] = instructions;
    }
}

fn prepare(
    profile: UpstreamProfile,
    caller: Value,
    transport: EmulationTransport,
) -> Result<Value, ()> {
    let prepared = prepare_codex_overlay_for_test(
        profile,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&caller).expect("caller JSON")),
        1024 * 1024,
    )?;
    serde_json::from_slice(&prepared.body).map_err(|_| ())
}

fn developer_message(text: &str) -> Value {
    serde_json::json!({
        "type":"message","role":"developer","content":[{"type":"input_text","text":text}]
    })
}

fn assert_developer_text(item: &Value, expected: &str) {
    assert_eq!(item["type"], "message");
    assert_eq!(item["role"], "developer");
    assert_eq!(item["content"][0]["type"], "input_text");
    assert_eq!(item["content"][0]["text"], expected);
}
