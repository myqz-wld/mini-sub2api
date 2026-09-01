use crate::request_profile::UpstreamProfile;
use crate::responses_lite;
use serde_json::Value;

const SUPPORTED_INJECT_FIELDS: &[&str] = &["type", "input", "response_id"];

pub(crate) fn prepare(
    original: String,
    mut value: Value,
    profile: UpstreamProfile,
    maximum: usize,
) -> Result<String, ()> {
    if profile == UpstreamProfile::BareOpenAi {
        return Ok(original);
    }
    prepare_object(&mut value, profile)?;
    encode(value, maximum)
}

pub(crate) fn prepare_without_identity(
    original: String,
    mut value: Value,
    profile: UpstreamProfile,
    maximum: usize,
) -> Result<String, ()> {
    if profile == UpstreamProfile::BareOpenAi {
        return Ok(original);
    }
    prepare_object(&mut value, profile)?;
    encode(value, maximum)
}

fn prepare_object(value: &mut Value, profile: UpstreamProfile) -> Result<(), ()> {
    if !profile.emulates_codex() {
        return Err(());
    }
    let object = value.as_object_mut().ok_or(())?;
    object.retain(|name, _| SUPPORTED_INJECT_FIELDS.contains(&name.as_str()));
    if let Some(input) = object.get_mut("input") {
        responses_lite::canonicalize_injected_input(input);
    }
    Ok(())
}

fn encode(value: Value, maximum: usize) -> Result<String, ()> {
    let encoded = serde_json::to_string(&value).map_err(|_| ())?;
    if encoded.len() > maximum {
        Err(())
    } else {
        Ok(encoded)
    }
}
