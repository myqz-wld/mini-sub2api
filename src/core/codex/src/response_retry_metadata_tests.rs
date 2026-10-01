use super::*;

fn upstream_error(headers: Value) -> Value {
    json!({"code":"server_is_overloaded","type":"service_unavailable_error",
        "message":"Our servers are currently overloaded. Please try again later.",
        "headers":headers})
}

#[test]
fn retry_metadata_survives_each_public_error_container() {
    for name in ["x-retry-metadata", "X-Retry-Metadata"] {
        for metadata in [json!("NO_MORE_RETRY"), json!(["NO_MORE_RETRY"])] {
            let headers = json!({name:metadata,"Authorization":"synthetic-private",
                "Set-Cookie":"synthetic-private","x-retry-metadata-private":"synthetic-private"});
            let error = upstream_error(headers.clone());
            if metadata.is_string() {
                let bytes = serde_json::to_vec(&json!({"error":error})).unwrap();
                assert_eq!(
                    crate::response_failure::retry_metadata(&bytes).unwrap(),
                    "NO_MORE_RETRY"
                );
            }
            for mut value in [
                json!({"type":"error","status":503,"error":error}),
                json!({"type":"error","code":"server_is_overloaded","headers":headers}),
                json!({"type":"response.failed","response":{"error":error}}),
                json!({"error":error}),
            ] {
                filter_response(&mut value, "req_gateway");
                let public_error = value
                    .pointer("/response/error")
                    .or_else(|| value.get("error"))
                    .unwrap_or(&value);
                assert_eq!(public_error["code"], "server_is_overloaded");
                assert_eq!(public_error["headers"], json!({name:metadata}));
                assert_eq!(public_error["message"], FAILURE_MESSAGE);
                assert!(!value.to_string().contains("synthetic-private"));
            }
        }
    }
}

#[test]
fn malformed_retry_metadata_cannot_open_the_error_header_boundary() {
    let mut too_deep = json!("NO_MORE_RETRY");
    for _ in 0..9 {
        too_deep = json!([too_deep]);
    }
    for metadata in [
        Value::Null,
        json!(true),
        json!(1),
        json!({"value":"NO_MORE_RETRY"}),
        json!(["NO_MORE_RETRY", false]),
        json!("NO_MORE_RETRY\r\nAuthorization: synthetic-private"),
        json!("x".repeat(crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES + 1)),
        too_deep,
    ] {
        let error = upstream_error(json!({"x-retry-metadata":metadata}));
        let bytes = serde_json::to_vec(&json!({"error":error})).unwrap();
        assert!(crate::response_failure::retry_metadata(&bytes).is_none());
        let mut value = json!({"type":"error","error":error});
        filter_response(&mut value, "req_gateway");
        assert_eq!(
            value["error"],
            json!({"code":"server_is_overloaded","message":FAILURE_MESSAGE})
        );
    }
    for headers in [Value::Null, json!("NO_MORE_RETRY"), json!([])] {
        let mut value = json!({"error":upstream_error(headers)});
        filter_response(&mut value, "req_gateway");
        assert!(value["error"].get("headers").is_none());
    }
}
