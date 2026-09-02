use http::HeaderMap;
use http::HeaderValue;
use mini_sub2api_protocol_v1::PROVIDER_REQUEST_ID_HEADER;

use crate::lifecycle_carriers::CarrierAction;
use crate::lifecycle_carriers::response_header_action;

const PROVIDER_REQUEST_ID_NAMES: &[&str] = &["x-request-id", "openai-request-id", "request-id"];
const MAX_PROVIDER_REQUEST_ID_BYTES: usize = 512;

pub(crate) fn filtered_provider_headers(
    source: &HeaderMap,
    gateway_request_id: &str,
) -> Result<HeaderMap, ()> {
    let gateway_alias = HeaderValue::from_str(gateway_request_id).map_err(|_| ())?;
    let mut filtered = HeaderMap::new();
    for (name, value) in source {
        match response_header_action(name.as_str()) {
            CarrierAction::Opaque => filtered.append(name.clone(), value.clone()),
            CarrierAction::GatewayRequestAlias => {
                filtered.append(name.clone(), gateway_alias.clone())
            }
            CarrierAction::PublicStrip => false,
            CarrierAction::RelationshipProjection | CarrierAction::ReversibleWireId => false,
        };
    }
    if let Some(raw) = provider_request_id(source) {
        filtered.insert(
            PROVIDER_REQUEST_ID_HEADER,
            HeaderValue::from_bytes(raw.as_bytes()).map_err(|_| ())?,
        );
    }
    Ok(filtered)
}

pub(crate) fn provider_request_id(source: &HeaderMap) -> Option<String> {
    for name in PROVIDER_REQUEST_ID_NAMES {
        for value in source.get_all(*name) {
            let bytes = value.as_bytes();
            if valid_provider_request_id(bytes) {
                return Some(String::from_utf8(bytes.to_vec()).expect("visible ASCII request ID"));
            }
        }
    }
    None
}

pub(crate) fn provider_request_id_control(raw: &str) -> Result<String, ()> {
    if !valid_provider_request_id(raw.as_bytes()) {
        return Err(());
    }
    let control = mini_sub2api_protocol_v1::ProviderRequestIdControl {
        event_type: mini_sub2api_protocol_v1::PROVIDER_REQUEST_ID_EVENT_TYPE.to_string(),
        provider_request_id: raw.to_string(),
    };
    let encoded = serde_json::to_string(&control).map_err(|_| ())?;
    if encoded.len() > MAX_PROVIDER_REQUEST_ID_BYTES * 2 + 128 {
        return Err(());
    }
    Ok(encoded)
}

fn valid_provider_request_id(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= MAX_PROVIDER_REQUEST_ID_BYTES
        && bytes.iter().all(|byte| matches!(byte, 0x21..=0x7e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny_aliases_public_request_ids_and_keeps_one_private_raw_id() {
        let mut source = HeaderMap::new();
        source.append("x-request-id", HeaderValue::from_static("provider-primary"));
        source.append("x-request-id", HeaderValue::from_static("provider-second"));
        source.insert(
            "openai-request-id",
            HeaderValue::from_static("provider-fallback"),
        );
        source.insert("server-timing", HeaderValue::from_static("provider;dur=7"));
        source.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static("must-not-cross"),
        );
        source.insert(
            "x-future-provider-id",
            HeaderValue::from_static("unknown-must-not-cross"),
        );

        let filtered = filtered_provider_headers(&source, "req_gateway").expect("headers");
        assert_eq!(filtered.get_all("x-request-id").iter().count(), 2);
        assert!(
            filtered
                .get_all("x-request-id")
                .iter()
                .all(|value| value == "req_gateway")
        );
        assert_eq!(filtered["openai-request-id"], "req_gateway");
        assert_eq!(filtered["server-timing"], "provider;dur=7");
        assert!(!filtered.contains_key("x-codex-turn-metadata"));
        assert!(!filtered.contains_key("x-future-provider-id"));
        assert_eq!(filtered[PROVIDER_REQUEST_ID_HEADER], "provider-primary");
    }

    #[test]
    fn diagnostic_rejects_spaces_controls_empty_and_oversized_values() {
        for raw in ["", "contains space", "contains\tcontrol"] {
            assert!(!valid_provider_request_id(raw.as_bytes()), "{raw:?}");
            assert!(provider_request_id_control(raw).is_err(), "{raw:?}");
        }
        let oversized = "x".repeat(MAX_PROVIDER_REQUEST_ID_BYTES + 1);
        assert!(!valid_provider_request_id(oversized.as_bytes()));
        assert!(provider_request_id_control(&oversized).is_err());
        let escaped_maximum = "\\".repeat(MAX_PROVIDER_REQUEST_ID_BYTES);
        assert!(provider_request_id_control(&escaped_maximum).is_ok());
    }
}
