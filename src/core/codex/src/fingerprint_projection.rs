use crate::ascii_json::to_ascii_json_string;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use anyhow::Context;
use anyhow::Result;
use bytes::Bytes;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Value;

const INSTALLATION_HEADER: &str = "x-codex-installation-id";
const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";

pub(crate) struct ProjectedRequest {
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

pub(crate) fn project_http_device(
    mut headers: HeaderMap,
    body: Bytes,
    fingerprint: &FingerprintSnapshot,
    maximum: usize,
) -> Result<ProjectedRequest> {
    anyhow::ensure!(
        fingerprint.mode() == FingerprintMode::Device,
        "device projection requires device mode"
    );
    ensure_identity_encoding(&headers)?;
    anyhow::ensure!(body.len() <= maximum, "request body is too large");
    project_device_headers(&mut headers, fingerprint)?;
    let body = project_json_body(body, fingerprint.installation_id(), maximum)?;
    Ok(ProjectedRequest { headers, body })
}

pub(crate) fn project_websocket_device(
    text: String,
    fingerprint: &FingerprintSnapshot,
    maximum: usize,
) -> Result<String> {
    anyhow::ensure!(
        fingerprint.mode() == FingerprintMode::Device,
        "device projection requires device mode"
    );
    anyhow::ensure!(text.len() <= maximum, "WebSocket request is too large");
    let body = project_json_body(
        Bytes::from(text.into_bytes()),
        fingerprint.installation_id(),
        maximum,
    )?;
    String::from_utf8(body.to_vec()).context("projected WebSocket request is not UTF-8")
}

pub(crate) fn project_device_headers(
    headers: &mut HeaderMap,
    fingerprint: &FingerprintSnapshot,
) -> Result<()> {
    let turn_metadata = headers
        .get_all(TURN_METADATA_HEADER)
        .iter()
        .map(|value| {
            let raw = value
                .to_str()
                .context("invalid credential turn metadata header")?;
            rewrite_serialized_turn_metadata(raw, fingerprint.installation_id())
        })
        .collect::<Result<Vec<_>>>()?;
    if let Some(first) = turn_metadata.first() {
        headers.insert(
            HeaderName::from_static(TURN_METADATA_HEADER),
            HeaderValue::from_str(first)
                .context("projected turn metadata is not a valid header")?,
        );
    }
    headers.insert(
        HeaderName::from_static(INSTALLATION_HEADER),
        HeaderValue::from_str(fingerprint.installation_id())
            .context("credential installation id is not a valid header")?,
    );
    Ok(())
}

fn project_json_body(body: Bytes, installation_id: &str, maximum: usize) -> Result<Bytes> {
    let mut value: Value =
        serde_json::from_slice(&body).context("device request body is not valid JSON")?;
    let object = value
        .as_object_mut()
        .context("device request body is not a JSON object")?;
    let Some(metadata_value) = object.get_mut("client_metadata") else {
        return Ok(body);
    };
    let metadata = metadata_value
        .as_object_mut()
        .context("device client_metadata is not a JSON object")?;
    let mut changed = false;
    if let Some(value) = metadata.get_mut(INSTALLATION_HEADER)
        && value.as_str() != Some(installation_id)
    {
        *value = Value::String(installation_id.to_string());
        changed = true;
    }
    if let Some(value) = metadata.get_mut(TURN_METADATA_HEADER) {
        let raw = value
            .as_str()
            .context("device client turn metadata is not a string")?;
        let rewritten = rewrite_serialized_turn_metadata(raw, installation_id)?;
        if rewritten != raw {
            *value = Value::String(rewritten);
            changed = true;
        }
    }
    if !changed {
        return Ok(body);
    }
    let encoded = serde_json::to_vec(&value).context("encoding projected device request")?;
    anyhow::ensure!(
        encoded.len() <= maximum,
        "projected device request is too large"
    );
    Ok(Bytes::from(encoded))
}

fn rewrite_serialized_turn_metadata(raw: &str, installation_id: &str) -> Result<String> {
    let mut value: Value =
        serde_json::from_str(raw).context("device turn metadata is not valid JSON")?;
    let object = value
        .as_object_mut()
        .context("device turn metadata is not a JSON object")?;
    let Some(current) = object.get_mut("installation_id") else {
        return Ok(raw.to_string());
    };
    *current = Value::String(installation_id.to_string());
    to_ascii_json_string(&value).context("encoding projected device turn metadata")
}

fn ensure_identity_encoding(headers: &HeaderMap) -> Result<()> {
    for value in headers.get_all(http::header::CONTENT_ENCODING) {
        let value = value.to_str().context("invalid request content encoding")?;
        for encoding in value.split(',').map(str::trim) {
            anyhow::ensure!(
                !encoding.is_empty() && encoding.eq_ignore_ascii_case("identity"),
                "unsupported request content encoding"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "fingerprint_projection_tests.rs"]
mod tests;
