---
review_id: 3
reviewed_at: 2026-08-19
baseline_commit: dc1e1658317976dfcf7046a45622272436834ad7
expired: false
skipped_expired: []
---

# REVIEW_3_codex-request-parity: Native Codex request parity

## Scope

Compared three loopback-captured request paths produced with Codex CLI 0.147.0 and
mini-sub2api, then reviewed the bounded parity remediation at the baseline above:

1. Codex CLI with ChatGPT subscription auth using its OpenAI HTTP fallback serializer;
2. Codex CLI with a mini-sub2api key mapped to the same subscription snapshot;
3. an ordinary Responses HTTP client with a mini-sub2api key mapped to subscription auth.

The capture retained only body lengths/hashes, redacted JSON structure, header names, and
ephemeral HMACs for credentials and identifiers. No raw token, account id, prompt body, or
response body was retained. All endpoints were literal loopback; no real model request was sent.

The conversion rules were checked against the sibling Codex source, principally
`codex-rs/core/src/client.rs::build_responses_request`,
`codex-rs/core/src/responses_metadata.rs`,
`codex-rs/tools/src/tool_spec.rs::create_tools_json_for_responses_api`, and the current
`codex-rs/models-manager/models.json` profiles.

**Review method**: lead Codex session, current installed Codex binary, sibling source inspection,
two-sided loopback capture, deterministic redacted comparison, Rust unit/integration tests, and a
real Go coordinator to Rust core to loopback-upstream race-enabled test.

**Machine-readable scope**:

```review-scope
README.md
src/coordinator/integration/request_parity_test.go
src/coordinator/internal/adapter/client.go
src/coordinator/internal/adapter/client_test.go
src/coordinator/internal/httpapi/handler_test.go
src/coordinator/internal/httpapi/headers.go
src/core/codex/src/main.rs
src/core/codex/src/request_normalizer.rs
src/core/codex/src/request_normalizer_tests.rs
src/core/codex/src/server.rs
src/core/codex/src/server_integration_tests.rs
src/core/codex/src/server_tests.rs
```

## Findings

### Confirmed Issues

| # | Severity | Area | Original evidence | Disposition |
|---|---|---|---|---|
| MS2A-PARITY-001 | MEDIUM | Request headers | Current Codex `session-id`, `thread-id`, and Responses-Lite marker reached the public service but were removed before the subscription upstream; `originator` was overwritten. | **FIXED**: all reviewed Codex metadata headers now cross both Go and Rust allowlists; a supplied originator wins and missing subscription originator becomes `mini_sub2api`. |
| MS2A-PARITY-002 | MEDIUM | Request encoding | Native subscription HTTP used zstd, but `Content-Encoding` was removed. Direct capture was 77,230 decoded bytes and 25,679 wire bytes. | **FIXED**: content encoding and the encoded body are preserved end to end; non-identity encoded bodies are never parsed or rewritten. |
| MS2A-PARITY-003 | MEDIUM | Body fidelity | A minimal ordinary request remained a 78-byte, three-field body instead of a Codex-shaped request. | **FIXED**: plain subscription-bound JSON is transformed with the current Codex Lite/non-Lite construction rules, while the client's tools and semantic controls remain authoritative. |
| MS2A-PARITY-004 | INFO | Transport | Native Codex prefers WebSocket for current models, while mini-sub2api exposes HTTP/SSE only. | **ACCEPTED PRODUCT BOUNDARY**: the user explicitly selected no WebSocket compatibility. Configuration declares `supports_websockets = false`; no Upgrade or 426 emulation was added. |

### Quantified Pre-Fix Diff

Codex-through-mini was already close to native HTTP fallback but not byte-identical:

- both bodies had the same 11 top-level fields, 7 input items, model, stream/store flags,
  reasoning, text controls, include list, tool choice, and parallel-tool setting;
