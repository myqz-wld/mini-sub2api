use anyhow::Context;
use anyhow::Result;
use bytes::Bytes;
use futures_util::StreamExt;
use http::HeaderMap;
use serde::de::DeserializeOwned;
use std::io::Cursor;
use std::io::Read;

const MAX_AUTH_JSON_BYTES: usize = 1024 * 1024;
const MAX_AUTH_ERROR_BYTES: usize = 64 * 1024;

pub(crate) fn decode_emulated_request_body(
    headers: &mut HeaderMap,
    body: Bytes,
    maximum: usize,
) -> Result<Bytes, ()> {
    let encodings = headers
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, ()>>()?;
    let encoding = match encodings.as_slice() {
        [] => return Ok(body),
        [encoding] if encoding.eq_ignore_ascii_case("identity") => None,
        [encoding] if encoding.eq_ignore_ascii_case("zstd") => Some(encoding),
        _ => return Err(()),
    };
    headers.remove(http::header::CONTENT_ENCODING);
    let Some(_) = encoding else {
        return Ok(body);
    };

    let decoder = zstd::stream::read::Decoder::new(Cursor::new(body)).map_err(|_| ())?;
    read_limited_reader(decoder, maximum).map(Bytes::from)
}

fn read_limited_reader(mut reader: impl Read, maximum: usize) -> std::result::Result<Vec<u8>, ()> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).map_err(|_| ())?;
        if count == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(count) > maximum {
            return Err(());
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

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

#[cfg(test)]
#[path = "http_body_tests.rs"]
mod tests;
