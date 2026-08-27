# Codex 0.149.0 base instructions

These files are the effective default model `instructions` from the official OpenAI Codex
`rust-v0.149.0` tag, commit `758ef40f50c1a458425c7cfbf1eb12cbc07af0b0`.
The copied assets retain that source's Apache-2.0 `LICENSE` and `NOTICE` in this directory.

- Model prompts were extracted from `codex-rs/models-manager/models.json`.
- `{{ personality }}` was resolved with the catalog's empty `personality_default`, matching the
  default `ModelInfo::get_model_instructions(None)` result.
- `fallback.md` is the tag's `codex-rs/models-manager/prompt.md` fallback.
- `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna` share `gpt-5.6.md`.
- `gpt-5.4` and `codex-auto-review` share `gpt-5.4.md`.
- `exp-codex-personality.md` preserves the tag's special local fallback with its default empty
  personality slot; other unknown models use `fallback.md`.

The Rust hash test protects these copied compatibility assets from silent drift. Update the prompt
files, lookup table, hashes, request-shape tests, and compatibility documentation together when the
pinned Codex version changes.
