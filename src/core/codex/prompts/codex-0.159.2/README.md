# Codex 0.159.2 instruction fixtures

These are byte-exact offline comparison/test fixtures from OpenAI Codex `rust-v0.159.2`, commit
`ff6aec96948b70d94983af2641a6b67c94faeff5`; LICENSE and NOTICE accompany them.
Core preserves valid caller bases and never injects these defaults. The Rust snapshot module is test-only.

Eleven catalog models use eight defaults. The three gpt-5.6 models and codex-auto-review
share one; gpt-6.1-sol, gpt-6-sol and gpt-6-luna each have their own literal template. `fallback.md`
covers models absent from the catalog. The catalog no longer contains gpt-5.4;
normal prefix/fallback selection still applies.

## Offline regeneration

```bash
bash scripts/generate-codex-prompts.sh --codex-source ref/sources/codex-v0.159.2 --check
```

Omit `--check` to regenerate; `--output build/codex-prompts` writes a separate copy.
The generator reads the exact commit with `git show`, disables lazy fetching and makes no provider calls.
It validates catalog/default/fallback coverage and copies native instruction templates literally.
The upstream templates already embed personality text; braces, including Astra's `{{connector_id}}`,
remain unchanged. Caller bases are never rendered.
Pinned hashes and offline checks protect exact bytes; do not condense the snapshot Markdown.
