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

Every credential defaults to `--fingerprint-mode device`: subscription installation identity
converges per ChatGPT account, while `off` keeps one-to-one pseudonyms. Other subscription IDs are
pseudonymized in both modes; OpenAI API-key payloads remain transparent.

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

With `supports_websockets = true`, Codex may reuse one Responses v2 socket for sequential turns.
Subscription routing waits for the first `response.create`; a later rejection is therefore a
WebSocket close rather than HTTP `426`. Set the flag to `false` if HTTP fallback is required.

## Source-aware Responses profiles

The gateway independently classifies the selected credential and the caller source. A syntactically
valid, non-empty `Originator` header marks a Codex caller; its value is not fixed. This marker
selects request formatting only—it never changes credential visibility, account permissions, or
which downstream key is accepted.

| Caller source | Credential | Upstream profile |
| --- | --- | --- |
| No valid `Originator` | OpenAI API key | `BareOpenAi`: reviewed headers only; HTTP body and WebSocket text frame bytes remain exact. |
| Valid `Originator` | OpenAI API key | `CodexOpenAi149`: Codex `0.149.0` Responses emulation with OpenAI API-key authentication. HTTP is not zstd-compressed. |
| Any source | Codex subscription | `CodexSubscription149`: Codex `0.149.0` Responses emulation with ChatGPT subscription authentication. HTTP uses zstd level 3; WebSocket application messages never use zstd. |

The two Codex emulation profiles begin with the caller's complete Responses object and make only
the required `0.149.0` overlays. Explicit official fields remain authoritative, including
`previous_response_id`, `conversation`, `background`, context/limits, metadata, moderation,
prompt/cache settings, safety identifiers, sampling, truncation, tools, and image detail. The
supported surface is the union of documented OpenAI Responses fields and completed Codex `0.149.0`
capture/source wire fields; other protocol fields are removed. Arbitrary keys remain valid inside
documented opaque/free-form containers such as metadata maps, JSON Schema, `prompt.variables`, and
function/custom tool-call argument/input/output payload values; structured tools and shell/computer
outputs are still filtered.
HTTP does not synthesize `previous_response_id`; it forwards an explicit value unchanged. Non-Lite
requests default an omitted image detail to `high`; Responses Lite leaves omitted detail absent but
preserves an explicitly supplied supported detail value.

Every subscription request uses the fixed identity
`codex_cli_rs/0.149.0 (Ubuntu 22.4.0; x86_64) xterm-256color`,
`originator: codex_cli_rs`, and `version: 0.149.0`. Request IDs are mapped to UUIDv8 with a
stateless HMAC over ChatGPT account, downstream key scope, field, and source value; `device` then
converges installation per account. The mapping is host-independent and the internal scope never
crosses upstream.

WebSocket policy is intentionally small and split across the coordinator and core:

- One socket is bound to one downstream key and credential; responses are sequential, and an
  overlapping `response.create` closes the connection with a policy violation.
- Other valid Responses v2 JSON application events pass through while a response is active.
- Mode changes fence existing sockets before their next `response.create`.
- A key may hold at most eight live sockets. The first application frame must arrive within 30
  seconds, an idle connection between turns closes after five minutes, each write is bounded to
  120 seconds, and application messages are limited to 16 MiB. There is no hard total lifetime.
- `generate=false` prewarm is tracked for revocation safety but excluded from daily inference
  aggregates. Public/provider sockets support deflate; the internal loopback hop does not.
- A bare subscription socket may receive one hidden `generate=false` prewarm before its first
  ordinary turn. This happens only when the caller did not provide explicit `generate`,
  `previous_response_id`, or `conversation`. Codex callers are never given a duplicate prewarm.
  Automatic reuse follows the Codex `0.149.0` semantic projection, excluding volatile generated
  metadata and item identity; tool-output continuation or a true semantic mismatch safely falls
  back to a full frame without a previous response id.

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
  not rotate or create identity state; outputs are derived from account/request namespaces.
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
- The service is not active-active: SQLite, the credential vault, OAuth refresh locking, policy
  sidecars, revocation state, and usage history are node-local. Stateless identity projection alone
  is multi-machine safe; horizontal deployment first requires shared storage and distributed locks.
- Stop the service before backing up or restoring the complete state directory.
- Provider secrets live in a private `0600` vault and are not encrypted at rest.
- Credential fingerprint sidecars are private `0600` core-vault state containing only mode and
  revision. Installation IDs and pseudonym seeds are not persisted. The coordinator/core protocol
  carries a stateless downstream pseudonym scope that is removed before upstream send and is never
  written to usage records or logs.
- SQLite stores credential metadata, downstream-key hashes, timing, status, and token counts. It
  does not store prompts, request bodies, response bodies, tool arguments, or generated content.
- Every WebSocket `response.create` rechecks key and credential eligibility. Revoking a key or
  disabling a credential therefore prevents the next turn on an already-open idle socket.
- Inference is not replayed after transport errors, `429`, or `5xx`. OAuth may refresh once and
  replay once after a pre-response upstream `401`. Gateway errors expose `retryAdvice`, `phase`,
  and `deliveryState`; mid-stream HTTP failures use trailers and upgraded WebSockets use close code
  `4500` with the same JSON tuple. Treat `ambiguous` as possibly delivered and never auto-retry it.

Connection pools are credential-isolated. HTTP uses platform TLS; provider WebSockets use the
pinned `0.149.0` AWS-LC/native-root stack, fork revisions, compression offer, header order, and
fresh TLS state. OAuth routes exclude API-key organization/SDK headers and retain only allowlisted
Cloudflare infrastructure cookies.

Compatibility is exact for request state that a `0.149.0` client supplies or the gateway can derive.
The gateway does not fabricate absent caller workspace, sandbox, thread-source, trace, attestation,
or tool-inventory state, and it preserves explicit supported Responses controls plus arbitrary
keys inside documented opaque/free-form containers. Its provider HTTP clients also refuse redirects
so credentials cannot be replayed to a redirected authority; this is an intentional security
boundary from the stock CLI client.

OAuth issuer/client and upstream URL overrides are available for controlled compatibility testing.
Plain HTTP overrides are accepted only for literal loopback IPs.
