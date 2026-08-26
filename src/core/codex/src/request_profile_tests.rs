use super::*;
use http::HeaderValue;

#[test]
fn source_and_credential_select_the_four_profiles_exhaustively() {
    let cases = [
        (
            CallerKind::Bare,
            CredentialKind::OpenAiApiKey,
            UpstreamProfile::BareOpenAi,
        ),
        (
            CallerKind::Codex,
            CredentialKind::OpenAiApiKey,
            UpstreamProfile::CodexOpenAi149,
        ),
        (
            CallerKind::Bare,
            CredentialKind::CodexSubscription,
            UpstreamProfile::CodexSubscription149,
        ),
        (
            CallerKind::Codex,
            CredentialKind::CodexSubscription,
            UpstreamProfile::CodexSubscription149,
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

    assert_eq!(profile, UpstreamProfile::CodexOpenAi149);
    assert_eq!(profile.credential_kind(), CredentialKind::OpenAiApiKey);
    assert!(profile.emulates_codex());
}
