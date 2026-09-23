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
| Actual OpenCode: 18 cases | Custom-provider text/read/denied-file and built-in OpenAI plugin; continuity, tool turns, API-key bytes and ordered Subscription wire/header contracts |
| Bare ordered wire: 12 cases | HTTP/WS × ordinary/Lite × minimal/explicit/null controls; independent actual CLI baselines, source-derived protocol order, complete header signatures |
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

The 0.156.0 continuation follow-up passed 26 real Subscription cases: 12 native direct/gateway/
captured-gateway conversations, 8 ordinary five-turn memory cases, 4 tool/schema cases and 2 tool
reconnect cases. All use gpt-5.5/Astra. Four captured-gateway cases additionally compare 18 paired
real-provider requests with the recursive ordered/scalar and complete header-shape assertions.
Long-token HTTP/WS/prewarm/capacity regressions and relay security checks run without providers.
See [the findings and connection limits](../../../docs/CODEX_COMPATIBILITY.md#real-upstream-continuation-evidence).

Historical validation covered 1,546 local leaves and 36 real Subscription cells (18 functional +
18 five-turn memory cases on gpt-5.5/Astra), with no real API-key credential. Execution snapshots
are recoverable from Git history; detailed local review records remain excluded from distribution.
Run the suites above for current results; the timeout investigation is summarized in
[Operations](../../../docs/OPERATIONS.md#diagnosing-long-requests).

Older 14-case OpenCode evidence proves delivery only; current 18-case coverage checks continuity.
Current OpenCode/ordinary ordered checks use `caller_wire_contract_test.go` and independently run
the pinned CLI through `caller_wire_capture_test.go`. Thus the OpenCode suite also requires that
CLI and source checkout. Live OpenCode and bare-memory checks now capture Core egress through the
authenticated bounded relay and apply the same wire contract. Bare live fixtures use one relay per
five-turn case so each stays within the relay's 12-request budget, including hidden setup.
`TestOpenCodeAnonymousFullHistoryAfterCoreRestart` keeps the actual custom-provider session across
a fixture Core restart and checks both full replay and item-turn field presence. The tested 1.18.29
client omitted those fields despite the synthetic provider emitting them. Independent HTTP/SSE/WS
gateway cases retain them explicitly to exercise the guarded historical-turn import itself.
Exports/live calls need separate opt-in and authorization. General protocol stress may force direct
functions; actual code-mode tests separately execute the native host. Core supplies no JavaScript
bridge to ordinary clients. No actual OpenCode WS producer, universal retry/retention contract,
all-model entitlement or exhaustive environment Cartesian coverage is claimed.
HTTP/1.1/WS shape equality and macOS ClientHello checks do not certify Linux TLS, negotiated HTTP/2
SETTINGS/HPACK, packet timing or remote classification. Detailed findings are in
[Codex compatibility](../../../docs/CODEX_COMPATIBILITY.md#field-order-and-presence).

## Opt-in live continuation run

After explicit authorization for real Subscription usage, supply the pinned CLI/host and an
existing unexpired login (default `$HOME/.codex/auth.json`, or `MINI_SUB2API_LIVE_AUTH_FILE`). The
fixture imports only access credentials into disposable state, verifies the original login remains
unchanged and never prints payloads or tokens. It does not renew credentials. Native direct workspaces
use neutral temporary paths; default test launchers remain loopback-only with no provider fallback.

```bash
MINI_SUB2API_LIVE_SUBSCRIPTION=1 \
MINI_SUB2API_NATIVE_CODEX_BINARY="$PWD/.ref/tools/codex-v0.156.0/codex" \
  mise exec -- go test -tags=nativeparity,liveparity -race -count=1 -timeout=15m \
  ./src/coordinator/integration \
  -run '^TestLiveSubscription(Conversation|MemoryBare|ToolAndSchema|ToolReconnect)$' -v
```

The authenticated capture relay targets only the fixed official backend; it keeps raw frames and
bodies in memory and bounds requests, capture sizes and lifetimes. Its Go TLS/HTTP/1.1 connection is
not a native TLS fingerprint oracle. Relay rejection and byte-preservation checks need no live login:

```bash
mise exec -- go test -tags=nativeparity,liveparity -race \
  ./src/coordinator/integration -run '^TestLiveRelay' -v
```
