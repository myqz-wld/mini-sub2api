# Operations

[Setup and usage](../README.md) · [Gateway behavior](BEHAVIOR.md)

## Authentication options

Device login is the default for a long-running Subscription. Browser PKCE is available with
`credential login codex --name personal --flow browser`; on a remote host, forward the printed
loopback callback port over SSH. To copy the current Codex login without its refresh token:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential import-codex --name personal --auth-file ~/.codex/auth.json
```

Set `MINI_SUB2API_STATE_DIR` to omit `--state-dir`. Request details default to seven days;
`serve --usage-retention-days N` changes this, and `0` disables automatic detail pruning.
Daily aggregates are retained separately.

## Administration


Shutdown waits for the owned Core process to exit and for both WS pumps to finish, including
terminal usage writes, before releasing the corresponding session and storage resources.

```bash
# Credentials
build/bin/mini-sub2api --state-dir ./state credential list
build/bin/mini-sub2api --state-dir ./state credential fingerprint cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential disable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential enable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential revoke cred_EXAMPLE --yes
build/bin/mini-sub2api --state-dir ./state credential remove cred_EXAMPLE --yes

# Downstream keys
build/bin/mini-sub2api --state-dir ./state key list
build/bin/mini-sub2api --state-dir ./state key revoke key_EXAMPLE --yes

# Usage
build/bin/mini-sub2api --state-dir ./state \
  usage history --key key_EXAMPLE --limit 100
build/bin/mini-sub2api --state-dir ./state \
  usage stats --key key_EXAMPLE --since 2026-08-01 --until 2026-08-31
build/bin/mini-sub2api --state-dir ./state \
  usage prune --before 2026-08-01 --yes
```

Changing fingerprint mode requires a disabled credential:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential fingerprint cred_EXAMPLE --mode off
```

`revoke` revokes OAuth upstream before local deletion. `remove` deletes service-side material;
forcing OAuth removal without upstream revocation requires `--force-service-only --yes`.

## Deployment and security

Plain HTTP may bind only to loopback. A non-loopback listener requires a certificate and private key:

```bash
build/bin/mini-sub2api --state-dir ./state serve \
  --listen 192.0.2.20:8787 \
  --tls-cert ./server.crt \
  --tls-key ./server.key
```

A reverse proxy may terminate TLS when it forwards to a deployment-local loopback listener,
preserves streaming, and supports WebSocket Upgrade without buffering.

Operational boundaries:

- Run one coordinator/core pair per state directory; the service is node-local and not active-active.
- Stop the service before backing up or restoring the complete state directory.
- Vault and identity files use private permissions but are not encrypted at rest.
- Request/response bodies, content, tool arguments, workspaces, and credentials are not persisted in
  identity state. Only bounded schema-recognized ID pairs are retained for reversible translation.
- Local request history may retain one visible-ASCII provider request ID for seven days by default;
  it is never exposed through the public Responses API.
- Provider HTTP clients refuse redirects. Plain HTTP test overrides are accepted only for literal
  loopback IPs.
- Credential deletion remains available when request state is corrupt; the final owner removes the
  shared state file. Remove/revoke rechecks disabled, key, and in-flight state under one mutation
  fence before core material is irreversibly removed.
