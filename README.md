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

Every credential defaults to `--fingerprint-mode device`: subscription installation identity uses
one persisted UUIDv4 per ChatGPT account, while `off` uses scoped persisted UUIDv4 values. Root
conversation/thread and ordinary turn identities use stable UUIDv7 values. Subscription Responses
correlation IDs are translated bidirectionally, so caller IDs are restored on responses and
provider IDs receive stable downstream aliases. Response aliases retain their conversation/thread
owner across restarts, while otherwise carrier-free calls never correlate solely because their
content matches. OpenAI API-key payloads remain transparent.

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

The emulated profiles clone the caller object, retain only fields supported by OpenAI Responses or
the fixed Codex capture, and apply missing `0.149.0` defaults. Except for documented target-specific
compatibility exceptions, explicit supported values remain authoritative on their transport,
including `previous_response_id`, HTTP `background`, WS
`stream_id`, tools, sampling, and image detail. Unknown structured members are removed; documented
JSON Schema, prompt variables, and function/custom payloads remain opaque.
Web and File Search use separate filter schemas. HTTP never synthesizes `previous_response_id`;
non-Lite image detail defaults to `high`, while Lite leaves an omitted detail absent. Subscription
emulation maps message role `system` to `developer` and removes output-cap/sampling controls that
the Subscription endpoint rejects: `max_output_tokens`, `temperature`, `top_p`, and
`stream_options`. `CodexOpenAi149` preserves those explicit fields; Codex `0.149.0` can emit
`stream_options.reasoning_summary_delivery=sequential_cutoff` on the API-key path when concurrent
reasoning summaries are enabled. Both Codex-emulated profiles remove caller `metadata`, `user`,
`prompt_cache_retention`, `safety_identifier`, and `truncation`; Codex identity remains available
through `client_metadata`. `BareOpenAi` continues to forward those public OpenAI fields
byte-for-byte. On the subscription profile, supported correlation values remain semantically
authoritative but are replaced by persistent upstream pseudonyms at the wire boundary.

Both emulated HTTP profiles always send `store: false`, `stream: true`, and
`Accept: text/event-stream` upstream. The gateway reads the caller's original `stream` preference
before applying that wire override: `stream: true` remains an SSE response, while an omitted or
false value is assembled from the terminal upstream SSE event and returned as an ordinary
`application/json` Responses object, with a 64 MiB aggregation limit. `BareOpenAi` remains
byte-transparent and keeps the public OpenAI request/response behavior unchanged.

Both emulated profiles pin the model-specific Codex `0.149.0` default base prompt. Normal Responses
replaces top-level `instructions` with that prompt and moves a non-empty caller customization into
a leading `developer` input message. Responses Lite emits `additional_tools`, the canonical base
prompt as a `developer` message, then the caller customization and original input; its top-level
`instructions` remains absent. Existing known `0.149.0` prompts are replaced rather than duplicated,
and an incremental Lite WebSocket frame does not repeat the established prompt. Model prefix and
single-namespace lookup follows the pinned catalog, with its bundled fallback for unknown models.
`BareOpenAi` remains byte-transparent and does not receive these prompts.

`CodexOpenAi149` and `CodexSubscription149` both replace caller identity with
`codex-tui/0.149.0 (<runtime OS>; <runtime architecture>) <runtime terminal> (codex-tui; 0.149.0)`,
`originator: codex-tui`, and `version: 0.149.0`. The runtime platform snapshot is captured on
first use and shared for the life of the core process. Subscription identity is resolved before
projection: conflicting root carriers converge on one persisted UUIDv7, explicit
parent/fork/subagent lineage receives a distinct child UUIDv7, and installation is a genuine
persisted UUIDv4. HMAC-SHA256 is used only for private lookup keys, never to forge UUID bits.

WebSocket policy is intentionally small and split across the coordinator and core:

- One socket is bound to one downstream key and credential; responses are sequential, and an
  overlapping `response.create` closes the connection with a policy violation.
- `BareOpenAi` keeps every valid Responses v2 JSON application event byte-exact. Simulated
  `response.inject` events retain only documented `type`, `input`, and `response_id` carriers and
  apply the same structured-item filtering as create input. Subscription create/inject/control
  frames translate only enumerated lifecycle IDs; a typed non-create frame with no such ID remains
  byte-exact.
- Mode changes fence existing sockets before their next `response.create`.
- A key may hold at most eight live sockets. The first application frame must arrive within 30
  seconds, an idle connection between turns closes after five minutes, each write is bounded to
  120 seconds, and application messages are limited to 16 MiB. There is no hard total lifetime.
