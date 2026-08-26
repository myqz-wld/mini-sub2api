use crate::error::CoreFailure;
use crate::server::AppState;
use crate::server::header_text;
use axum::http::HeaderMap;
use base64::Engine;
use mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER;
use mini_sub2api_protocol_v1::PSEUDONYM_SCOPE_HEADER;
use mini_sub2api_protocol_v1::REQUEST_ID_HEADER;
use mini_sub2api_protocol_v1::VERSION;
use mini_sub2api_protocol_v1::VERSION_HEADER;
use sha2::Digest;
use sha2::Sha256;
use std::net::SocketAddr;
use subtle::ConstantTimeEq;

pub(crate) struct InternalRequestIdentity {
    pub(crate) account_ref: String,
    pub(crate) pseudonym_scope: String,
}

pub(crate) fn validate_internal_request(
    peer: SocketAddr,
    state: &AppState,
    headers: &HeaderMap,
) -> Result<InternalRequestIdentity, CoreFailure> {
    if !peer.ip().is_loopback() {
        return Err(CoreFailure::InvalidInternalAuth);
    }
    if header_text(headers, VERSION_HEADER).as_deref() != Some(VERSION) {
        return Err(CoreFailure::UnsupportedProtocol);
    }
    validate_internal_auth(headers, &state.internal_token_hash)?;
    let account_ref = header_text(headers, ACCOUNT_REF_HEADER)
        .filter(|value| value.starts_with("acct_") && value.len() <= 133)
        .ok_or(CoreFailure::InvalidRequest)?;
    let pseudonym_scope = header_text(headers, PSEUDONYM_SCOPE_HEADER)
        .filter(|value| valid_pseudonym_scope(value))
        .ok_or(CoreFailure::InvalidRequest)?;
    header_text(headers, REQUEST_ID_HEADER)
        .filter(|value| value.starts_with("req_") && value.len() <= 132)
        .ok_or(CoreFailure::InvalidRequest)?;
    Ok(InternalRequestIdentity {
        account_ref,
        pseudonym_scope,
    })
}

fn valid_pseudonym_scope(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("psn_") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return false;
    };
    decoded.len() == 32
        && base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded) == encoded
}

pub(crate) fn validate_internal_auth(
    headers: &HeaderMap,
    expected_hash: &[u8; 32],
) -> Result<(), CoreFailure> {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(CoreFailure::InvalidInternalAuth)?;
    let actual: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if actual.ct_eq(expected_hash).into() {
        Ok(())
    } else {
        Err(CoreFailure::InvalidInternalAuth)
    }
}

#[cfg(test)]
mod tests {
    use super::valid_pseudonym_scope;

    #[test]
    fn pseudonym_scope_requires_one_canonical_sha256_digest() {
        assert!(valid_pseudonym_scope(
            "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        ));
        assert!(!valid_pseudonym_scope("psn_short"));
        assert!(!valid_pseudonym_scope(
            "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!"
        ));
        assert!(!valid_pseudonym_scope(
            "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB"
        ));
    }
}
