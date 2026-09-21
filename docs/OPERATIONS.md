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

HTTP duration covers the request lifecycle, not model computation alone; TTFB measures headers.
Stream logs pair the gateway request ID with fixed reasons such as `event_idle_timeout`,
`terminal_tail_closed`, `incomplete_terminal_tail` or `downstream_write_timeout`, without payloads.
An idle/truncated upstream is an error; a validated terminal tail can complete normally; client
cancellation or stalled writes record disconnection. See [timeout limits](BEHAVIOR.md#completion-and-recovery).

The 2026-09-21 investigation found an 11.4-hour request with headers after 741 ms, no recorded token
usage and a final client-disconnected status. Loopback tests reproduced heartbeat-only and
completed-but-open responses waiting for transport EOF before the lifetime repair. This established
the missing bounds; retained production metadata cannot identify the exact upstream event sequence.
An in-progress usage row alone does not prove Core still holds an execution lane or memory reservation.
Correlate gateway request IDs, fixed termination reasons and proxy cancellation times without storing
traffic bodies. The [HTTP lifetime tests](../src/coordinator/integration/responses_http_lifetime_test.go)
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
