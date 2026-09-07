//! Pinned native instruction fixtures; never linked into production request normalization.
const GPT_6_ASTRA_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-6-astra.md");
const DAYBREAK_BLUE_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.153.4/gpt-daybreak-blue.md");
const DAYBREAK_RED_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.153.4/gpt-daybreak-red.md");
const GPT_5_6_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-5.6.md");
const GPT_5_5_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-5.5.md");
const GPT_5_4_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-5.4.md");
const GPT_5_4_MINI_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-5.4-mini.md");
const GPT_5_2_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/gpt-5.2.md");
const FALLBACK_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.153.4/fallback.md");
const EXP_CODEX_PERSONALITY_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.153.4/exp-codex-personality.md");

const MODEL_INSTRUCTIONS: &[(&str, &str)] = &[
    ("gpt-6-astra", GPT_6_ASTRA_INSTRUCTIONS),
    ("gpt-daybreak-blue-latest", DAYBREAK_BLUE_INSTRUCTIONS),
    ("gpt-daybreak-red-latest", DAYBREAK_RED_INSTRUCTIONS),
    ("gpt-5.6-sol", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-terra", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-luna", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.5", GPT_5_5_INSTRUCTIONS),
    ("gpt-5.4-mini", GPT_5_4_MINI_INSTRUCTIONS),
    ("gpt-5.4", GPT_5_4_INSTRUCTIONS),
    ("gpt-5.2", GPT_5_2_INSTRUCTIONS),
    ("codex-auto-review", DAYBREAK_BLUE_INSTRUCTIONS),
];

pub(crate) fn for_model(model: &str) -> &'static str {
    if model == "exp-codex-personality" {
        return EXP_CODEX_PERSONALITY_INSTRUCTIONS;
    }
    find_by_longest_prefix(model)
        .or_else(|| find_by_namespaced_suffix(model))
        .unwrap_or(FALLBACK_INSTRUCTIONS)
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use sha2::Sha256;

    #[test]
    fn model_lookup_matches_catalog_prefix_and_namespace_rules() {
        assert_eq!(MODEL_INSTRUCTIONS.len(), 11);
        for (model, instructions) in MODEL_INSTRUCTIONS {
            assert_eq!(for_model(model), *instructions, "catalog model {model}");
        }
        assert_eq!(for_model("gpt-5.6-sol"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(for_model("gpt-5.6-terra-preview"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(
            for_model("vendor/gpt-5.4-mini-preview"),
            GPT_5_4_MINI_INSTRUCTIONS
        );
        assert_eq!(for_model("codex-auto-review"), DAYBREAK_BLUE_INSTRUCTIONS);
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
    fn all_bundled_defaults_are_nonblank_and_fully_rendered() {
        for (model, prompt) in MODEL_INSTRUCTIONS.iter().copied().chain([
            ("fallback", FALLBACK_INSTRUCTIONS),
            ("exp-codex-personality", EXP_CODEX_PERSONALITY_INSTRUCTIONS),
        ]) {
            assert!(!prompt.trim().is_empty(), "empty prompt for {model}");
            assert!(
                !prompt.contains("{{ personality }}"),
                "unresolved placeholder for {model}"
            );
        }
    }

    #[test]
    fn bundled_prompt_hashes_match_codex_01534_effective_defaults() {
        for (prompt, expected) in [
            (
                GPT_6_ASTRA_INSTRUCTIONS,
                "152dfaeeb552876190962be1c12c93d426840ff12691f648261554a7675a6698",
            ),
            (
                DAYBREAK_BLUE_INSTRUCTIONS,
                "ebd0d5854abd07dc38300a71e027204eb028e9fa443c59d18e36fcc24289e818",
            ),
            (
                DAYBREAK_RED_INSTRUCTIONS,
                "40a1232c8bd01a87dc2283e5ae3c75f2b054dc2a12cf04e5a279c26e5c541b9b",
            ),
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
