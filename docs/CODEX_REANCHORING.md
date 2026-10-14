# Codex re-anchoring checklist

[Compatibility and dated evidence](CODEX_COMPATIBILITY.md) ·
[Capture suite](../src/coordinator/integration/NATIVE_PARITY.md) · [Behavior](BEHAVIOR.md)

Use this checklist for every stable Codex anchor change, including patch releases.
Record the exact old/new tags and commits, changed native behavior, gateway adaptations,
validation results and remaining platform/provider limits in the compatibility report.
Keep project-selected behavior separate from native behavior using the
[ownership table](CODEX_COMPATIBILITY.md#behavioral-ownership-and-assessment).

## Establish the reference

1. Resolve the latest **stable** release from the official `openai/codex` releases, then freeze
   its tag and full commit for this task. Exclude prereleases unless explicitly requested.
2. Retain the exact source under `ref/sources/codex-v<VERSION>` and the official CLI with its
   matching Code Mode host under `ref/tools/codex-v<VERSION>`. Verify release asset digests,
   archive members, executable hashes and `codex --version`; record portable provenance.
   Use these isolated references without replacing globally installed clients.
3. Compare the two exact tagged **trees**, including lockfiles and release scripts. A patch
   branch may diverge: a merge-base comparison or release summary alone is insufficient.
4. List changed native surfaces below, identifying unchanged surfaces by exact source/blob
   equality. Update the gateway version/header anchor, exact source checks, CLI prerequisites,
   prompt-fixture generator and current documentation together. Preserve historical results.

## Check each compatibility surface

| Surface | What to compare | Repository validation / implementation |
|---|---|---|
| Runtime identity and authentication | Version, Originator and UA; runtime OS/architecture; OAuth/browser/device/refresh identities and header emission order. | `upstream_request.rs`, `codex_user_agent.rs`, `upstream_request_oauth_wire_tests.rs`, OAuth loopback tests and native built-in-provider captures. |
| HTTP and WS headers | Complete names, casing, order, presence, values and upgrade-only metadata. Compare the built-in OpenAI provider as well as custom providers; they emit different conditional headers. | `upstream_header_order.rs`, `upstream_classifier.rs`, `native_header_parity_test.go`, `native_wire_shape_capture_test.go`. |
| JSON field order and shape | Outer routing order and producer-specific nested order; omitted/null/empty/false distinctions, integer versus string controls, tool/schema order, late metadata mutation and WS type/control frames. | `native_request_types.rs`, request/emulation/schema order tests, recursive ordered capture oracle in `native_wire_shape_test.go` and `caller_wire_contract_test.go`. Never use unordered JSON equality as the sole oracle. |
| Compression and transport fingerprint | HTTP zstd and WS deflate settings, framing/masking/control behavior; TLS implementation and ClientHello; ALPN, HTTP/2 SETTINGS order/values, connection window and Authorization HPACK mode. | `Cargo.lock`, `transport_registry.rs`, `websocket_connector.rs`, `scripts/cargo.sh`, `scripts/prepare-openssl.sh`, `native_tls_test.go`, `upstream_hpack_tests.rs` and raw WS capture tests. |
| Models and instructions | Complete catalog/default/fallback coverage, literal prompt bytes, tools, effort/verbosity, optional modes and supported control omission. Preserve caller bases and project parameter policy. | `scripts/generate-codex-prompts.sh`, `codex_instruction_snapshots.rs`, model/default tests and `native_model_matrix_test.go`. Snapshots remain test-only. |
| Tool observation budgets | Argument/message limits, metadata byte counters, source/result order, UTF-8 boundaries and retained completeness evidence. | `tool_observation_budget.rs`, native budget captures and the extracted comparison oracle under `ref/tools/`. Reuse an older extraction only after its manifest/source hashes match the new tagged algorithm; otherwise re-extract and compare. |
| Identity and continuation | UUID formats and exact content hashes, installation/session/thread/turn/window lifetime, routing token first-value behavior, forks, full/incremental history, WS reuse and compaction acceptance. | `request_identity*`, `subscription_*`, native lifecycle/compaction/reference matrices; account/Key/restart/expiry negatives. |
| Errors and recovery | HTTP status versus SSE/WS envelopes, context-limit classification and next-turn compaction, standalone errors, real failed footers, retry-delay location/parsing, quota/flex terminal behavior and 426 handshake fallback. | `response_failure.rs`, `response_sse_translation.rs`, `response_privacy.rs`, deferred WS tests, `native_retry1620_test.go`, `native_error_recovery1621_test.go`. Test real CLI behavior and no gateway replay. |
| Privacy and accounting | Public header/error allowlists, typed ID projection, content-free diagnostics, failed-history rejection, single outcome/usage accounting, cancellation and bounded resource release. | Standard Rust and Go privacy/security/lifecycle tests, `responses_error_recovery_test.go`, `responses_sse_error_terminal_test.go`. |

Rust paths in the table are relative to `src/core/codex/src/`; Go capture/test paths are relative
to `src/coordinator/integration/`. Exact native source paths can move between releases: record
the resolved source symbols and changed paths in that release's evidence.

The error-tail distinction is a retained regression: an ordinary SSE `error` can precede a real
`response.failed` by more than one second. Preserve the existing read deadline and late footer
usage; the one-second terminal tail starts on a response terminal. Error-only fallback belongs
at EOF/read failure and must not mask malformed, oversized or conflicting tail data.

When creating a native-readable failure from a standalone error, test retry advice in each
carrier: valid nested advice wins; absent/invalid nested advice falls back to valid top-level
advice. The synthesized `response.failed` must place that advice inside `response.error.headers`.
Use a real CLI overload case: losing its delay changes the result from retryable to terminal.
Keep the internal protocol reference synchronized with status/envelope exceptions and accounting
markers whenever an error adaptation changes.

## Validate and record

Run all ordinary suites against loopback mocks. The native launcher rejects missing/wrong
prerequisites and rebuilds the current Core; a successful run must also have **no skipped cases**.

Set the version variable to the stable tag frozen for this task.

```bash
codex_anchor_version=0.162.1
bash scripts/generate-codex-prompts.sh --codex-source "ref/sources/codex-v${codex_anchor_version}" --check
bash scripts/test.sh
bash scripts/test-native-parity.sh
bash scripts/test-scaffold-parity.sh
bash scripts/build.sh
```

Record full-suite group/leaf counts and any focused repair reruns separately. Check file sizes,
documentation links, archive membership, build metadata and personal-path exclusion. Retain
only bounded aggregate evidence; captures, credentials and raw request/response bodies do not
belong in reports or packages. Source and local policy/reference directories are excluded from
distribution inputs.

macOS ClientHello and application-wire checks do not certify Linux TLS, a full negotiated HTTP/2
connection, packet timing or remote fingerprint classification. Verify those only with evidence
from the corresponding runtime and scope. Live provider calls, deployment and publication need
task-specific authorization; a local version alignment does not establish live backend capability.

## Current patch-line note

The 0.162.1 anchor uses commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. Its exact tagged tree
diff from 0.162.0 changes ten files in the version/TUI/startup areas. Protocol, error handling,
model catalog, instruction bodies, lockfile and release TLS scripts are identical. The internal
`CodexSubscription1620` enum continues to name this unchanged protocol family; runtime headers,
source checks, CLI and fixture provenance are pinned to 0.162.1.
