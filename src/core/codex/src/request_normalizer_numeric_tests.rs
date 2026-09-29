use super::*;
use serde_json::json;

#[test]
fn numeric_effort_uses_u64_wire_values_and_selected_string_metadata() {
    let cases = [
        (json!("8192"), json!(8192), "8192"),
        (json!(8192), json!(8192), "8192"),
        (json!("0008192"), json!(8192), "0008192"),
        (json!("+8192"), json!(8192), "+8192"),
        (json!("0"), json!(0), "0"),
        (json!(u64::MAX), json!(u64::MAX), "18446744073709551615"),
        (
            json!("18446744073709551615"),
            json!(u64::MAX),
            "18446744073709551615",
        ),
        (
            json!("18446744073709551616"),
            json!("18446744073709551616"),
            "18446744073709551616",
        ),
        (json!("-1"), json!("-1"), "-1"),
        (json!("1.5"), json!("1.5"), "1.5"),
        (json!(" 8192"), json!(" 8192"), " 8192"),
        (json!("future"), json!("future"), "future"),
    ];
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-5.5", "gpt-6-sol", "gpt-6-luna"] {
            for (selected, expected, metadata_effort) in &cases {
                let request = json!({"model":model,"input":"synthetic numeric probe",
                    "reasoning":{"effort":selected}});
                let prepared = prepare_codex_overlay_for_test(
                    UpstreamProfile::CodexSubscription1580,
                    transport,
                    &HeaderMap::new(),
                    Bytes::from(request.to_string()),
                    64 * 1024,
                )
                .unwrap();
                let body: Value = serde_json::from_slice(&prepared.body).unwrap();
                assert_eq!(&body["reasoning"]["effort"], expected);
                let metadata: Value = serde_json::from_str(
                    body["client_metadata"]["x-codex-turn-metadata"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(metadata["reasoning_effort"], *metadata_effort);
            }
        }
    }
}

#[test]
fn invalid_numeric_types_fall_back_without_changing_numeric_strings() {
    for effort in [json!(-1), json!(1.5), json!(true), json!({}), Value::Null] {
        let request = json!({"model":"gpt-6-sol","input":"synthetic",
            "reasoning":{"effort":effort}});
        let prepared = prepare_codex_overlay_for_test(
            UpstreamProfile::CodexSubscription1580,
            EmulationTransport::Http,
            &HeaderMap::new(),
            Bytes::from(request.to_string()),
            64 * 1024,
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(body["reasoning"]["effort"], "medium");
    }
}

#[test]
fn retired_models_use_fallback_and_removed_tiers_are_omitted() {
    for model in ["gpt-5.4", "vendor/gpt-5.4-mini", "future-model"] {
        let mut body = json!({"model":model,"input":"synthetic"});
        crate::request_defaults::merge_request_defaults(
            body.as_object_mut().unwrap(),
            crate::request_defaults::model_profile(model),
            true,
        );
        assert_eq!(body["reasoning"], json!({"summary":"auto"}));
        assert!(body.get("text").is_none());
        assert_eq!(body["parallel_tool_calls"], true);
    }
    let mut body = json!({"service_tier":"ultrafast"});
    crate::request_defaults::merge_request_defaults(
        body.as_object_mut().unwrap(),
        crate::request_defaults::model_profile("gpt-5.6-sol"),
        true,
    );
    assert!(body.get("service_tier").is_none());
}

#[test]
fn ultra_and_priority_follow_strict_profile_selection() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in [
            "gpt-5.4",
            "future-model",
            "vendor/group/gpt-6-sol",
            "vendor!/gpt-6-astra",
        ] {
            let request = json!({"model":model,"input":"synthetic",
                "reasoning":{"effort":"ultra"},"service_tier":"priority"});
            let prepared = prepare_codex_overlay_for_test(
                UpstreamProfile::CodexSubscription1580,
                transport,
                &HeaderMap::new(),
                Bytes::from(request.to_string()),
                64 * 1024,
            )
            .unwrap();
            let body: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert_eq!(body["reasoning"]["effort"], "medium");
            assert!(body.get("service_tier").is_none());
        }
    }
}
