# Capture tests

[Setup](../../../README.md) · [Behavior](../../../docs/BEHAVIOR.md)

## Run and prerequisites

```bash
bash scripts/test.sh
bash scripts/test-native-parity.sh
mise exec -- python3 scripts/prepare-opencode-tests.py
bash scripts/test-scaffold-parity.sh
```

Pinned clients: Codex **0.156.0** (`fe74a774532af67b5a4a3dec03ce9469e17f89af`) and OpenCode
**1.18.29** (`16747470f976aca3d362ad730bcd3fe82ecc2c9a`). Missing/wrong prerequisites fail.
Clients are verified on macOS arm64. Standard suites use loopback mocks and never fall back to providers.
Code Mode execution requires the matching 0.156.0 `codex-code-mode-host` companion in the CLI's
installation layout. An isolated CLI path can be supplied with `MINI_SUB2API_NATIVE_CODEX_BINARY`;
`MINI_SUB2API_CODEX_SOURCE` selects the exact read-only source checkout.

## Method

- Run actual clients in isolated home/config/auth/project state with synthetic credentials and blocked
  external networking. OpenCode uses in-memory SQLite, no snapshots and a null log sink. Join only owned children.
- Compare direct traffic with client → public Go gateway → Rust Core. Check API-key byte equality and
  Subscription ordered content/settings plus typed identity relationships.
- Parse ordered JSON tokens recursively, including serialized turn metadata. Fail on field order,
  additions/omissions, duplicate keys, null/empty-container/empty-string/boolean differences.
  The built-in OpenAI fixture also requires exact header name order and casing; custom providers
  do not supply all built-in defaults. Captures and scalar payloads remain memory-only.
- Tap HTTP/WS before decoding; independently parse masking, fragmentation, controls and deflate.
  Bounds: 64 connections/tap, 64 MiB/connection, 128 application requests and 64 MiB/application capture.
  Raw bodies stay in memory; failures report structure.
- Verify Lite UUIDv5 from exact tools/base bytes. Never recursively strip IDs or rewrite opaque
  arguments/ciphertext/resource references in the oracle.
- TLS probes stop at ClientHello before credentials. Compare stable negotiation fields, excluding
  randomness/key material; this does not certify negotiation or remote fingerprint classification.

## Coverage

Counts describe overlapping matrices, not additive independent cases.

| Matrix | Assertions |
|---|---|
| Codex HTTP/WS; both credentials; ordinary/Lite | Full requests, prewarm/increments, tool loops and next turn without extra business inference |
| Explicit references: 54 cases | HTTP reconstruction, WS remote continuation, missing state; transport/format/identity combinations |
| Anonymous histories: 96 three-call cases | Credentials × JSON/SSE/WS-reconnect × ordinary/converted/formed/forced Lite × text/tool × exact/reduced histories |
| Minimal histories: 24 cases | Model/input only; no instructions/tools/IDs; ordinary/Lite across credentials/transports |
| Configuration: 48 four-call cases | Stable association under changed/current/omitted settings; eligible WS suffix, Lite IDs and API-key bytes |
| Reasoning: 96 four-call cases | Include visibility, full/reference histories, JSON/SSE/WS/reconnect, caller markers, hidden-state restoration and tool/user turns |
| Caller bases: 144 two-call cases | Missing/invalid/explicit bases, both credentials and HTTP/WS ordinary/Lite; no defaults, stable prefix ownership |
| Actual OpenCode: 18 cases | Custom-provider text/read/denied-file and built-in OpenAI plugin; continuity, tool turns, headers and bytes |
| Models/context | All 9 catalog models; literal bases, personality removal, ordered developer messages, AGENTS/Skills/permissions and caller environment |
| 0.156.0 controls | Actual CLI HTTP/WS effort changes with the capability enabled; analytics/model/effort metadata, scoped cache affinity, Guardian references and tool-result evidence |
| Built-in wire shape: 28 requests | Actual CLI app-server; HTTP fallback/WS × ordinary/Lite × two isolated processes; complete header order/presence/casing and recursive JSON shape, including tool loops and the next turn |
| Identity/privacy | Key/device/shared-account isolation, forks/owners, explicit conflicts, restart/corruption and required references |
| HTTP boundaries: 64 cases | EOF/delta/DONE-only/all terminals, 9 MiB output, JSON/SSE, no replay, exact API-key bytes and usage/status |
| WS controls: 24 cases | Inject/append/generic ownership, stale-history rejection and full replacement |
| Cache/completion | Publication order, item/footer proof, failure/expiry/pressure, interning and tool consumption |
| WS/compaction | Reuse/reconnect, first routing token, prewarm, uncertain sends, V2 item-done and window commit |
| Compaction continuation: 36 cases | Explicit/in-band windows, two HTTP deltas, full WS recovery, Lite setup and missing-window errors |
| Anonymous checkpoints: 36 four-call cases | Configuration/context changes, stable session/thread/window, new turn and exact checkpoint across credentials/transports/formats |
| Tools | Actual native nested host callbacks; bare/OpenCode direct tools; schema order/duplicates and deterministic prefixes |

Anonymous WS reconnects each submission so socket binding cannot hide matching defects.
Custom OpenCode asserts no recognized session/reference carrier; built-in tests check actual
`session-id/originator`. Formed Lite without native provenance gets no invented native-base IDs.
Negative controls preserve explicit IDs, substantive content/decoration, Key isolation and dependencies.

## Evidence and limits

Historical validation covered 1,546 local leaves and 36 real Subscription cells (18 functional +
18 five-turn memory cases on gpt-5.5/Astra), with no real API-key credential. Execution snapshots
are recoverable from Git history; detailed local review records remain excluded from distribution.
Run the suites above for current results; the timeout investigation is summarized in
[Operations](../../../docs/OPERATIONS.md#diagnosing-long-requests).

Older 14-case OpenCode evidence proves delivery only; current 18-case coverage checks continuity.
Exports/live calls need separate opt-in and authorization. General protocol stress may force direct
functions; actual code-mode tests separately execute the native host. Core supplies no JavaScript
bridge to ordinary clients. No actual OpenCode WS producer, universal retry/retention contract,
all-model entitlement or exhaustive environment Cartesian coverage is claimed.
HTTP/1.1/WS shape equality and macOS ClientHello checks do not certify Linux TLS, negotiated HTTP/2
SETTINGS/HPACK, packet timing or remote classification. Detailed findings are in
[Codex compatibility](../../../docs/CODEX_COMPATIBILITY.md#field-order-and-presence).
