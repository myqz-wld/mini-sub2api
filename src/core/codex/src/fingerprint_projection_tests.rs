use super::*;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use pretty_assertions::assert_eq;

const DEVICE_ID: &str = "11111111-1111-4111-8111-111111111111";

fn device() -> FingerprintSnapshot {
    FingerprintSnapshot::for_test(FingerprintMode::Device, 7)
}

#[test]
fn converges_all_recognized_carriers_and_preserves_other_identity() {
    let mut headers = HeaderMap::new();
    headers.insert(
        INSTALLATION_HEADER,
        "header-device".parse().expect("header"),
    );
    headers.insert(
        TURN_METADATA_HEADER,
        r#"{"installation_id":"header-turn-device","session_id":"session-header","thread_id":"thread-header","turn_id":"turn-header","window_id":"window-header","future":{"kept":true}}"#
            .parse()
            .expect("turn metadata"),
    );
    headers.insert("x-unrelated", "preserved".parse().expect("unrelated"));
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "test",
            "session_id": "top-session",
            "client_metadata": {
                "x-codex-installation-id": "body-device",
                "x-codex-turn-metadata": serde_json::json!({
                    "installation_id": "body-turn-device",
                    "session_id": "session-body",
                    "thread_id": "thread-body",
                    "turn_id": "turn-body",
                    "window_id": "window-body",
                    "future": ["kept"]
                }).to_string(),
                "custom": "preserved"
            }
        }))
        .expect("body"),
    );

    let projected = project_http_device(headers, body, &device(), DEVICE_ID, 1024 * 1024)
        .expect("projected request");
    assert_eq!(
        projected
            .headers
            .get(INSTALLATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(DEVICE_ID)
    );
    assert_eq!(
        projected
            .headers
            .get("x-unrelated")
            .and_then(|value| value.to_str().ok()),
        Some("preserved")
    );
    let header_turn: Value = serde_json::from_str(
        projected
            .headers
            .get(TURN_METADATA_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("header turn metadata"),
    )
    .expect("header turn JSON");
    assert_eq!(header_turn["installation_id"], DEVICE_ID);
    assert_eq!(header_turn["session_id"], "session-header");
    assert_eq!(header_turn["thread_id"], "thread-header");
    assert_eq!(header_turn["turn_id"], "turn-header");
    assert_eq!(header_turn["window_id"], "window-header");
    assert_eq!(header_turn["future"]["kept"], true);

    let body: Value = serde_json::from_slice(&projected.body).expect("body JSON");
    assert_eq!(body["session_id"], "top-session");
    assert_eq!(body["client_metadata"][INSTALLATION_HEADER], DEVICE_ID);
    assert_eq!(body["client_metadata"]["custom"], "preserved");
    let body_turn: Value = serde_json::from_str(
        body["client_metadata"][TURN_METADATA_HEADER]
            .as_str()
            .expect("body turn metadata"),
    )
    .expect("body turn JSON");
    assert_eq!(body_turn["installation_id"], DEVICE_ID);
    assert_eq!(body_turn["session_id"], "session-body");
    assert_eq!(body_turn["thread_id"], "thread-body");
    assert_eq!(body_turn["turn_id"], "turn-body");
    assert_eq!(body_turn["window_id"], "window-body");
    assert_eq!(body_turn["future"][0], "kept");
}

#[test]
fn valid_body_without_recognized_carrier_remains_byte_exact() {
    let original =
        Bytes::from_static(br#" {"model":"test", "client_metadata":{"custom":"kept"}} "#);
    let projected = project_http_device(
        HeaderMap::new(),
        original.clone(),
        &device(),
        DEVICE_ID,
        1024,
    )
    .expect("projected request");
    assert_eq!(projected.body, original);
    assert_eq!(
        projected
            .headers
            .get(INSTALLATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(DEVICE_ID)
    );
}

#[test]
fn preserves_turn_metadata_that_intentionally_omits_installation() {
    let mut headers = HeaderMap::new();
    headers.insert(
        TURN_METADATA_HEADER,
        r#"{"request_kind":"turn","future":1}"#
            .parse()
            .expect("turn metadata"),
    );
    let body = Bytes::from_static(
        br#"{"client_metadata":{"x-codex-turn-metadata":"{\"request_kind\":\"turn\",\"future\":2}"}}"#,
    );
    let projected =
        project_http_device(headers, body, &device(), DEVICE_ID, 1024).expect("projected request");
    let header: Value = serde_json::from_str(
        projected
            .headers
            .get(TURN_METADATA_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("header turn"),
    )
    .expect("header JSON");
    assert!(header.get("installation_id").is_none());
    assert_eq!(header["future"], 1);
    let body: Value = serde_json::from_slice(&projected.body).expect("body JSON");
    let body_turn: Value = serde_json::from_str(
        body["client_metadata"][TURN_METADATA_HEADER]
            .as_str()
            .expect("body turn"),
    )
    .expect("body turn JSON");
    assert!(body_turn.get("installation_id").is_none());
    assert_eq!(body_turn["future"], 2);
}

#[test]
fn rejects_unsafe_device_payloads_before_projection() {
    let cases = [
        (HeaderMap::new(), Bytes::from_static(b"not-json")),
        (HeaderMap::new(), Bytes::from_static(br#"[]"#)),
        (
            HeaderMap::new(),
            Bytes::from_static(br#"{"client_metadata":"invalid"}"#),
        ),
        (
            HeaderMap::new(),
            Bytes::from_static(br#"{"client_metadata":{"x-codex-turn-metadata":"not-json"}}"#),
        ),
    ];
    for (headers, body) in cases {
        assert!(project_http_device(headers, body, &device(), DEVICE_ID, 1024).is_err());
    }

    let mut malformed_header = HeaderMap::new();
    malformed_header.insert(TURN_METADATA_HEADER, "not-json".parse().expect("header"));
    assert!(
        project_http_device(
            malformed_header,
            Bytes::from_static(br#"{}"#),
            &device(),
            DEVICE_ID,
            1024,
        )
        .is_err()
    );

    let mut encoded = HeaderMap::new();
    encoded.insert(
        http::header::CONTENT_ENCODING,
        "zstd".parse().expect("encoding"),
    );
    assert!(
        project_http_device(
            encoded,
            Bytes::from_static(br#"{}"#),
            &device(),
            DEVICE_ID,
            1024,
        )
        .is_err()
    );
}

#[test]
fn rejects_rewrite_that_exceeds_limit() {
    let body = Bytes::from_static(br#"{"client_metadata":{"x-codex-installation-id":"x"}}"#);
    assert!(
        project_http_device(
            HeaderMap::new(),
            body.clone(),
            &device(),
            DEVICE_ID,
            body.len(),
        )
        .is_err()
    );
}

#[test]
fn websocket_projection_preserves_no_op_bytes_and_rewrites_carriers() {
    let no_op = " {\"type\":\"response.create\", \"custom\":true} ".to_string();
    assert_eq!(
        project_websocket_device(no_op.clone(), &device(), DEVICE_ID, 1024).expect("no-op frame"),
        no_op
    );

    let projected = project_websocket_device(
        serde_json::json!({
            "type": "response.create",
            "client_metadata": {
                "x-codex-installation-id": "conflict",
                "x-codex-turn-metadata": "{\"installation_id\":\"nested-conflict\",\"turn_id\":\"kept\"}"
            }
        })
        .to_string(),
        &device(),
        DEVICE_ID,
        1024,
    )
    .expect("projected frame");
    let projected: Value = serde_json::from_str(&projected).expect("frame JSON");
    assert_eq!(projected["client_metadata"][INSTALLATION_HEADER], DEVICE_ID);
    let turn: Value = serde_json::from_str(
        projected["client_metadata"][TURN_METADATA_HEADER]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn JSON");
    assert_eq!(turn["installation_id"], DEVICE_ID);
    assert_eq!(turn["turn_id"], "kept");
}

#[test]
fn projection_preserves_codex_field_order_and_ascii_turn_metadata() {
    let body = Bytes::from_static(
        r#"{"model":"gpt-5.6-sol","instructions":"follow","input":[{"type":"message","id":"msg_keep","role":"user","content":[{"type":"input_text","text":"hello"}]}],"client_metadata":{"custom":"kept","x-codex-installation-id":"conflict","x-codex-turn-metadata":"{\"installation_id\":\"conflict\",\"workspaces\":{\"/tmp/東京\":{}}}"}}"#.as_bytes(),
    );
    let projected = project_http_device(HeaderMap::new(), body, &device(), DEVICE_ID, 4096)
        .expect("projected request");
    let text = std::str::from_utf8(&projected.body).expect("UTF-8 JSON");
    let positions = [
        text.find("\"model\"").expect("model"),
        text.find("\"instructions\"").expect("instructions"),
        text.find("\"input\"").expect("input"),
        text.find("\"client_metadata\"").expect("client metadata"),
    ];
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    let item_positions = [
        text.find("\"type\":\"message\"").expect("type"),
        text.find("\"id\":\"msg_keep\"").expect("id"),
        text.find("\"role\":\"user\"").expect("role"),
        text.find("\"content\"").expect("content"),
    ];
    assert!(item_positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(text.contains(r#"\\u6771\\u4eac"#));
    assert!(!text.contains("東京"));
    let value: Value = serde_json::from_slice(&projected.body).expect("request JSON");
    let metadata: Value = serde_json::from_str(
        value["client_metadata"][TURN_METADATA_HEADER]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    assert_eq!(metadata["workspaces"]["/tmp/東京"], serde_json::json!({}));
}
