# Operations

[Setup](../README.md) · [Behavior and limits](BEHAVIOR.md)

## Authentication

Device login is preferred remotely. Browser PKCE uses `credential login codex --flow browser`;
forward its printed loopback callback port over SSH. Importing excludes the existing refresh token:

```bash
export MINI_SUB2API_STATE_DIR=./state
build/bin/mini-sub2api credential import-codex --name personal --auth-file ~/.codex/auth.json
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

Mode changes require disabled/drained credentials. `revoke` revokes OAuth upstream before deletion;
`remove` deletes local service material. Forced removal without revocation requires
`--force-service-only --yes`. Deletion rechecks keys/in-flight work, remains available for corrupt
identity state and removes shared state after its final owner.

Usage belongs to the Key. Details default to seven days; `serve --usage-retention-days N` changes this,
and `0` disables automatic pruning. Daily aggregates survive. A bounded provider request ID may be
retained in private diagnostics, never exposed publicly.

## Deployment

Ship both binaries and build-info.json together. Plain HTTP binds only loopback; other listeners need TLS:

```bash
build/bin/mini-sub2api serve --listen 192.0.2.20:8787 --tls-cert ./server.crt --tls-key ./server.key
```

An optional TLS proxy must forward to local loopback and preserve streaming/WS upgrades. Run one
service per state directory; no active-active operation. Stop it before copying/restoring state.
Shutdown joins Core and both WS pumps, including terminal usage writes.

Bodies and raw Keys are not persisted. Identity files keep bounded typed ID pairs; protect them like
the vault. Provider clients reject redirects; plain-HTTP test overrides accept literal loopback IPs only.
