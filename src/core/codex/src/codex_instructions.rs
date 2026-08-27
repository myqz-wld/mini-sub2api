use serde_json::Map;
use serde_json::Value;

const GPT_5_6_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/gpt-5.6.md");
const GPT_5_5_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/gpt-5.5.md");
const GPT_5_4_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/gpt-5.4.md");
const GPT_5_4_MINI_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/gpt-5.4-mini.md");
const GPT_5_2_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/gpt-5.2.md");
const FALLBACK_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.149.0/fallback.md");
const EXP_CODEX_PERSONALITY_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.149.0/exp-codex-personality.md");

const MODEL_INSTRUCTIONS: &[(&str, &str)] = &[
    ("gpt-5.6-sol", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-terra", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-luna", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.5", GPT_5_5_INSTRUCTIONS),
    ("gpt-5.4-mini", GPT_5_4_MINI_INSTRUCTIONS),
    ("gpt-5.4", GPT_5_4_INSTRUCTIONS),
    ("gpt-5.2", GPT_5_2_INSTRUCTIONS),
    ("codex-auto-review", GPT_5_4_INSTRUCTIONS),
];

pub(crate) fn for_model(model: &str) -> &'static str {
    if model == "exp-codex-personality" {
        return EXP_CODEX_PERSONALITY_INSTRUCTIONS;
    }
    find_by_longest_prefix(model)
        .or_else(|| find_by_namespaced_suffix(model))
        .unwrap_or(FALLBACK_INSTRUCTIONS)
}

/// Replaces the caller-owned base prompt with the Codex 0.149.0 model prompt and preserves a
/// non-Codex caller prompt as a developer message. Responses Lite carries the base prompt in its
/// input prefix; normal Responses uses the top-level `instructions` field.
pub(crate) fn apply(
    object: &mut Map<String, Value>,
    responses_lite: bool,
    lite_incremental: bool,
) -> Result<(), ()> {
    let model = object.get("model").and_then(Value::as_str).unwrap_or("");
    let base = for_model(model);
    let custom = take_custom_instructions(object);

    if responses_lite {
        if lite_incremental {
            return Ok(());
        }
        replace_input_base(object, base, custom)
    } else {
        object.insert("instructions".to_string(), Value::String(base.to_string()));
        replace_input_base_messages(object, custom)
    }
}

fn find_by_longest_prefix(model: &str) -> Option<&'static str> {
    MODEL_INSTRUCTIONS
        .iter()
        .filter(|(slug, _)| model.starts_with(slug))
        .max_by_key(|(slug, _)| slug.len())
        .map(|(_, instructions)| *instructions)
}

fn find_by_namespaced_suffix(model: &str) -> Option<&'static str> {
    let (namespace, suffix) = model.split_once('/')?;
    if suffix.contains('/')
        || namespace.is_empty()
        || !namespace
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    find_by_longest_prefix(suffix)
}

fn take_custom_instructions(object: &mut Map<String, Value>) -> Option<String> {
    match object.remove("instructions") {
        Some(Value::String(instructions)) if !instructions.is_empty() => {
            let custom = known_prefix(&instructions)
                .map(|base| instructions[base.len()..].to_string())
                .unwrap_or(instructions);
            (!custom.is_empty()).then_some(custom)
        }
        _ => None,
    }
}

fn replace_input_base(
    object: &mut Map<String, Value>,
    base: &str,
    custom: Option<String>,
) -> Result<(), ()> {
    let input = input_items(object)?;
    strip_known_base_prefixes(input);
    let insertion = usize::from(
        input
            .first()
            .and_then(Value::as_object)
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            == Some("additional_tools"),
    );
    input.insert(insertion, developer_message(base.to_string()));
    if let Some(custom) = custom {
        input.insert(insertion + 1, developer_message(custom));
    }
    Ok(())
}

fn replace_input_base_messages(
    object: &mut Map<String, Value>,
    custom: Option<String>,
) -> Result<(), ()> {
    let Some(input) = object.get_mut("input") else {
        if let Some(custom) = custom {
            object.insert(
                "input".to_string(),
                Value::Array(vec![developer_message(custom)]),
            );
        }
        return Ok(());
    };
    let input = input.as_array_mut().ok_or(())?;
    strip_known_base_prefixes(input);
    if let Some(custom) = custom {
        input.insert(0, developer_message(custom));
    }
    Ok(())
}

