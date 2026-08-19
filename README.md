# mini-sub2api

`mini-sub2api` is a small Responses API gateway. Each downstream `ms2a_…` API key maps to one
Codex subscription or OpenAI API-key credential, while request status, latency, and token totals
are recorded per downstream key.

The public API is limited to `POST /v1/responses` over HTTP/SSE. WebSockets, Chat Completions,
account pooling, quotas, billing, dashboards, and administration HTTP APIs are intentionally out
of scope.

## Build

The project supports macOS and Linux and pins Go and Rust through `mise`:

```bash
mise install
bash scripts/build.sh
```

Keep these generated files together when installing or copying the service:

- `build/bin/mini-sub2api`
- `build/bin/mini-sub2api-core-codex`
- `build/bin/build-info.json`

Check an installation with:

```bash
build/bin/mini-sub2api --version
build/bin/mini-sub2api --check-installed
```

`--check-installed` returns JSON and never fetches a remote repository.

## Quick start

The examples below use `./state` as an isolated state directory. You can instead set
`MINI_SUB2API_STATE_DIR` once for all commands.

### 1. Add an upstream credential

For a Codex subscription, sign in with the device flow:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal-subscription
```

The device flow is the default and works well on remote or headless servers. Browser PKCE is
available with `--flow browser`; on a remote server, forward the printed loopback callback port
over SSH before opening the authorization URL.

To explicitly import an existing Codex login:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential import-codex --name personal-subscription \
  --auth-file ~/.codex/auth.json
```

Importing copies only the current access/identity snapshot, not the refresh token. It does not
alter the original Codex login, but the imported credential will require a new login shortly
before the copied access token expires. Use `credential login codex` for long-running deployments.

To use an OpenAI API key, read it from standard input:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential add-api-key codex --name openai-api --secret-stdin
```

Paste the key and finish with EOF. The secret is not placed in command arguments, environment
variables, or SQLite.

### 2. Create a downstream API key

```bash
build/bin/mini-sub2api --state-dir ./state credential list

build/bin/mini-sub2api --state-dir ./state \
  key create --credential cred_EXAMPLE --name laptop
```

The generated `ms2a_…` secret is displayed once. Only its SHA-256 hash and a short display prefix
are retained. A downstream key is permanently mapped to its credential; revoke it and create a new
one to change that mapping.

### 3. Start the service

```bash
build/bin/mini-sub2api --state-dir ./state serve
```

The default listener is `http://127.0.0.1:8787`. Request details are retained for seven days by
default; change this with `--usage-retention-days N`, or use `0` to disable automatic detail
deletion.

### 4. Send a request

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H "Authorization: Bearer ms2a_EXAMPLE" \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

Responses include `X-Mini-Sub2Api-Request-Id` and upstream time-to-first-byte in `Server-Timing`.
JSON and SSE response bytes are otherwise preserved.

## Use from Codex

Add a custom Responses provider to `~/.codex/config.toml`:

```toml
[model_providers.mini-sub2api]
name = "mini-sub2api"
base_url = "http://127.0.0.1:8787/v1"
env_key = "MINI_SUB2API_API_KEY"
wire_api = "responses"
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0

[profiles.mini-sub2api]
model_provider = "mini-sub2api"
```

Then select the profile and supply only the downstream key:

```bash
MINI_SUB2API_API_KEY='ms2a_EXAMPLE' codex -p mini-sub2api
```

Keep `supports_websockets = false`; mini-sub2api supports HTTP/SSE only. Subscription-backed plain
Responses requests are normalized for current Codex model behavior while preserving explicit
models, tools, instructions, reasoning controls, and streaming choices.

## Administration

Credential commands:

```bash
build/bin/mini-sub2api --state-dir ./state credential list
build/bin/mini-sub2api --state-dir ./state credential disable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential enable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential revoke cred_EXAMPLE --yes
build/bin/mini-sub2api --state-dir ./state credential remove cred_EXAMPLE --yes
```

- `disable` and `enable` are reversible service-side operations.
- `revoke` is for OAuth credentials. It requires no active downstream keys, waits for in-flight
  requests, revokes upstream first, and removes local material only after success.
- `remove` deletes service-side material. OpenAI API-key deletion at the provider remains the
  operator's responsibility. Removing OAuth without upstream revocation requires
  `--force-service-only --yes`.

Downstream key commands:

```bash
build/bin/mini-sub2api --state-dir ./state key list
build/bin/mini-sub2api --state-dir ./state key revoke key_EXAMPLE --yes
```

Usage commands:

```bash
build/bin/mini-sub2api --state-dir ./state \
  usage history --key key_EXAMPLE --limit 100
build/bin/mini-sub2api --state-dir ./state \
  usage stats --key key_EXAMPLE --since 2026-08-01 --until 2026-08-31
build/bin/mini-sub2api --state-dir ./state \
  usage prune --before 2026-08-01 --yes
```

Daily per-key aggregates remain after request details expire. Add `--include-aggregates` to
`usage prune` only when those aggregates should also be permanently deleted. Add global `--json`
to management commands for machine-readable output.

## Deployment and security

Plain HTTP can bind only to IPv4 or IPv6 loopback. Every non-loopback listener requires both a TLS
certificate and private key:

```bash
build/bin/mini-sub2api --state-dir ./state serve \
  --listen 192.0.2.20:8787 \
  --tls-cert ./server.crt \
  --tls-key ./server.key
```

For direct IP HTTPS, the certificate must contain that IP as an `iPAddress` subject alternative
name. mini-sub2api does not issue or renew certificates. A reverse proxy may terminate public TLS
only when it forwards to a deployment-local loopback listener and preserves streaming.

Operational boundaries:

- Run only one coordinator/core pair per state directory.
- Stop the service before backing up or restoring the complete state directory.
- Provider secrets live in a private `0600` vault and are not encrypted at rest.
- SQLite stores credential metadata, downstream-key hashes, timing, status, and token counts. It
  does not store prompts, request bodies, response bodies, tool arguments, or generated content.
- Inference is not replayed after transport errors, `429`, or `5xx`. OAuth may refresh once and
  replay once after a pre-response upstream `401`.

OAuth issuer/client and upstream URL overrides are available for controlled compatibility testing.
Plain HTTP overrides are accepted only for literal loopback IPs.
