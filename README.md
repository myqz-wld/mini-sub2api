# mini-sub2api

`mini-sub2api` is a small Responses API gateway. Each downstream `ms2a_…` key maps to one Codex
subscription or OpenAI API-key credential. It supports:

- `POST /v1/responses` over JSON or SSE
- `GET /v1/responses` over sequential Responses WebSockets
- per-key request status, latency, and token usage

Chat Completions, account pooling, quotas, billing, dashboards, and administration HTTP APIs are
out of scope.

## Build

Go 1.26.4 and Rust 1.96.0 are pinned through `mise`:

```bash
mise install
bash scripts/build.sh
```

Keep the generated package together:

- `build/bin/mini-sub2api`
- `build/bin/mini-sub2api-core-codex`
- `build/bin/build-info.json`

```bash
build/bin/mini-sub2api --version
build/bin/mini-sub2api --check-installed
```

`--check-installed` returns JSON and never fetches a remote repository.

## Quick start

These examples use `./state`. Set `MINI_SUB2API_STATE_DIR` to omit `--state-dir`.

### 1. Add a credential

For a long-running Codex subscription, use the device flow:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal-subscription
```

Browser PKCE is available with `--flow browser`; on a remote host, forward the printed loopback
callback port over SSH. To copy a current Codex login without its refresh token:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential import-codex --name personal-subscription \
  --auth-file ~/.codex/auth.json
```

For an OpenAI API key:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential add-api-key codex --name openai-api --secret-stdin
```

Secrets are read from standard input and are not stored in arguments, environment variables, or
SQLite.

### 2. Create a downstream key

```bash
build/bin/mini-sub2api --state-dir ./state credential list
build/bin/mini-sub2api --state-dir ./state \
  key create --credential cred_EXAMPLE --name laptop
```

The `ms2a_…` value is shown once; only its SHA-256 hash and short prefix are retained.

### 3. Start and call the service

```bash
build/bin/mini-sub2api --state-dir ./state serve

curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H "Authorization: Bearer ms2a_EXAMPLE" \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

The default listener is `127.0.0.1:8787`. Request details are retained for seven days; change this
with `--usage-retention-days N`, or use `0` to disable automatic deletion.

## Codex configuration

Add a custom provider to `~/.codex/config.toml`:

```toml
[model_providers.mini-sub2api]
name = "mini-sub2api"
base_url = "http://127.0.0.1:8787/v1"
env_key = "MINI_SUB2API_API_KEY"
wire_api = "responses"
supports_websockets = true
request_max_retries = 0
stream_max_retries = 0

[profiles.mini-sub2api]
model_provider = "mini-sub2api"
```

```bash
MINI_SUB2API_API_KEY='ms2a_EXAMPLE' codex -p mini-sub2api
```

Set `supports_websockets = false` when HTTP-only behavior is required.

## Behavior

### Routing profiles

A valid non-empty `Originator` header marks a Codex caller. It selects request formatting only and
never changes authentication or credential visibility.

| Caller | Credential | Upstream behavior |
|---|---|---|
| No `Originator` | OpenAI API key | `BareOpenAi`: reviewed headers; HTTP bodies and valid WS text frames remain byte-exact. |
| Codex | OpenAI API key | `CodexOpenAi149`: Codex 0.149.0 request shape; no HTTP zstd. |
| Any | Codex subscription | `CodexSubscription149`: Codex 0.149.0 request shape; OAuth HTTP uses zstd level 3. |

### Compatibility and state

- Codex profiles pin the 0.149.0 user agent, `originator`, `version`, base prompt, supported
  request fields, and model defaults. Unknown structured members are removed; documented
  schemas and free-form payloads stay opaque.
- Emulated HTTP always uses `store:false`, `stream:true`, and SSE upstream. A non-streaming caller
  receives the terminal Responses object, bounded to 64 MiB. Supported downstream zstd is decoded
  before the caller's original streaming preference is evaluated.
- Both Codex profiles persist a UUIDv4 installation identity plus UUIDv7 conversation, thread, and
  turn identities, and translate schema-recognized lifecycle IDs in both directions. API-key state
  is isolated by local credential; duplicate OAuth credentials share their ChatGPT-account state.
- Request state remains schema v1 with no historical compatibility branch. Each private file is
  limited to 16 MiB; completed detail becomes pruneable after 30 days. Compaction advances its
  window only after the matching `response.completed` is persisted.
- Historical response/conversation and control/item/call/approval references must resolve an
  existing reversible mapping. A missing required reference fails locally as `state_unavailable`
  before upstream delivery; new sessions, stream IDs, and request-local definitions still allocate
  fresh pseudonyms.
- Sandbox permission meaning is preserved, while `seatbelt`, `seccomp`, or
  `windows_sandbox` is derived from the gateway OS. Caller `workspaces` values remain unchanged.
- Provider response headers are default-denied. Public provider request-ID headers contain only the
  gateway `req_*` alias; one bounded raw provider ID may appear only in local request history and is
  pruned with that detail. SQLite migrates schema 2 to 3 for this nullable field.
- Stateful Codex non-2xx bodies are replaced with bounded gateway errors; `BareOpenAi` keeps its
  reviewed response body bytes. Unknown body fields, content, tool arguments, and output stay opaque.
- WebSocket turns are sequential. Each key may hold eight sockets; first-frame, idle, write, and
  message limits are 30 seconds, 5 minutes, 120 seconds, and 16 MiB respectively.
- Inference is never replayed after ordinary transport, `429`, or `5xx` failures. OAuth may refresh
  and replay once after a pre-response upstream `401`. Delivery failures expose
  `retryAdvice`, `phase`, and `deliveryState`; `ambiguous` must not be retried automatically.
- HTTP `response.failed`, `response.incomplete`, and `error` terminals remain valid Responses
  output but are recorded as upstream errors in request history and daily statistics.

See [the v1 protocol reference](src/protocol/v1/README.md) for the complete HTTP, SSE, WebSocket,
identity, and failure contracts.

## Administration

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

## Validation

```bash
mise exec -- go test ./src/coordinator/...
bash scripts/test.sh
bash scripts/build.sh
```

The direct Go integration suite builds the current debug core when no explicit test binary is set;
it never silently skips cross-language coverage.

## Disclaimer

This project is for personal learning and research. It is not an official OpenAI product and is not
intended for commercial or production use. Users are responsible for applicable laws and service
terms.
