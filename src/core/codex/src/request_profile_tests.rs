use super::*;
use http::HeaderValue;

#[test]
fn credential_alone_selects_emulation_for_all_callers() {
    let cases = [
        (
            CallerKind::Bare,
            CredentialKind::OpenAiApiKey,
            UpstreamProfile::ApiKeyPassthrough,
        ),
        (
            CallerKind::Codex,
            CredentialKind::OpenAiApiKey,
            UpstreamProfile::ApiKeyPassthrough,
        ),
        (
            CallerKind::Bare,
            CredentialKind::CodexSubscription,
            UpstreamProfile::CodexSubscription1534,
        ),
        (
            CallerKind::Codex,
            CredentialKind::CodexSubscription,
            UpstreamProfile::CodexSubscription1534,
        ),
    ];

    for (caller, credential, expected) in cases {
        let actual = UpstreamProfile::select(caller, credential);
        assert_eq!(actual, expected);
        assert_eq!(actual.credential_kind(), credential);
    }
}

#[test]
fn a_valid_non_empty_originator_marks_codex_without_requiring_a_fixed_value() {
    for value in ["codex_cli_rs", "codex_exec", "custom-app-server"] {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_str(value).expect("header"));
        assert_eq!(CallerKind::from_headers(&headers), CallerKind::Codex);
    }
}

#[test]
fn absent_empty_whitespace_or_non_text_originator_is_bare() {
    let mut cases = vec![HeaderMap::new()];
    for value in ["", " ", "\t  "] {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_str(value).expect("header"));
        cases.push(headers);
    }
    let mut non_text = HeaderMap::new();
    non_text.insert(
        "originator",
        HeaderValue::from_bytes(&[0x80]).expect("valid opaque header bytes"),
    );
    cases.push(non_text);

    for headers in cases {
        assert_eq!(CallerKind::from_headers(&headers), CallerKind::Bare);
    }
}

#[test]
fn any_valid_non_empty_originator_value_marks_codex() {
    let mut headers = HeaderMap::new();
    headers.append("originator", HeaderValue::from_static(""));
    headers.append("originator", HeaderValue::from_static("codex_cli_rs"));

    assert_eq!(CallerKind::from_headers(&headers), CallerKind::Codex);
}

#[test]
fn originator_never_changes_the_credential_dimension() {
    let profile = UpstreamProfile::select(CallerKind::Codex, CredentialKind::OpenAiApiKey);

    assert_eq!(profile, UpstreamProfile::ApiKeyPassthrough);
    assert_eq!(profile.credential_kind(), CredentialKind::OpenAiApiKey);
    assert!(!profile.emulates_codex());
}

#[test]
fn identity_and_transport_capabilities_are_independent() {
    let openai = UpstreamProfile::ApiKeyPassthrough;
    let subscription = UpstreamProfile::CodexSubscription1534;
    let bare = UpstreamProfile::ApiKeyPassthrough;

    assert!(!openai.uses_identity_state());
    assert!(subscription.uses_identity_state());
    assert!(!bare.uses_identity_state());

    assert!(!openai.uses_subscription_transport());
    assert!(subscription.uses_subscription_transport());
    assert!(!bare.uses_subscription_transport());

    assert!(!openai.uses_oauth_refresh());
    assert!(subscription.uses_oauth_refresh());
    assert!(!openai.uses_http_zstd());
    assert!(subscription.uses_http_zstd());
}