- `generate=false` prewarm is tracked for revocation safety but excluded from daily inference
  aggregates. Public/provider sockets support deflate; the internal loopback hop does not.
- A bare subscription socket may receive one hidden `generate=false` prewarm unless the caller
  supplied `generate`, `previous_response_id`, `conversation`, or `stream_id`. It uses an independent
  prewarm identity; explicit/provider item IDs remain semantic during reuse, while temporary wire
  IDs do not. Incompatible continuation falls back to a full frame.
- Simulated WS `response.create` removes HTTP-only `background` and implicit `stream`, while
  preserving WS-only `stream_id`. It also applies the Codex-emulation field filter described above.

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
- The service is not active-active: SQLite, the credential vault, OAuth refresh locking, request
  identity state, policy sidecars, revocation state, and usage history are node-local. Horizontal
  deployment first requires shared storage and distributed locks.
- Stop the service before backing up or restoring the complete state directory.
- Provider secrets live in a private `0600` vault and are not encrypted at rest.
- Credential fingerprint sidecars remain private `0600` state containing only mode and revision.
  Subscription request identity lives separately in one versioned
  `rs_<account-digest>.request-state.json` per upstream ChatGPT account. The filename hides the
  account ID; the file is `0600`, atomically replaced, limited to 16 MiB, and shared by duplicate
  local credentials until the last owner is removed. It stores generated UUIDv4/v7 assignments,
  relationships, windows, timestamps, and bounded schema-recognized raw ID pairs needed for
  transparent reversal—never request/response bodies, content, workspace values, credentials,
  opaque values, or tool arguments. Completed turn/item/compaction/wire detail becomes eligible for
  LRU pruning after 30 days; conversation identity is capacity-LRU only, and ancestor eviction
  removes or retains a complete relationship graph rather than leaving dangling lineage. A retried
  compaction refreshes and protects its idempotency marker in the current edit. Corrupt or unsupported
  state is preserved and the affected account returns retryable `state_unavailable` before upstream
  delivery. Credential deletion remains available: non-final owners preserve corrupt evidence and
  the final owner removes it.
- SQLite stores credential metadata, downstream-key hashes, timing, status, and token counts. It
  does not store prompts, request bodies, response bodies, tool arguments, or generated content.
- Every WebSocket `response.create` rechecks key and credential eligibility. Revoking a key or
  disabling a credential therefore prevents the next turn on an already-open idle socket.
- Inference is not replayed after transport errors, `429`, or `5xx`. OAuth may refresh once and
  replay once after a pre-response upstream `401`. Gateway errors expose `retryAdvice`, `phase`,
  and `deliveryState`; mid-stream HTTP failures use trailers and upgraded WebSockets use close code
  `4500` with the same JSON tuple. WebSocket state failures preserve the active create's attempted or
  observed delivery state, and an upstream HTTP response or WebSocket application event proves
  delivery before fallible ID translation. Treat `ambiguous` as possibly delivered and never
  auto-retry it.

Connection pools are credential-isolated. Every provider HTTP client explicitly selects
reqwest/native-tls so its ClientHello follows the deployment runtime; provider WebSockets retain the
pinned `0.149.0` AWS-LC/native-root stack, PQ-first groups, absent ALPN, fork revisions, compression
offer, header order, and fresh TLS state. OAuth routes exclude API-key organization/SDK headers and
retain only allowlisted Cloudflare infrastructure cookies.

Compatibility is exact for request state that a `0.149.0` client supplies or the gateway can derive.
Standard turn, prewarm, and compaction metadata always contains `sandbox_mode` plus a sandbox family
derived from the sidecar OS: restricted macOS/Linux/Windows requests use
`seatbelt`/`seccomp`/`windows_sandbox`, unrestricted uses `none`, and external uses `external`.
Legal permission semantics are retained; invalid or missing pairs become
`danger-full-access`/`none`. Caller `workspaces` values are preserved unchanged. Other absent
thread-source, trace, attestation, or tool-inventory state is not fabricated, and arbitrary keys
remain valid inside documented opaque/free-form containers. Provider HTTP clients also refuse
redirects so credentials cannot be replayed to a redirected authority; this is an intentional
security boundary from the stock CLI client.

OAuth issuer/client and upstream URL overrides are available for controlled compatibility testing.
Plain HTTP overrides are accepted only for literal loopback IPs.

## Disclaimer

This project is intended solely for personal learning and research. It is not an official OpenAI
product and is not intended for commercial or production use. Users are responsible for complying
with applicable laws and the terms of any services they access.
