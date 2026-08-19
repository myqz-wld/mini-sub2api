use anyhow::Context;
use anyhow::Result;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;

const MAX_AUTH_JSON_BYTES: usize = 1024 * 1024;
const MAX_AUTH_ERROR_BYTES: usize = 64 * 1024;

pub async fn decode_auth_json<T: DeserializeOwned>(
    response: reqwest::Response,
    description: &'static str,
) -> Result<T> {
    let bytes = read_limited(response, MAX_AUTH_JSON_BYTES)
        .await
        .with_context(|| format!("reading {description}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decoding {description}"))
}

pub async fn read_auth_error(response: reqwest::Response) -> Result<String> {
    let bytes = read_limited(response, MAX_AUTH_ERROR_BYTES)
        .await
        .context("reading OAuth error response")?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_limited(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading HTTP response chunk")?;
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= maximum,
            "HTTP response exceeds the configured safety limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