fn input_items(object: &mut Map<String, Value>) -> Result<&mut Vec<Value>, ()> {
    object
        .entry("input".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(())
}

fn strip_known_base_prefixes(input: &mut Vec<Value>) {
    input.retain_mut(|item| {
        let Some(text) = developer_message_text_mut(item) else {
            return true;
        };
        let Some(base) = known_prefix(text) else {
            return true;
        };
        let custom = text[base.len()..].to_string();
        if custom.is_empty() {
            return false;
        }
        *text = custom;
        true
    });
}

fn developer_message_text_mut(item: &mut Value) -> Option<&mut String> {
    let item = item.as_object_mut()?;
    if item.get("role").and_then(Value::as_str) != Some("developer") {
        return None;
    }
    let content = item.get_mut("content")?.as_array_mut()?;
    let [content] = content.as_mut_slice() else {
        return None;
    };
    let content = content.as_object_mut()?;
    if content.get("type").and_then(Value::as_str) != Some("input_text") {
        return None;
    }
    match content.get_mut("text")? {
        Value::String(text) => Some(text),
        _ => None,
    }
}

fn developer_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "developer",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn known_prefix(instructions: &str) -> Option<&'static str> {
    [
        GPT_5_6_INSTRUCTIONS,
        GPT_5_5_INSTRUCTIONS,
        GPT_5_4_INSTRUCTIONS,
        GPT_5_4_MINI_INSTRUCTIONS,
        GPT_5_2_INSTRUCTIONS,
        EXP_CODEX_PERSONALITY_INSTRUCTIONS,
        FALLBACK_INSTRUCTIONS,
    ]
    .into_iter()
    .filter(|base| instructions.starts_with(base))
    .max_by_key(|base| base.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use sha2::Sha256;

    #[test]
    fn model_lookup_matches_catalog_prefix_and_namespace_rules() {
        assert_eq!(for_model("gpt-5.6-sol"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(for_model("gpt-5.6-terra-preview"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(
            for_model("vendor/gpt-5.4-mini-preview"),
            GPT_5_4_MINI_INSTRUCTIONS
        );
        assert_eq!(for_model("codex-auto-review"), GPT_5_4_INSTRUCTIONS);
        assert_eq!(
            for_model("exp-codex-personality"),
            EXP_CODEX_PERSONALITY_INSTRUCTIONS
        );
        for model in [
            "vendor/group/gpt-5.6-sol",
            "vendor!/gpt-5.4",
            "future-model",
        ] {
            assert_eq!(for_model(model), FALLBACK_INSTRUCTIONS, "model {model}");
        }
    }

    #[test]
    fn bundled_prompt_hashes_match_codex_0149_effective_defaults() {
        for (prompt, expected) in [
            (
                GPT_5_6_INSTRUCTIONS,
                "cbefa6b0bede0e332d957fca70ccacf9f12f4c0ecdf81b819e5cbe1a3b16e265",
            ),
            (
                GPT_5_5_INSTRUCTIONS,
                "e58c21f9377e946e2e10f886fcbf6f030e1c6fd9067241c637a56e9e998d3c31",
            ),
            (
                GPT_5_4_INSTRUCTIONS,
                "9721f7a86edc261996e628fe14fade8d66ec60e6cc727274a8da6a03e15464de",
            ),
            (
                GPT_5_4_MINI_INSTRUCTIONS,
                "9109777dc7f3bc9ee9a0d187982b13538c53e0572de2959300f7226e9c59855e",
            ),
            (
                GPT_5_2_INSTRUCTIONS,
                "c9b2fa097ac69cae82c3d2ae12271083890a96521c55ad8dc14cae5168ad3f39",
            ),
            (
                FALLBACK_INSTRUCTIONS,
                "ac8ae107a0d72fe3476b430afb161ea4e67da2e446d778aefc44828160559807",
            ),
            (
                EXP_CODEX_PERSONALITY_INSTRUCTIONS,
                "4cf5dd6317a9920b3f0398f6fa7ca49310b57961f6dd076eb2141acd4f963843",
            ),
        ] {
            assert_eq!(format!("{:x}", Sha256::digest(prompt)), expected);
        }
    }
}
