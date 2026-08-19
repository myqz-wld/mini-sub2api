---
review_id: 1
reviewed_at: 2026-08-18
baseline_commit: unborn  # The new repository has no commit yet.
expired: true            # Refresh to a real commit baseline after the initial commit.
skipped_expired: []
---

# REVIEW_1_mini-sub2api-security-lifecycle: Initial service security and lifecycle review

## Scope

This record consolidates the complete initial implementation review, the authorized fixes, the
post-fix security/lifecycle pass, and the bounded concurrent-revoke residual pass. There were no
pre-existing expired files.

**Review method**:

- Codex A: independent `reviewer-codex` / `codex-cli` Agent Deck session, read-only complete-scope
  review plus targeted loopback-only probes.
- Codex B: independent `reviewer-codex` / `codex-cli` Agent Deck session, read-only complete-scope
  review plus targeted loopback-only probes.
- The user explicitly authorized two Codex reviewers because the Claude and Grok channels were
  unavailable. This is a same-adapter exception and does not claim heterogeneous corroboration.

**Scope**: 74 implementation, protocol, automation, and root contract files; approximately 9,553
lines. The first pass covered the complete scope. Each changed security/lifecycle file was covered
again after its fix, and the final revoke change received a bounded residual pass from both
reviewers.

```text
Repository contracts and packaging metadata
scripts/ automation
src/coordinator/ Go coordinator, CLI, persistence, listener, and integration tests
src/core/codex/ Rust credential vault, OAuth lifecycle, and Responses forwarding
src/protocol/v1/ shared Go/Rust private protocol and fixtures
```

**Machine-readable scope**:

```review-scope
AGENTS.md
CLAUDE.md
Cargo.toml
README.md
UI_COPY_LANGUAGE.md
go.mod
mise.toml
scripts/build.sh
scripts/file-level-review-expiry.sh
scripts/ref-archive-reminder-pre-commit.sh
scripts/test.sh
src/coordinator/cmd/generate-build-info/main.go
src/coordinator/cmd/mini-sub2api/main.go
src/coordinator/integration/credential_recovery_test.go
src/coordinator/integration/e2e_test.go
src/coordinator/internal/adapter/client.go
src/coordinator/internal/adapter/supervisor.go
src/coordinator/internal/adapter/supervisor_test.go
src/coordinator/internal/buildmeta/buildmeta.go
src/coordinator/internal/buildmeta/buildmeta_test.go
src/coordinator/internal/cli/cli.go
src/coordinator/internal/cli/cli_test.go
src/coordinator/internal/cli/common.go
src/coordinator/internal/cli/core_command.go
src/coordinator/internal/cli/credential.go
src/coordinator/internal/cli/key.go
src/coordinator/internal/cli/serve.go
src/coordinator/internal/cli/usage.go
src/coordinator/internal/httpapi/errors.go
src/coordinator/internal/httpapi/handler.go
src/coordinator/internal/httpapi/handler_test.go
src/coordinator/internal/httpapi/headers.go
src/coordinator/internal/httpapi/listener.go
src/coordinator/internal/httpapi/listener_test.go
src/coordinator/internal/httpapi/stream.go
src/coordinator/internal/storage/credentials.go
src/coordinator/internal/storage/instance_lock_unix.go
src/coordinator/internal/storage/keys.go
src/coordinator/internal/storage/migrations.go
src/coordinator/internal/storage/migrations_test.go
src/coordinator/internal/storage/models.go
src/coordinator/internal/storage/prune.go
src/coordinator/internal/storage/queries.go
src/coordinator/internal/storage/requests.go
src/coordinator/internal/storage/storage_test.go
src/coordinator/internal/storage/store.go
src/coordinator/internal/storage/usage_test.go
src/coordinator/internal/usage/observer.go
src/coordinator/internal/usage/observer_test.go
src/core/codex/Cargo.toml
src/core/codex/src/build_info.rs
src/core/codex/src/cli.rs
src/core/codex/src/error.rs
src/core/codex/src/http_body.rs
src/core/codex/src/http_client.rs
src/core/codex/src/main.rs
src/core/codex/src/oauth.rs
src/core/codex/src/oauth_integration_tests.rs
src/core/codex/src/oauth_login.rs
src/core/codex/src/oauth_login_tests.rs
src/core/codex/src/oauth_tests.rs
src/core/codex/src/server.rs
src/core/codex/src/server_integration_tests.rs
src/core/codex/src/server_tests.rs
src/core/codex/src/test_support.rs
src/core/codex/src/vault.rs
src/core/codex/src/vault_tests.rs
src/protocol/v1/README.md
src/protocol/v1/fixtures/error.json
src/protocol/v1/fixtures/readiness.json
src/protocol/v1/go/contract.go
src/protocol/v1/go/contract_test.go
src/protocol/v1/rust/Cargo.toml
src/protocol/v1/rust/src/lib.rs
```

