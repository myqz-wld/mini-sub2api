# mini-sub2api

`mini-sub2api` is a small Responses API gateway. Each downstream `ms2a_…` key maps to one Codex
subscription or OpenAI API-key credential, with request status, latency, and token totals recorded
per key. It exposes Responses through `POST /v1/responses` over HTTP/SSE and native sequential
Responses WebSockets through `GET /v1/responses`. Chat Completions, account pooling, quotas,
billing, dashboards, and administration HTTP APIs are out of scope.

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

The examples use `./state`; set `MINI_SUB2API_STATE_DIR` to omit `--state-dir` from each command.

### 1. Add an upstream credential

For a Codex subscription, sign in with the device flow:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal-subscription
```

Device flow is the default. Browser PKCE is available with `--flow browser` and uses Codex's
registered loopback ports `1455`, then `1457` when the first is occupied. For remote servers,
forward the printed loopback callback port over SSH.

To explicitly import an existing Codex login:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential import-codex --name personal-subscription \
  --auth-file ~/.codex/auth.json
```

Import copies the current access/identity snapshot, not the refresh token, so it does not alter the
original login but requires a new login near token expiry. Prefer `credential login codex` for
long-running deployments.

To use an OpenAI API key, read it from standard input:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential add-api-key codex --name openai-api --secret-stdin
```

Paste the key and finish with EOF. The secret is not placed in arguments, environment variables,
or SQLite.

Every new OAuth or upstream API-key credential defaults to `--fingerprint-mode device`. In this
mode, one credential represents one persistent Codex installation identity across HTTP and
WebSocket requests. Use `--fingerprint-mode off` on any of the three creation commands only when
caller-supplied installation identity must pass through unchanged. Existing credentials receive
the same `device` default when first opened by this version.

### 2. Create a downstream API key

```bash
build/bin/mini-sub2api --state-dir ./state credential list

build/bin/mini-sub2api --state-dir ./state \
  key create --credential cred_EXAMPLE --name laptop
```

The `ms2a_…` secret is displayed once; only its SHA-256 hash and a short prefix are retained. To
change its credential mapping, revoke the key and create another.

### 3. Start the service

```bash
build/bin/mini-sub2api --state-dir ./state serve
```

The default listener is `http://127.0.0.1:8787`. Request details are retained for seven days;
change this with `--usage-retention-days N`, or use `0` to disable automatic deletion.

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
supports_websockets = true
request_max_retries = 0
stream_max_retries = 0

[profiles.mini-sub2api]
model_provider = "mini-sub2api"
```

Then select the profile and supply only the downstream key:

```bash
MINI_SUB2API_API_KEY='ms2a_EXAMPLE' codex -p mini-sub2api
```

With `supports_websockets = true`, current Codex uses the Responses WebSocket v2 protocol and may
reuse one connection for sequential turns. API-key credentials establish the upstream socket before
the public upgrade. Subscription credentials accept the authenticated internal/public upgrade,
then use the first `response.create` model and service tier to construct the exact OAuth routing
handshake. A subscription-side rejection after that upgrade is a WebSocket close, not an HTTP
`426`; set the flag to `false` when client-side HTTP fallback is required.

Subscription routes apply the Codex CLI `0.149.0` request contract: unsupported top-level fields
are removed, `system` instruction messages become `developer` messages, request/tool/item JSON is
serialized in the pinned CLI order, and synthesized conversation items use UUIDv7 plus turn/create
metadata. Responses Lite uses the official `functions` namespace without inventing the opt-in
`tool_namespaces_info` field. OAuth HTTP pins streaming, JSON media types, level-3 zstd, `version`,
and the Codex User-Agent. The bundled model defaults, unknown-model fallback, output schema name,
OAuth authorize/refresh identity, and wire-critical dependency lock all follow `0.149.0`. Explicit
models, tools, instructions, reasoning controls, and WebSocket `generate`/continuation fields remain
authoritative. Remote compaction and sparse memory metadata retain their request kinds. OpenAI
API-key bodies and carrier-free WebSocket frames remain byte-exact; organization/project headers
are not sent on OAuth routes.

WebSocket policy is intentionally small and split across the coordinator and core:

- One socket is bound to one downstream key and credential; responses are sequential, and an
  overlapping `response.create` closes the connection with a policy violation.
- Other valid Responses v2 JSON application events pass through while a response is active.
- The core captures one credential fingerprint at the upstream handshake. It checks the current
  fingerprint revision before every `response.create`; after a mode change, an idle old socket
  closes once with reconnectable service-restart semantics before forwarding another create.
- A key may hold at most eight live sockets. The first application frame must arrive within 30
  seconds, an idle connection between turns closes after five minutes, each write is bounded to
  120 seconds, and application messages are limited to 16 MiB. There is no hard total lifetime.
- Codex's `generate=false` startup prewarm is retained in history as `websocket_prewarm` for
  in-flight/revocation safety, but it is excluded from daily inference aggregates.
- The public and provider hops support per-message deflate. The provider offer matches Codex
  `0.149.0` (`permessage-deflate; client_max_window_bits`); the authenticated loopback hop remains
  uncompressed, and payload semantics are unchanged.

## Administration

Credential commands:

```bash
build/bin/mini-sub2api --state-dir ./state credential list
build/bin/mini-sub2api --state-dir ./state credential fingerprint cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential disable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state \
  credential fingerprint cred_EXAMPLE --mode off
