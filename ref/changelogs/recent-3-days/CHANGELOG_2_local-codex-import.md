---
changelog_id: 2
changed_at: 2026-08-19
---

# CHANGELOG_2_local-codex-import: Safe local Codex subscription smoke path

## Summary

Added an explicit way to reuse the current local Codex ChatGPT access token for short-lived
mini-sub2api tests without copying its rotating refresh token. A real localhost smoke test also
identified and fixed missing SSE media-type handling so per-key Token Usage is retained reliably.

## Changes

### Credential import (`src/core/codex/`, `src/coordinator/internal/cli/`)

- Added `credential import-codex --name NAME --auth-file FILE`.
- Rust reads the explicitly named Codex auth file, requires ChatGPT mode, validates account identity
  from the ID token, and stores only the current ID/access token plus non-secret identity.
- The refresh token is never copied. An expired snapshot changes to `requires_login` without a
  refresh attempt, protecting the original Codex CLI login from refresh-token rotation conflicts.
- Go receives and stores only credential metadata; imported token material remains Rust-vault-owned.

### Live SSE usage (`src/core/codex/src/server.rs`, `src/coordinator/internal/usage/`)

- Added `text/event-stream` to a successful `stream:true` response when the real upstream omits a
  content type.
- Added a defensive SSE framing detector for usage observation when media type is still unavailable.
- Added Rust and Go regressions for response metadata and usage parsing.

## Validation

- `bash scripts/test.sh`
- `bash scripts/build.sh`
- 24 Rust core tests, 2 protocol tests, Clippy with warnings denied, Go vet, and Go race tests
- User-authorized local subscription smoke: HTTP 200; output matched the requested phrase; TTFB
  2,016 ms; duration 4,025 ms; 29 input, 10 output, and 39 total tokens; daily statistics matched
- Codex auth file unchanged; refresh token not copied; test key revoked; temporary credential
  removed service-only; no live listener left behind

## Do Not Split Protection

None. No first-party source file exceeds 500 lines.

## Notes

- Related review:
  [`REVIEW_2_local-codex-live-smoke.md`](../../reviews/recent-3-days/REVIEW_2_local-codex-live-smoke.md).
- Access-only import is for bounded local reuse. Use `credential login codex` for a persistent
  deployment that must refresh independently.
