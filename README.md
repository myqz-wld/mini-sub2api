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

### Credential routing

Each distribution key selects its bound upstream credential. There is no account pool or automatic
account switching. Caller markers such as `Originator` do not select emulation or authentication.

| Upstream credential | Behavior for every caller |
|---|---|
| API key | Request bodies, valid WS application frames and response bodies pass through unchanged, including Codex-marked callers. Authentication, routing, admission, usage accounting and reviewed response-header policy still apply. Identity/cache failures do not affect this route. |
| Codex subscription | Emulate the supported Codex v0.153.4 request shape, defaults and identity metadata. HTTP uses zstd level 3 upstream; WS application frames remain JSON. |

### Context and continuation

HTTP stays HTTP, including SSE/JSON response adaptation. WebSocket stays WebSocket during full
sending, optimization and recovery. Neither path falls back to the other transport.

| Subscription request | Upstream behavior |
|---|---|
| Full HTTP, ordinary or Lite | Send the complete validated request over HTTP. |
| HTTP with `previous_response_id` | Append all supplied input to that exact completed response's locally materialized context, then send full HTTP without the reference. Missing context fails before inference. |
| Full WS, ordinary or Lite | Send full WS, or use an eligible completed socket baseline to send a suffix and `previous_response_id`. |
| WS with `previous_response_id` | Continue through that live upstream socket when ownership, mappings and format permit. Complete local history is needed only when the selected transformation or replacement socket requires full sending. |

A valid previous response means append semantics: even apparently repeated history remains in the
input. Earlier-response forks use that response's context. Failed and incomplete responses never
become completed context baselines. Output item events and final output are reconciled once, and
response ownership/context is published before the corresponding public event.

Session lookup is scoped by distribution key and upstream account namespace:

1. The original HTTP/WS handshake `session-id` header takes priority, followed by
   `client_metadata.session_id`, then turn metadata `session_id`.
2. Without an explicit session, a known `previous_response_id` restores its owning session.
3. Full history can match completed-response prefixes within a known session or the same key's
   anonymous-only pool. Explicitly identified sessions are excluded from that pool. Structured
   content, explicit IDs and tool dependencies determine eligibility before the longest match;
   recency does not resolve conflicting contexts.
4. A WS connection binds its session on the first request. Later frames inherit it and reject
   cross-session identities/references. Handshake turn/window fields apply to the first frame;
   later frames provide current turn evidence. Reconnection performs session lookup again.

`conversation_id`, cache keys, request IDs and thread/window identifiers do not locate a session.
Existing mapped top-level `conversation` references remain a compatibility boundary, and cannot
establish complete local history. This service does not provide `/v1/conversations` management.

Turns and responses are separate: valid explicit turn identity is authoritative; otherwise known
context and tool dependencies determine continuation. New user input after a quiescent completed
context starts a turn. Tool follow-ups retain their turn. Equivalent anonymous contexts can share
immutable content while retaining independent execution state. Each branch/turn and physical WS
allows one inference at a time. Identity edits commit only after admission; cache identity facts
publish after that commit.

The first upstream `x-codex-turn-state` from a handshake or `response.metadata` is retained within
its turn. A new turn starts without the prior token. That token is distinct from the generated or
mapped UUIDv7 turn ID. Optional v0.153.4 window, fork, trigger and history-ingest metadata is validated
and forwarded without becoming session identity. Context-window UUIDs receive scoped aliases.

### Retention and limits

Complete history, comparison snapshots and prefix-index membership expire after **3 hours of business
inactivity**, checked at lookup and by a 30-second sweep. Active operations are protected. Expiry does
not disconnect a live WS, rotate required aliases or clear its valid turn token. A full request can
rebuild materialized history. Expired HTTP increments fail; valid live WS increments may continue
through upstream history. A remote-only response stays unmaterialized locally until full history is
actually supplied.

| Resource | Default |
|---|---:|
| Accepted request and selected outgoing JSON/frame | 128 MiB |
| Collected response / WS response frame | 128 MiB |
| Retained output items per response | 8,192 |
| Context, index, live metadata and assembly reservations per Core | 2 GiB |
| Per distribution key | 1 GiB |
| Per session | 256 MiB |
| Completed response records per session | 8,192 |

