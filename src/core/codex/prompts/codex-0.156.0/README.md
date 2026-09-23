# Codex 0.156.0 instruction fixtures

These are byte-exact offline comparison/test fixtures from OpenAI Codex `rust-v0.156.0`, commit
`fe74a774532af67b5a4a3dec03ce9469e17f89af`; LICENSE and NOTICE accompany them.
Core preserves valid caller bases and never injects these defaults. The Rust snapshot module is test-only.

Nine catalog models use six defaults. The three gpt-5.6 models share one; codex-auto-review uses
daybreak-blue with Lite settings. `fallback.md` covers models absent from the catalog. The upstream
catalog no longer includes gpt-5.2 or gpt-5.4-mini; normal prefix/fallback selection still applies.

## Offline regeneration

```bash
bash scripts/generate-codex-prompts.sh --codex-source .ref/sources/codex-v0.156.0 --check
```

Omit `--check` to regenerate; `--output build/codex-prompts` writes a separate copy.
The generator reads the exact commit with `git show`, disables lazy fetching and makes no provider calls.
It validates catalog/default/fallback coverage and copies native instruction templates literally.
The upstream templates already embed personality text; braces, including Astra's `{{connector_id}}`,
remain unchanged. Caller bases are never rendered.
Pinned hashes and offline checks protect exact bytes; do not condense the snapshot Markdown.
