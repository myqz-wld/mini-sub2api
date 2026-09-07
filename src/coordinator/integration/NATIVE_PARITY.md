# Capture tests

[Setup](../../../README.md) · [Behavior](../../../docs/BEHAVIOR.md)

## Run and prerequisites

```bash
bash scripts/test.sh
bash scripts/test-native-parity.sh
mise exec -- python3 scripts/prepare-opencode-tests.py
bash scripts/test-scaffold-parity.sh
```

Codex **0.153.4** uses source `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`;
OpenCode **1.18.29** uses `16747470f976aca3d362ad730bcd3fe82ecc2c9a`.
Missing/wrong prerequisites fail, not skip. Client executables are verified on macOS arm64.
Standard suites need no provider account and never fall back to real endpoints.

## Method

- Run actual Codex app-server/OpenCode serve in isolated home/config/auth/project state, with synthetic
  credentials, loopback responders and blocked external networking. OpenCode uses in-memory SQLite,
  no snapshots and a null log sink. Cleanup joins only owned children.
- Compare direct traffic with client → public Go gateway → supervised Rust Core. API-key bodies are
  byte-exact; Subscription checks ordered content/settings and typed identity relationships.
- Tap HTTP and WS handshakes/frames before server decoding. Independently parse masking, fragmentation,
  control frames and permessage-deflate. Limits: 64 connections/tap, 64 MiB/connection, 128 application
  requests and 64 MiB/application capture. Raw bodies stay in memory; failures report structure only.
- Verify Lite UUIDv5 against exact transmitted tools/base bytes. Do not recursively strip IDs or
  rewrite opaque arguments, ciphertext or resource references in the comparison oracle.
- TLS probes stop after ClientHello, before credentials: compare stable ciphers/extensions/groups/
  signatures/versions/ALPN, excluding random bytes and key material. This does not certify negotiation
  or remote fingerprint classification.

## Coverage

| Dimension | Assertions |
|---|---|
| Codex HTTP/WS, both credentials, ordinary/Lite | Full requests, prewarm/increments, tool loops, next turn, no added business inference |
| Bare explicit references | 54 transport/format/identity cases: HTTP reconstruction, WS remote continuation and missing-state errors |
| Bare anonymous history | 96 cases, three calls each: both credentials × JSON/SSE/WS-reconnect × ordinary/converted/formed/forced Lite × text/tool × exact/reduced history |
| Bare minimal history | 24 cases start with model/input only, without instructions/tools/IDs; ordinary/Lite × transports × exact/reduced history × credentials |
| Caller-only bases | 144 standard HTTP/WS cases, two business requests each: credentials × caller markers × ordinary/Lite × missing/invalid/explicit bases; no inserted defaults, stable prefix ownership and API-key bytes |
| Actual OpenCode | 18 custom-provider text/read/denied-file and built-in OpenAI-plugin cases: stable session/thread, user/tool turns, headers and API-key bytes |
| Model/context | All 11 catalogs; caller bases/personality, ordered developer messages, AGENTS/Skills/permissions, CWD, detected time/shell and supplied timezone text |
| Identity/privacy | Key isolation, device modes/shared account, branches/forks/history ownership, explicit conflicts, restart/corruption and required references |
| Cache/completion | Publication ordering, item/footer reconciliation, failed/incomplete responses, expiry/pressure, exact interning and tool consumption |
| WS/compaction | Socket reuse/reconnect, first routing token, hidden prewarm, uncertain-send fences, V2 item-done proof and window commits |
| Compaction continuation | 36 credential/transport/format cases: explicit and in-band windows, two HTTP deltas, full WS recovery, caller Lite setup, unavailable-window errors and exact API-key packets |
| Tools | Actual native nested host callback; bare/OpenCode direct calls; schema order/duplicates and deterministic prefixes |

Anonymous bare WS reconnects before every submission, so socket binding cannot mask a failed match.
Custom OpenCode asserts no recognized session/reference carrier; built-in tests enable its isolated
plugin and check actual session-id/originator. Formed Lite instructions without native provenance
must not receive invented native-base IDs. Core negative controls preserve explicit IDs, substantive
content, nonempty decoration, Key isolation and dependency validation.

## Evidence and limits

- [Current repair](../../../ref/reviews/recent-3-days/REVIEW_38_history-prefix-compatibility.md): failures, fixes and current results.
- [Historical 128-case excerpts](../../../ref/architecture/plan-17/capture-index.md) and [audit](../../../ref/architecture/plan-17/source-audit.md).

The earlier 14 OpenCode cases proved functional delivery, not cross-request session continuity.
Historical sanitized excerpts are reencoded, not byte-exact replay files; in-memory byte/UUID checks
precede export. Export and real-provider runs require separate explicit authorization and opt-in.
This repair makes no live calls. Prior live evidence covers 18 text/tool/schema and 18 five-turn
memory cells on gpt-5.5/Astra; no real API-key credential was available.

General responders sometimes force direct functions for protocol stress. Actual code-mode tests
separately execute the native nested host; host availability and tool exposure differ. Core provides
no JavaScript bridge for ordinary clients. No actual OpenCode WS producer, universal server retry/
retention contract, all-model entitlement or complete environment Cartesian coverage is claimed.
Successive suite counts overlap and must not be added as independent cases.