The cache budgets account for retained objects and assembly reservations; they are not a process-RSS
limit. Protocol buffers have separate request/response bounds. A small WS delta is checked at its
selected frame size, independently of the expanded history size. Essential capacity is reserved
before inference. Completed body-retention overflow preserves otherwise valid delivery and marks
its context unavailable; it never publishes truncated history as usable. Compaction whose replacement
history depends on client retention choices, and interleaved injection, require a subsequent full
request to establish reconstruction proof.

[Shared limit defaults](src/protocol/v1/go/limits.json) and the `MINI_SUB2API_LIMITS` JSON environment
variable apply across Go and Rust. Operators can override individual fields, for example:

```bash
export MINI_SUB2API_LIMITS='{"requestBytes":134217728,"outputBytes":134217728}'
```

Values must be positive integers, with `sessionBytes <= keyBytes <= globalBytes`. Unknown fields,
invalid types and invalid hierarchies are rejected. Caller metadata cannot override these settings.
Authentication/error-body limits remain separate. Each key may hold eight WS connections; existing
first-frame, idle and write limits remain 30 seconds, 5 minutes and 120 seconds.

Private identity files retain UUID assignments and reversible schema-owned ID mappings, with no
request/response bodies or raw distribution keys. They remain schema v1, bounded to 512 MiB per
account namespace, with inactive detail eligible for pruning after 30 days. Live/retained context
mappings are protected independently of bulk history. Installation IDs use UUIDv4, session/thread/
turn IDs use UUIDv7, and generated Lite prefix IDs use native UUIDv5 thread/payload derivation.

### Delivery and recovery

A missing required mapping or context fails as `state_unavailable` before inference. Full WS sending
on a replacement connection requires complete local history. Automatic WS recovery permits at most one
additional attempt only while public inference is proven unsent; the upstream rejection retry
allowlist is empty. An attempted send, uncertain completion or any delivered current-response event
prevents hidden replay. OAuth handshake/authentication refresh remains bounded to its existing single
retry. A caller may submit a later full request after a surfaced failure.

Failure metadata exposes `retryAdvice`, `phase` and `deliveryState`. HTTP failed/incomplete/error
terminals remain valid Responses output and count as upstream errors. Subscription non-2xx bodies
are bounded gateway errors; API-key bodies stay transparent. Provider response headers are reviewed:
public request-ID headers use gateway aliases, while one bounded provider ID may be retained in local
diagnostics. Sandbox names follow the gateway OS, preserving permission meaning and caller workspaces.

See [the v1 protocol reference](src/protocol/v1/README.md) for transport and failure contracts.

### Base and developer instructions

Subscription emulation prefers a nonblank caller `instructions` string verbatim, including whitespace
and literal template syntax. Missing, null, blank and non-string bases use the selected model default
only when a base is required.

| Caller shape | Placement |
|---|---|
| Ordinary Responses | Selected base stays in top-level `instructions`. |
| Ordinary converted to Lite | `additional_tools`, one selected-base developer message, then original input. Remove top-level `instructions`. |
| Already formed Lite | Preserve input instructions. An explicit valid top-level base is inserted after tools; otherwise add no fallback base. |
| Valid Lite WS delta | Inherit validated caller format and existing setup. Changed setup requires a complete full send when it cannot be represented as the existing delta. |

Developer messages retain content, duplicates and relative order. Subscription converts `system` to
`developer` in place. Caller format, actual upstream format and setup provenance stay distinct, so
ordinary-to-Lite conversion does not reclassify future ordinary input as native Lite.

The [offline snapshots and generator](src/core/codex/prompts/codex-0.153.4/README.md) cover all eleven
catalog models plus generic and experimental fallbacks. Native template variables are rendered;
literal examples such as `{{connector_id}}` remain unchanged. Caller text is never rendered. Explicit
supported `access_programs` selection and `sequential_cutoff` summary delivery are forwarded per
response; the gateway does not synthesize account entitlements.

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
mise exec -- go test -count=1 ./src/coordinator/...
bash scripts/test.sh
bash scripts/build.sh
```

The direct Go integration suite builds the current debug core when no explicit test binary is set;
it never silently skips cross-language coverage. `scripts/test.sh` disables Go test-result caching
so Rust-only changes are exercised through the newly built Core, including race checks.

## Disclaimer

This project is for personal learning and research. It is not an official OpenAI product and is not
intended for commercial or production use. Users are responsible for applicable laws and service
terms.
