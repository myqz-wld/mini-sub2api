//! Pinned native instruction fixtures; never linked into production request normalization.
const GPT_6_ASTRA_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.156.0/gpt-6-astra.md");
const DAYBREAK_BLUE_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.156.0/gpt-daybreak-blue.md");
const DAYBREAK_RED_INSTRUCTIONS: &str =
    include_str!("../prompts/codex-0.156.0/gpt-daybreak-red.md");
const GPT_5_6_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.156.0/gpt-5.6.md");
const GPT_5_5_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.156.0/gpt-5.5.md");
const GPT_5_4_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.156.0/gpt-5.4.md");
const FALLBACK_INSTRUCTIONS: &str = include_str!("../prompts/codex-0.156.0/fallback.md");

const MODEL_INSTRUCTIONS: &[(&str, &str)] = &[
    ("gpt-6-astra", GPT_6_ASTRA_INSTRUCTIONS),
    ("gpt-daybreak-blue-latest", DAYBREAK_BLUE_INSTRUCTIONS),
    ("gpt-daybreak-red-latest", DAYBREAK_RED_INSTRUCTIONS),
    ("gpt-5.6-sol", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-terra", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.6-luna", GPT_5_6_INSTRUCTIONS),
    ("gpt-5.5", GPT_5_5_INSTRUCTIONS),
    ("gpt-5.4", GPT_5_4_INSTRUCTIONS),
    ("codex-auto-review", DAYBREAK_BLUE_INSTRUCTIONS),
];

pub(crate) fn for_model(model: &str) -> &'static str {
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
        assert_eq!(MODEL_INSTRUCTIONS.len(), 9);
        for (model, instructions) in MODEL_INSTRUCTIONS {
            assert_eq!(for_model(model), *instructions, "catalog model {model}");
        }
        assert_eq!(for_model("gpt-5.6-sol"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(for_model("gpt-5.6-terra-preview"), GPT_5_6_INSTRUCTIONS);
        assert_eq!(
            for_model("vendor/gpt-5.4-mini-preview"),
            GPT_5_4_INSTRUCTIONS
        );
        assert_eq!(for_model("codex-auto-review"), DAYBREAK_BLUE_INSTRUCTIONS);
        assert_eq!(for_model("exp-codex-personality"), FALLBACK_INSTRUCTIONS);
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
        for (model, prompt) in MODEL_INSTRUCTIONS
            .iter()
            .copied()
            .chain([("fallback", FALLBACK_INSTRUCTIONS)])
        {
            assert!(!prompt.trim().is_empty(), "empty prompt for {model}");
            assert!(
                !prompt.contains("{{ personality }}"),
                "unresolved placeholder for {model}"
            );
        }
    }

    #[test]
    fn bundled_prompt_hashes_match_codex_01560_effective_defaults() {
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
                "2351631dfc5644dc5a45eaaca4139475bd02810ee6cb792d058b551559b3242e",
            ),
            (
                GPT_5_4_INSTRUCTIONS,
                "f8c4032f78fdb19b46f468239c5dc757f745fbb10520e64e1e4afbf747e52fb8",
            ),
            (
                FALLBACK_INSTRUCTIONS,
                "ac8ae107a0d72fe3476b430afb161ea4e67da2e446d778aefc44828160559807",
            ),
        ] {
            assert_eq!(format!("{:x}", Sha256::digest(prompt)), expected);
        }
    }
}
