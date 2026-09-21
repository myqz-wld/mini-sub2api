# Codex 0.153.4 instruction fixtures

These are byte-exact offline comparison/test fixtures from OpenAI Codex `rust-v0.153.4`, commit
`3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`; LICENSE and NOTICE accompany them.
Core preserves valid caller bases and never injects these defaults. The Rust snapshot module is test-only.

Eleven catalog models use eight defaults. The three gpt-5.6 models share one; codex-auto-review uses
daybreak-blue with Lite settings. `fallback.md` and `exp-codex-personality.md` cover local fallback forms.

## Offline regeneration

```bash
bash scripts/generate-codex-prompts.sh --codex-source .ref/sources/codex-v0.153.4 --check
```

Omit `--check` to regenerate; `--output build/codex-prompts` writes a separate copy.
The generator reads the exact commit with `git show`, disables lazy fetching and makes no provider calls.
It validates catalog/default/fallback coverage and renders only declared native personality slots;
literal braces, including Astra's `{{connector_id}}`, remain unchanged. Caller bases are never rendered.
Pinned hashes and offline checks protect exact bytes; do not condense the snapshot Markdown.