- 9 of 10 prompt text segments matched by hash; one dynamic environment segment differed;
- decoded body was 63,736 bytes versus 77,230 bytes (17.47% smaller), primarily because the
  custom provider produced a shorter tool description and omitted five internal metadata fields;
- native wire body was zstd-compressed to 25,679 bytes, while the custom-provider body was 63,736
  uncompressed bytes;
- authorization and account identity matched by ephemeral HMAC; Accept, Content-Type, User-Agent,
  client request id, beta features, turn metadata, and window id matched exactly;
- direct-only upstream headers were `content-encoding`, `session-id`, `thread-id`, and
  `x-openai-internal-codex-responses-lite`.

The original pure-client body had only `model`, `input`, and `stream`: 78 bytes, or 0.101% of the
77,230-byte native decoded request. The old 29-byte mock was 0.038% and proved transport and secret
boundaries, not request fidelity.

### Post-Fix Structural Diff

- The cross-language pure-client fixture now reaches the subscription upstream with the same 11
  top-level field categories as the captured native request.
- `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna` use Responses Lite: exact client tools move to
  the first `additional_tools` item, instructions become a developer message, and parallel tool
  calls are false. Current non-Lite models retain top-level instructions/tools.
- Missing `tool_choice`, reasoning effort/context, encrypted-reasoning include, text verbosity,
  cache key, session/thread/turn/install/window identities, and `client_metadata` receive
  source-derived defaults. `store` is false. Explicit model, stream, reasoning/text controls, and
  tool choice are preserved.
- The fixture's two client tools remain exactly two after conversion, with equal type, name,
  description, and parameter schema. No mini-sub2api tool is injected.
- A compact unit fixture containing instructions and two tools normalizes to 889 decoded bytes.
  It remains much smaller than the 77,230-byte native coding turn because mini-sub2api does not
  invent Codex's project instructions, tool descriptions, or conversation history.
- Already Codex-shaped JSON keeps its body bytes. Existing zstd or other encoded bodies keep both
  body bytes and `Content-Encoding`. Regular OpenAI API-key credentials remain byte-transparent.

## Validation / Evidence

- `bash scripts/test.sh` passed.
- 28 Rust core tests and 2 Rust protocol tests passed.
- Cargo fmt, Clippy for all targets with warnings denied, and the debug build passed.
- Go fmt, vet, and all coordinator tests under the race detector passed.
- The real Go to Rust parity regression asserted subscription auth reconstruction, 11 top-level
  fields, source-derived Lite defaults, consistent header/body metadata, exact client tools, SSE
  completion, and per-key usage attribution.
- Header-layer tests cover current Codex metadata and zstd transparency. The ordinary API-key Rust
  integration still proves exact body transparency.
- No validation contacted a real OpenAI, ChatGPT, or Codex endpoint.

## Fixes Landed

### CRITICAL

1. None.

### HIGH

1. None.

### MEDIUM

1. Preserved current Codex headers, request encoding, and supplied originator across both process
   boundaries.
2. Added a source-derived subscription request normalizer with model profiles, exact client-tool
   preservation, and consistent synthesized metadata.
3. Added unit, Rust integration, Go header-layer, and real cross-language regressions.

### LOW

1. None.

### INFO

1. Documented HTTP/SSE-only operation and `supports_websockets = false`.

## Residual Risk

- WebSocket transport is intentionally unsupported. Clients must select HTTP/SSE directly.
- The normalizer aligns the protocol envelope but does not fabricate Codex's large built-in coding
  prompt, project context, conversation history, or tool catalog. Doing so would change caller
  semantics and violate the exact-tool requirement.
- A non-identity encoded request is passed through without inspection. Native Codex requests are
  already shaped before compression; an ordinary client that compresses a minimal non-Codex body
  opts out of normalization.
- Model profile defaults can drift as sibling Codex models change. A Codex model-table or request
  serializer update is the trigger for repeating this parity capture.

## Follow-ups

Repeat the bounded loopback parity capture when the installed Codex serializer/model profiles
change. No current remediation remains open.
