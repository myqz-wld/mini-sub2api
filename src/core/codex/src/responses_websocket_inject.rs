use crate::request_profile::UpstreamProfile;
use crate::request_pseudonym::RequestPseudonymizer;
use crate::responses_lite;
use serde_json::Value;

const SUPPORTED_INJECT_FIELDS: &[&str] = &["type", "input", "response_id"];

pub(crate) fn prepare(
    original: String,
    mut value: Value,
    account_namespace: Option<&str>,
    profile: UpstreamProfile,
    pseudonym_scope: &str,
    maximum: usize,
) -> Result<String, ()> {
    if profile == UpstreamProfile::BareOpenAi {
        return Ok(original);
    }
    let object = value.as_object_mut().ok_or(())?;
    object.retain(|name, _| SUPPORTED_INJECT_FIELDS.contains(&name.as_str()));
    match (profile, account_namespace) {
        (UpstreamProfile::CodexOpenAi149, None) => {}
        (UpstreamProfile::CodexSubscription149, Some(account_namespace)) => {
            RequestPseudonymizer::new(account_namespace, pseudonym_scope)
                .apply_body_only(object)?;
        }
        _ => return Err(()),
    }
    if let Some(input) = object.get_mut("input") {
        responses_lite::canonicalize_injected_input(input);
    }
    let encoded = serde_json::to_string(&value).map_err(|_| ())?;
    if encoded.len() > maximum {
        Err(())
    } else {
        Ok(encoded)
    }
}
