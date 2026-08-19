---
review_id: 2
reviewed_at: 2026-08-19
baseline_commit: 66fe0ca1229b7fc2d1e3c17c5c9eca1bf9d37d46
expired: false
skipped_expired: []
---

# REVIEW_2_local-codex-live-smoke: Local Codex subscription import and live usage

## Scope

Targeted credential, refresh-ownership, SSE metadata, and per-key usage review triggered by the
user-authorized local deployment smoke test. No expired files were omitted.

**Review method**:

- Lead Codex implementation session: source inspection, targeted Rust/Go security tests, complete
  repository gates, and a manual localhost deployment against the user's existing Codex
  subscription.
- No additional reviewer session was requested for this bounded follow-up. Confirmed issues below
  required direct automated or live evidence.

**Scope**: 12 changed files, approximately 2,933 lines.

```text
Explicit local Codex access-token snapshot import
OAuth refresh ownership and requires-login transition
SSE response metadata preservation and content-type fallback
Per-downstream-key live usage observation
README operator contract
```

**Machine-readable scope**:

```review-scope
README.md
src/coordinator/internal/cli/cli_test.go
src/coordinator/internal/cli/credential.go
src/coordinator/internal/usage/observer.go
src/coordinator/internal/usage/observer_test.go
src/core/codex/src/cli.rs
src/core/codex/src/codex_auth_import.rs
src/core/codex/src/main.rs
src/core/codex/src/oauth.rs
src/core/codex/src/oauth_integration_tests.rs
src/core/codex/src/server.rs
src/core/codex/src/server_integration_tests.rs
```

**Constraints**: automated tests remained loopback-only. The user explicitly authorized the manual
live smoke request. No token, account id, raw downstream key, request body, or response body was
printed or retained. The existing Codex auth file was hash-compared before and after the test.

## Findings

### Confirmed Issues

| # | Severity | File:Line | Issue | Evidence |
|---|---|---|---|---|
| MS2A-LIVE-001 | MEDIUM | `src/core/codex/src/codex_auth_import.rs:80` | Copying a rotating refresh token into a second independent vault would create dual ownership and could invalidate the original Codex login. | The first uncommitted import draft copied the token; access-token expiry inspection showed refresh was not needed for the smoke test, so the design was changed before any live import. |
| MS2A-LIVE-002 | MEDIUM | `src/core/codex/src/server.rs:296` | The real Codex SSE response omitted `Content-Type`; Go treated the full stream as JSON, so the successful request history stored no Token Usage. | Two user-authorized live requests returned HTTP 200 and a usage-bearing `response.completed`, while history showed `usage: null`; the missing media type was reproduced and localized. |

### Refuted Findings

| Reporter | Claim | Refutation Evidence |
|---|---|---|
| None | None | No finding was classified as refuted. |

### Partial / Unverified

| Area | View | Verified? | Conclusion |
|---|---|---|---|
| None | None | Yes | No partial or unverified material finding remains. |

## Validation / Evidence

- Rust import tests verify ChatGPT mode, required access/ID tokens, account-id agreement, absence of
  refresh-token copying, and secret-free metadata.
- Rust OAuth test verifies an expired access-only snapshot changes to `requires_login` without any
  refresh request.
- Rust core integration verifies a successful streamed request receives synthesized
  `Content-Type: text/event-stream` when the upstream omits it.
- Go observer test verifies SSE usage is still detected defensively when content type is absent.
- `bash scripts/test.sh` passed with 24 Rust core tests, 2 protocol tests, Clippy with warnings
  denied, Go vet, and all Go tests under the race detector.
- `bash scripts/build.sh` passed release packaging and installed checks.
- Final live request through `127.0.0.1:8787` and a one-time downstream key returned HTTP 200 using
  model `gpt-5.6-sol`; history recorded TTFB 2,016 ms, total duration 4,025 ms, 29 input tokens,
  10 output tokens, and 39 total tokens. Daily per-key statistics matched one request and 39 tokens.
- The source `~/.codex/auth.json` hash was unchanged, no refresh token entered mini-sub2api, the
  smoke key was revoked, the imported credential was removed service-only, and the service stopped.

## Fixes Landed

### CRITICAL

1. None.

### HIGH

1. None.

### MEDIUM

1. **`src/core/codex/src/codex_auth_import.rs:80`** — Made local Codex reuse an explicit
   access-token-only snapshot; the refresh token is ignored, and expiry/401 transitions to
   `requires_login` without contacting the token endpoint.
2. **`src/core/codex/src/server.rs:296`** — Synthesized `text/event-stream` for successful
   `stream:true` responses when upstream omits content type; the Go observer also recognizes SSE
   framing defensively so usage attribution cannot depend on that header alone.

### LOW

1. None.

### INFO

1. None.

## Residual Risk

- Imported local Codex access is deliberately temporary and becomes `requires_login` five minutes
  before token expiry. Long-running service deployments must use mini-sub2api's own OAuth login so
  only one vault owns refresh rotation.
- The live state root retains tombstoned credential/key metadata and three request-history rows from
  diagnosis; no active downstream key or provider secret remains.

## Follow-ups

None.
