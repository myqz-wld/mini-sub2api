# Operations

[Setup](../README.md) · [Behavior and limits](BEHAVIOR.md)

## Authentication

Device login is preferred remotely. Browser PKCE uses `credential login codex --flow browser`;
forward its printed loopback callback port over SSH. Importing excludes the existing refresh token:

```bash
export MINI_SUB2API_STATE_DIR=./state
build/bin/mini-sub2api credential import-codex --name personal --auth-file "${CODEX_HOME:-$HOME/.codex}/auth.json"
```

Core serializes refresh. Vault files are private, but not encrypted at rest.

## Administration

```bash
build/bin/mini-sub2api credential list
build/bin/mini-sub2api credential disable cred_EXAMPLE
build/bin/mini-sub2api credential fingerprint cred_EXAMPLE --mode off
build/bin/mini-sub2api credential enable cred_EXAMPLE
build/bin/mini-sub2api credential revoke cred_EXAMPLE --yes
build/bin/mini-sub2api key list
build/bin/mini-sub2api key revoke key_EXAMPLE --yes
build/bin/mini-sub2api usage history --key key_EXAMPLE --limit 100
build/bin/mini-sub2api usage stats --key key_EXAMPLE --since 2026-08-01 --until 2026-08-31
build/bin/mini-sub2api usage prune --before 2026-08-01 --yes
```

Mode changes require disabled/drained credentials. `revoke` revokes OAuth before deletion;
`remove` deletes local material, with `--force-service-only --yes` required to skip revocation.
Deletion rechecks Keys/in-flight work and removes shared identity state after its final owner,
including corrupt state.

Usage belongs to the Key. Details default to seven days; `serve --usage-retention-days N` changes this,
and `0` disables automatic pruning. Daily aggregates survive. A bounded provider request ID may be
retained in private diagnostics, never exposed publicly.

## Diagnosing long requests

Start with the public `X-Mini-Sub2Api-Request-Id`; it joins coordinator and Core logs:

```bash
sudo journalctl -u mini-sub2api.service --since '30 minutes ago' -o cat --no-pager | rg -F 'req_EXAMPLE'
```

| Evidence | Meaning |
|---|---|
| `http_request_started`, `http_forward_started` | Accepted request, then buffered body forwarded; `request_bytes` and `body_tag` help recognize repeated attempts. |
| `core_http_progress` | Waiting phase: credential lock/refresh, normalization or upstream headers. |
| `upstream_headers`, `upstream_auth_retry` | HTTP status, attempt 1/2 and time since the initial send; retry 2 is the existing OAuth 401 refresh path. Subscription span fields show allowlisted model/effort before and after defaults. API-key passthrough does not parse extra metadata. |
| `core_http_response` | Response constructed or failed; `response_ready` only means handoff, not inference completion. |
| `http_sse_progress`, `http_stream_progress` | Core/Go body bytes, event classes and first/last byte/event/output times. Core also reports byte/output idle time. |
| `transport_error`, `request_failure` | Layer/phase, typed I/O/HTTP/TLS category and available OS/TLS/HTTP2 numeric codes. HTTP2 includes reset/GOAWAY and remote flags. Error-source traversal stops at 16 links. |
| `http_sse_observation` | Core reader end, processing stage, terminal kind and allowlisted provider error code. A received terminal still requires validation. |
| `http_stream_finished`, `request_finished` | Go stop reason/outcome, downstream bytes/write time, duration and history-save result; Core failure trailers are logged separately. |

`event_idle_timeout`, `first_output_timeout`, `output_idle_timeout` and `downstream_write_timeout`
identify local limits; `missing_terminal` distinguishes EOF without completion. `core_http_canceled`
means the handler was dropped before response construction; correlate the Go outcome to identify
caller cancellation. `read_end=reader_dropped` alone cannot distinguish cancellation from validation
failure: check `processing_stage`, transport errors and the final outcome. `terminal_tail_closed`
can complete normally after validation. See [timeout limits](BEHAVIOR.md#completion-and-recovery).

Timings are milliseconds: stream times start at body observation; total duration includes all stages
and TTFB measures headers. `-1` means unobserved, `none` means no recorded category, and `other` means
unclassified. Text/reasoning counts prove activity, not useful progress or completion. A reset/TLS
category cannot establish a provider's internal policy; if lower-level evidence is unavailable, the
cause remains unknown. Logs contain no raw errors, URLs, payloads, event names, Keys or provider IDs.

`body_tag` is a 96-bit HMAC over the existing body, scoped to the downstream Key and random process
instance. Equal tags within one `instance` suggest byte-identical input, not proof of automatic retry.
Different Keys/restarts do not correlate; entropy failure disables the tag. Protect logs as operational
metadata. Normal requests produce a fixed number of lifecycle lines; each active progress phase/layer
emits at most once per minute. Core wakes its existing read/wait loop; Go reports on forwarded chunks.
There are no new per-request background tasks, body copies, caches or per-event logs. Correlation
scans input once with constant extra memory. Keep journal rotation/size limits appropriate to request
volume; the application adds no separate log files or retention store.

The 2026-09-21 investigation found an 11.4-hour request with headers after 741 ms, no recorded token
usage and a final client-disconnected status. Loopback tests reproduced heartbeat-only and
completed-but-open responses waiting for transport EOF before the lifetime repair. This established
the missing bounds; retained production metadata cannot identify the exact upstream event sequence.
An in-progress usage row alone does not prove Core still holds an execution lane or memory reservation.
The [HTTP lifetime tests](../src/coordinator/integration/responses_http_lifetime_test.go)
cover the repaired behavior through the actual Go/Core loopback path.

## Deployment

Ship both binaries and build-info.json together. Plain HTTP binds only loopback; other listeners need TLS:

```bash
build/bin/mini-sub2api serve --listen 192.0.2.20:8787 --tls-cert ./server.crt --tls-key ./server.key
```

An optional TLS proxy forwards to loopback and preserves streaming/WS upgrades. Use one service
per state directory; stop it before copying/restoring state. Shutdown joins Core/WS work and usage writes.

Size small hosts before load: the default 2 GiB context budget is not an RSS cap.
[Memory](MEMORY.md) provides a starting profile and read-only diagnostics.

Bodies and raw Keys are not persisted. Identity files keep bounded typed ID pairs; protect them like
the vault. Provider clients reject redirects; plain-HTTP test overrides accept literal loopback IPs only.
