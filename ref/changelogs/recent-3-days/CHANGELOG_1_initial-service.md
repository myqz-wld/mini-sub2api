---
changelog_id: 1
changed_at: 2026-08-18
---

# CHANGELOG_1_initial-service: Initial mini-sub2api service

## Summary

Created the minimal mini-sub2api v1: a Go coordinator supervises a Rust Codex core and exposes only
the Responses API through downstream keys. It supports Codex subscription OAuth and regular OpenAI
API-key credentials, while recording latency and token usage per downstream API key.

## Changes

### Coordinator and public service (`src/coordinator/`)

- Added `POST /v1/responses` with `ms2a_` bearer authentication, immutable key-to-credential
  routing, streaming pass-through, request IDs, and TTFB timing.
- Added native static TLS for every non-loopback listener; plaintext HTTP is accepted only on
  literal IPv4/IPv6 loopback.
- Added SQLite credential/key metadata, request history, daily per-key aggregates, transactional
  completion, seven-day default detail retention, and explicit aggregate pruning.
- Added supervision of one Rust core over an authenticated ephemeral loopback channel without
  transferring provider secrets into the coordinator.
- Added CLI commands for credential login/lifecycle, downstream key lifecycle, service startup,
  usage history/statistics, and confirmed pruning.

### Codex core (`src/core/codex/`)

- Added Rust forwarding for both Codex subscription OAuth and regular OpenAI API-key credentials,
  with provider-specific construction isolated behind opaque account references.
- Added source-compatible device and browser PKCE login, token rotation, serialized refresh,
  exactly one pre-response 401 replay, explicit upstream revoke, and `requires_login` failure state.
- Added a private atomic `0600` credential vault, one-core state lock, non-secret removal receipts,
  and crash-recoverable idempotent removal/revoke behavior.
- Added strict request-header allowlisting/replacement, response streaming parity, usage
  preservation, cancellation propagation, and literal-loopback proxy bypass.

### Protocol, engineering foundation, and packaging

- Added a versioned Go/Rust private protocol with shared readiness and error fixtures.
- Initialized the repository's durable engineering foundation: shared instructions, English CLI
  copy SSOT, time-bucketed plan/review/changelog indexes, archive helpers, and 500-line source guard.
- Pinned Go 1.26.4 and Rust 1.96.0 with project-local `mise`; confined generated output to `build/`.
- Added debug/release builds for `mini-sub2api` and `mini-sub2api-core-codex`, shared
  `build-info.json`, human-readable `--version`, and machine-checkable `--check-installed`.

## Validation

- `bash scripts/test.sh`
- `bash scripts/build.sh`
- 21 Rust core tests and 2 shared-protocol tests
- Clippy with warnings denied, Go vet, and all Go tests under the race detector
- Actual Go-to-Rust loopback integration covering both credential modes, streaming, usage
  attribution, refresh, revoke recovery, cancellation, TLS, retention, and process restart
- Paired complete review, post-fix review, and residual concurrent-revoke review; all accepted HIGH
  and MEDIUM findings were fixed and both residual reviewers returned `PASS`
- No real OpenAI, ChatGPT, or Codex endpoint was contacted during implementation or validation

## Do Not Split Protection

None. No first-party source file exceeds 500 lines.

## Notes

- Related review:
  [`REVIEW_1_mini-sub2api-security-lifecycle.md`](../../reviews/recent-3-days/REVIEW_1_mini-sub2api-security-lifecycle.md).
- Related completed plan:
  [`PLAN_1_mini-sub2api-initial-service.md`](../../plans/recent-3-days/PLAN_1_mini-sub2api-initial-service.md).
- Codex subscription OAuth/backend compatibility remains source-compatible rather than a public
  third-party API guarantee. The service intentionally adds no dashboard, billing, quotas, key
  rebinding, chat-completions endpoint, multi-account balancing, or request/response body storage.