**Constraints**: no real OpenAI, ChatGPT, or Codex endpoint could be contacted; active probes used
literal-loopback mocks or reserved invalid hostnames captured by loopback proxies. Findings required
direct code or test evidence. Reviewer repositories remained read-only.

## Findings

### Confirmed Issues

| # | Severity | File:Line | Issue | A | B | Evidence |
|---|---|---|---|---|---|---|
| MS2A-REB-001 | HIGH | `src/core/codex/src/server.rs:72` | Ambient proxy settings could route a plaintext literal-loopback request, provider bearer, and body to a non-loopback proxy. | Confirmed | Confirmed | Both reviewers diverted a live loopback mock request to a dead proxy with empty `NO_PROXY`; bounded rebuttal retained HIGH. |
| MS2A-FULL-B-002 | MEDIUM | `src/core/codex/src/server.rs:159` | Concurrent requests receiving 401 with the same stale OAuth token could each force another refresh rotation. | Confirmed | Confirmed | A synchronized probe observed two refresh grants before remediation. |
| MS2A-FULL-A-002 / B-003 | MEDIUM | `src/core/codex/src/vault.rs:207` | A crash after vault deletion but before SQLite tombstoning left deletion retries unable to converge. | Confirmed | Confirmed | Repeating core removal returned not-found and retained a disabled coordinator record before remediation. |
| MS2A-POST-001 | MEDIUM | `src/core/codex/src/cli.rs:193` | Simultaneous OAuth revoke commands could both contact the upstream authority during the record-lock-to-receipt handoff. | Confirmed | Confirmed | Both post-fix reviewers reproduced duplicate revokes; one observed 3 duplicates in 30 trials. |

### Refuted Findings

| Reporter | Claim | Refutation Evidence |
|---|---|---|
| None | None | No reported finding was classified as refuted. |

### Partial / Unverified

| Area | A View | B View | Verified? | Conclusion |
|---|---|---|---|---|
| None | None | None | Yes | No partial or unverified material finding remained. |

## Validation / Evidence

- `bash scripts/test.sh` passed after all fixes: 21 Rust core tests, 2 protocol tests, the explicit
  ambient-proxy regression, Clippy with warnings denied, Go vet, and all Go race tests.
- `bash scripts/build.sh` passed release builds, packaging, build metadata generation, both
  `--version` commands, and both `--check-installed` commands.
- The race-enabled actual Go-to-Rust cleanup test passed five consecutive runs and verified two
  concurrent revoke commands produce exactly one upstream call.
- Codex A independently ran the former revoke-race probe for 100 isolated credentials: 100/100
  pairs succeeded with exactly one upstream revoke and no missing or duplicate call.
- Codex B independently reran the former revoke-race probe and observed one ordinary success, one
  recovered success, and exactly one upstream call.
- Both residual reviewers confirmed the receipt contains only account reference, action kind, and
  completion time, remains mode `0600`, and is persisted before secret-record deletion.
- No validation contacted a real provider endpoint. Disposable review artifacts were confined to
  the assigned `/tmp/agent-deck-review/MS2A-DR-20260818-01` directory and removed afterward.

## Fixes Landed

### CRITICAL

1. None.

### HIGH

1. **`src/core/codex/src/http_client.rs:4`** — Added literal-loopback URL detection and selected a
   proxy-bypassing reqwest client for loopback inference, OAuth login/refresh, and revoke traffic,
   while retaining normal proxy support for external HTTPS endpoints.

### MEDIUM

1. **`src/core/codex/src/server.rs:159`** — Captured the failed access token and double-checked it
   under the account lock so concurrent 401 waiters share one forced refresh and each request still
   receives at most one replay.
2. **`src/core/codex/src/vault.rs:207`** — Made removal idempotent and crash-recoverable with a
   private non-secret completion receipt written before secret-record deletion; coordinator retry
   can then finish the SQLite tombstone.
3. **`src/core/codex/src/cli.rs:193`** — Kept the same account lock continuously across successful
   upstream revoke, receipt persistence, and record deletion; a losing waiter rechecks the receipt
   and returns recovered success without network access.

### LOW

1. None.

### INFO

1. None.

## Residual Risk

- ChatGPT subscription routing and OAuth reproduce source-observed Codex behavior rather than a
  documented third-party compatibility contract and may require maintenance after upstream change.
- The owner-only vault is not encrypted at rest; host or root compromise can expose provider
  credentials.
- Non-loopback serving relies on operator-provided static TLS certificates; issuance, trust, and
  renewal are outside v1.
- v1 supports one coordinator and one Codex core per state directory, not HA or a cross-host core.
- Because the repository is unborn, this review cannot name a real final commit. `expired: true`
  deliberately forces the first post-commit review to establish a usable commit baseline.
- The two-reviewer process used the user's approved same-adapter exception and therefore provides
  independent repetition, not heterogeneous model corroboration.

## Follow-ups

No implementation follow-up remains in the approved v1 scope. After the user creates the initial
commit, the next review should refresh this record's covered scope against a real commit baseline.
