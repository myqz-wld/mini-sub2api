# Codex 0.153.4 base instructions

These effective default instructions are copied from OpenAI Codex `rust-v0.153.4`, commit
`3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`. The source Apache-2.0 LICENSE and NOTICE accompany them.

The eleven catalog models use eight distinct defaults; `fallback.md` and
`exp-codex-personality.md` provide the two local fallback forms. The three gpt-5.6 models share
one default. `codex-auto-review` shares the daybreak-blue instructions and uses Lite settings.

Only the native personality variable is rendered with its catalog default. The pinned Astra
instructions contain a literal `{{connector_id}}` connector-link example, which stays unchanged.
Templates without variables remain literal, including brace syntax examples. Declared personality
slots are rendered and checked; exact pinned hashes protect the resulting defaults. Valid caller base instructions are preserved verbatim at runtime and are never rendered by this generator.

## Offline regeneration

```bash
bash scripts/generate-codex-prompts.sh --codex-source .ref/sources/codex-v0.153.4 --check
```

Omit `--check` to regenerate, or use `--output build/codex-prompts` for a separate copy. The generator
reads the exact commit with `git show`, disables Git lazy fetching and performs no provider calls.
It validates the complete catalog, shared defaults, fallbacks and placeholders before writing.
Rust hash regressions and the offline check protect byte equality with the pinned source.
