use super::*;
use bytes::Bytes;
use http::HeaderValue;

const INTERNAL_TOKEN: &str = "internal-test-token-with-at-least-32-bytes";

pub(super) async fn api_key_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    api_key_state_with_mode(base_url, crate::fingerprint::FingerprintMode::Device).await
}

pub(super) async fn api_key_state_with_mode(
    base_url: &str,
    mode: crate::fingerprint::FingerprintMode,
) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-api-key-test".to_string(),
            format!("{base_url}/responses"),
            mode,
        )
        .await
        .expect("API key record");
    (app_state(vault), metadata.account_ref, temp)
}

pub(super) fn app_state(vault: Vault) -> AppState {
    AppState {
        vault,
        transports: Arc::new(TransportRegistry::new().expect("transport registry")),
        internal_token_hash: Sha256::digest(INTERNAL_TOKEN.as_bytes()).into(),
        account_locks: Arc::new(Mutex::new(HashMap::new())),
    }
}

pub(super) async fn call_core(
    state: &AppState,
    account_ref: &str,
    body: Bytes,
) -> std::result::Result<Response<Body>, CoreFailure> {
    call_core_with_headers(state, account_ref, body, HeaderMap::new()).await
}

pub(super) async fn call_core_with_headers(
    state: &AppState,
    account_ref: &str,
    body: Bytes,
    extra_headers: HeaderMap,
) -> std::result::Result<Response<Body>, CoreFailure> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer internal-test-token-with-at-least-32-bytes"),
    );
    headers.insert(VERSION_HEADER, HeaderValue::from_static(VERSION));
    headers.insert(
        ACCOUNT_REF_HEADER,
        HeaderValue::from_str(account_ref).expect("account ref"),
    );
    headers.insert(
        PSEUDONYM_SCOPE_HEADER,
        HeaderValue::from_static("psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    );
    headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("req_test"));
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert("x-codex-turn-state", HeaderValue::from_static("turn-test"));
    headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
    headers.extend(extra_headers);
    let request = Request::builder().body(Body::from(body)).expect("request");
    responses_inner(
        "127.0.0.1:43210".parse().expect("peer"),
        state,
        headers,
        request,
    )
    .await
}