build/bin/mini-sub2api --state-dir ./state credential enable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential revoke cred_EXAMPLE --yes
build/bin/mini-sub2api --state-dir ./state credential remove cred_EXAMPLE --yes
```

- `disable` and `enable` are reversible service-side operations.
- `credential fingerprint ID` reports only the safe mode and revision. Changing the mode requires
  the credential to be disabled; the command waits for active HTTP/WebSocket operations, performs
  a fenced core update, and leaves it disabled for an explicit later `enable`. Switching modes does
  not rotate the saved device identity.
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

Daily per-key aggregates remain after request details expire. Use `--include-aggregates` to delete
them during `usage prune`, and global `--json` for machine-readable management output.

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
name. mini-sub2api does not manage certificates. A reverse proxy may terminate public TLS only when
it forwards to a deployment-local loopback listener, preserves streaming, and passes WebSocket
HTTP/1.1 Upgrade/Connection headers without buffering the upgraded connection.

Operational boundaries:

- Run only one coordinator/core pair per state directory.
- Stop the service before backing up or restoring the complete state directory.
- Provider secrets live in a private `0600` vault and are not encrypted at rest.
- Credential fingerprint sidecars are also private `0600` core-vault state. Their installation IDs
  are not copied into SQLite, the coordinator/core runtime protocol, usage records, logs, or CLI
  output. OAuth refresh and complete state-directory backup/restore preserve the ID; deleting and
  newly creating/importing a credential creates a new one.
- SQLite stores credential metadata, downstream-key hashes, timing, status, and token counts. It
  does not store prompts, request bodies, response bodies, tool arguments, or generated content.
- Every WebSocket `response.create` rechecks key and credential eligibility. Revoking a key or
  disabling a credential therefore prevents the next turn on an already-open idle socket.
- Inference is not replayed after transport errors, `429`, or `5xx`. OAuth may refresh once and
  replay once after a pre-response upstream `401`.

Upstream connection pools are isolated per credential. HTTP uses the platform transport-default
TLS backend; Responses WebSocket uses AWS-LC rustls with platform-native roots, matching the fixed
Codex `0.149.0` transport split, including its absent WebSocket ALPN offer and PQ-first key groups.
The provider WebSocket handshake and compression path use the same pinned OpenAI
`tokio-tungstenite`/`tungstenite` fork revisions as that release. These profiles are internal and
are not user-configurable, and every new provider WebSocket gets fresh TLS session state.
Subscription requests also pin Codex's `version` header to `0.149.0`;
regular OpenAI API-key routes preserve a reviewed client-supplied value. Reviewed
`x-openai-subagent` identity crosses both HTTP and WebSocket routes, including Codex's conditional
subagent handshake order. Managed residency and runtime-timing headers retain their captured
conditional order; reviewed attestation, inference-call, and memory-generation names cross the Go
filters. OAuth HTTP clients share only the official allowlist of Cloudflare infrastructure cookies;
account, session, and arbitrary application cookies are never retained.

Compatibility is exact for request state that a `0.149.0` client supplies or the gateway can derive.
The gateway does not fabricate absent caller workspace, sandbox, thread-source, trace, attestation,
or tool-inventory state, and it preserves explicit public Responses controls. Its provider HTTP
clients also refuse redirects so credentials cannot be replayed to a redirected authority; this is
an intentional security boundary from the stock CLI client.

OAuth issuer/client and upstream URL overrides are available for controlled compatibility testing.
Plain HTTP overrides are accepted only for literal loopback IPs.
