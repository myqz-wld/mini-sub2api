# Coordinator/Core Protocol v1

This protocol is private to one mini-sub2api deployment unit. It connects the Go coordinator to a
supervised provider core over loopback HTTP and WebSocket. It is not a public provider API.

## Process startup

1. The coordinator starts `mini-sub2api-core-codex serve --listen 127.0.0.1:0 --state-dir <path>`.
2. The coordinator writes one line containing a 256-bit URL-safe bearer token to the child's stdin and closes stdin.
3. The core binds IPv4 loopback and writes exactly one JSON line to stdout. The line is at most 4096 bytes and matches `fixtures/readiness.json`.
4. All diagnostic output goes to stderr. Secrets and request/response bodies are forbidden in stdout and stderr.

The core exits rather than accepting an empty token, a non-loopback internal listener, an already-locked state directory, or an unsupported configuration.

## Readiness record

```json
{
  "protocolVersion": "1",
  "port": 42123,
  "pid": 12345,
  "build": {
    "name": "mini-sub2api-core-codex",
    "version": "0.1.0",
    "commit": "0123456789abcdef0123456789abcdef01234567"
  },
  "capabilities": {
    "responsesWebSocket": true
  }
}
```

The coordinator rejects a protocol version other than the exact string `1`. It also rejects a
core that does not advertise `capabilities.responsesWebSocket=true` when opening the internal
WebSocket route. The capability is additive: older v1 readiness decoders may ignore it and the
existing HTTP inference route does not depend on it.

## Inference request

```text
POST /internal/v1/responses HTTP/1.1
Authorization: Bearer <internal-token>
X-Mini-Sub2Api-Protocol-Version: 1
X-Mini-Sub2Api-Account-Ref: acct_<opaque-id>
X-Mini-Sub2Api-Pseudonym-Scope: psn_<sha256-derived-scope>
X-Mini-Sub2Api-Request-Id: req_<opaque-id>
Content-Type: application/json
```

- The TCP peer must be loopback.
- `Authorization` must match the startup token.
- Account references and request ids are opaque ASCII values, 1-128 characters after their prefix.
- The pseudonym scope is a 32-byte base64url digest derived from the authenticated downstream key
  verifier. It is stable across machines that share the same downstream key, is never accepted
  from the public caller, and never crosses the provider boundary.
- The request body is the public Responses body and is never persisted or logged.
- Raw provider credentials never appear in protocol headers or bodies.

## Forwarded request headers

The coordinator may forward only these public-client headers to the core:

- `accept`
- `content-encoding`
- `content-type`
- `originator`
- `session-id`
- `thread-id`
- `traceparent`
- `tracestate`
- `user-agent`
- `version`
- `openai-beta`
- `openai-organization`
- `openai-project`
- `x-client-request-id`
- `x-codex-beta-features`
- `x-codex-routing-hint`
- `x-codex-inference-call-id`
- `x-codex-turn-state`
- `x-codex-turn-metadata`
- `x-codex-parent-thread-id`
- `x-openai-subagent`
- `x-codex-window-id`
- `x-codex-installation-id`
- `x-openai-internal-codex-responses-lite`
- `x-openai-internal-codex-residency`
- `x-openai-memgen-request`
- `x-oai-attestation`
- `x-responsesapi-include-timing-metrics`
- `x-stainless-arch`
- `x-stainless-lang`
- `x-stainless-os`
- `x-stainless-package-version`
- `x-stainless-retry-count`
- `x-stainless-runtime`
- `x-stainless-runtime-version`
- `x-stainless-timeout`
- `session_id`
- `conversation_id`

The core constructs authoritative `Authorization` and `Host` headers. The bound credential alone
selects the request and response profile:

| Credential kind | Profile for every caller |
| --- | --- |
| OpenAI API key | `ApiKeyPassthrough` |
| Codex subscription | `CodexSubscription1534` |

Caller markers never grant permissions or change the selected credential. API-key bodies, valid WS
application frames and response bodies remain byte-transparent, including Codex-marked callers;
that path never reads identity/context state or applies the Subscription schema/default filter.
Authentication, admission, accounting and the reviewed header allowlists still apply.

Caller `x-codex-routing-hint` is preserved for API-key HTTP/WS requests, including an explicit empty
value; omission stays omission. Subscription derives its hint from the actual model/service tier.

Subscription replaces `User-Agent`, `originator`, and `version` with the runtime-derived Codex
v0.153.4 identity and adds `ChatGPT-Account-ID`. HTTP uses `Accept: text/event-stream`, JSON and
level-3 zstd. Only API-key upstreams receive `OpenAI-Organization`, `OpenAI-Project` and reviewed
`X-Stainless-*` headers. Both layers remove caller cookies, proxy authentication, forwarding headers,
content length, transfer encoding, connection-specific headers, unreviewed internal headers and
unknown `X-Stainless-*` headers.

## Subscription request preparation

Subscription follows Codex v0.153.4, commit `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`.
Classify original transport, identity/reference evidence and caller format before applying the overlay.
Caller format, complete context and transmitted format remain distinct. Deterministic repairs require
equivalent-content proof and revalidation. There is no HTTP/WS conversion.

[Routing and continuation](../../../docs/BEHAVIOR.md#routing) define full/incremental sends;
[instructions and Lite](../../../docs/BEHAVIOR.md#instructions-and-lite) define base/order/prefix rules.
Unknown required ownership, dependencies, aliases or history yield `state_unavailable`; malformed,
ambiguous or lossy evidence yields `invalid_request`, before inference. Previous IDs always append
every supplied input item to that exact response, including repetitions.

| Overlay | Wire behavior |
|---|---|
| Unsupported controls | Remove `max_output_tokens`, `temperature`, `top_p`, `metadata`, `user`, `prompt_cache_retention`, `safety_identifier`, `truncation`, and unsupported structured members. Preserve schema-owned opaque payloads. |
| Supported options | Retain `stream_options.reasoning_summary_delivery=sequential_cutoff` and explicit `access_programs.cyber`: `standard`, `daybreak_blue`, `daybreak_red`. Do not invent entitlement. |
| HTTP / WS | HTTP removes `type`, `generate`, `stream_id`; WS removes `background`. Both send `store:false`, `stream:true`. |
| Lite | `parallel_tool_calls:false`, `reasoning.context:all_turns`; caller/upstream format remains distinct. |
| Input | Map system→developer in place; historical assistant strings use `output_text`. Non-Lite missing image detail defaults to `high`; Lite leaves it absent. |
| IDs | Project caller inline IDs; ordinary omitted message IDs stay omitted. Required references must resolve. Native Lite prefix UUIDv5 uses OID + thread, then exact tools bytes/base text. |
| Public HTTP output | Retain caller stream preference: translated SSE when true, otherwise the final response object as bounded JSON. API-key traffic stays byte-transparent. |

Caller instructions remain verbatim, including whitespace/placeholders; missing/invalid bases are
omitted, never filled from model defaults. Ordinary-to-Lite emits tools, optional caller-base developer
text, then original input. Formed Lite preserves its input base; valid inherited increments do not
repeat setup. Developer content/order/duplicates survive. All model-default snapshots are test-only.

## Session, turn and context state

Use the [session/matching rules](../../../docs/BEHAVIOR.md#sessions-and-matching) and
[state limits](../../../docs/BEHAVIOR.md#state-and-limits). Account namespace and distribution-key
pseudonym scope isolate state; Core never receives the raw Key. Admission allows one operation per
branch/turn and physical WS. Commit identity after successful admission, then publish cache aliases.
The first upstream `x-codex-turn-state` is retained for its logical turn, separately from UUIDv7 turn
identity; new turns start empty. Window/fork/trigger/history-ingest metadata is validated/projected,
not used as a session locator.

Defaults in [go/limits.json](go/limits.json) are embedded in both languages.
`MINI_SUB2API_LIMITS` accepts partial operator-only JSON overrides for `requestBytes`, `outputBytes`,
`outputItems`, `globalBytes`, `keyBytes`, `sessionBytes`, `sessionRecords`: positive integers up to
2^40 with `sessionBytes <= keyBytes <= globalBytes`. Unknown/malformed values fail startup.
[Shared fixtures](fixtures/limits.json) exercise the same defaults/overrides in Go and Rust.

Account shared allocations once and reserve essential capacity before inference. Measure WS deltas
at actual transmitted size. Body-retention overflow may preserve delivery while making history
unavailable; partial history is never reusable. Protocol/auth/error buffers have separate bounds.
These budgets do not cap RSS. Live WS ownership/aliases/tokens survive bulk history expiry; a remote
suffix cannot recreate missing full history.

## Persistent relationship-aware pseudonymization

Persist scoped session/thread/turn UUIDv7 and installation UUIDv4; HMAC-SHA256 supplies private lookup
keys. Parent/fork/subagent relationships create child threads within the session. Prewarm retains its
empty turn ID. Duplicate OAuth credentials share an account namespace; Keys remain isolated.
API-key passthrough never accesses the identity graph.

Each account uses one private `rs_<namespace-digest>.request-state.json`: 0600, atomic replacement
under namespace lock, serialization v1, at most 512 MiB. It contains typed identity/lineage/window/
compaction/reversible ID mappings, never bodies, raw Keys, arguments, credentials or workspaces.
Inactive detail is pruning-eligible after 30 days; active/retained/live dependencies stay protected.
Ancestor eviction protects descendants or removes the entire descendant graph. Corrupt/oversized/
unsupported state is preserved and fails with `state_unavailable`; final-owner deletion can remove
it without decoding, while duplicate credential owners retain it.

Publish response ownership/mappings before IDs become visible and complete context before terminal
delivery. Reconcile output once; failed/incomplete attempts never form complete baselines.
Definition carriers allocate aliases; historical provider references require provider-origin mappings.
Translate only enumerated lifecycle fields; opaque content and file/vector-store/model/connector/prompt
IDs remain unchanged. [Completion and compaction](../../../docs/BEHAVIOR.md#completion-and-recovery)
define publication and reconstruction proof.

Turn/prewarm/compaction metadata preserves legal sandbox meaning with Core OS implementation;
missing/invalid evidence defaults to `danger-full-access/none`. Body/header metadata agree and
caller workspaces remain intact.

Each credential owns HTTP/WS transport contexts. HTTP uses reqwest/native-tls. Provider WS uses
AWS-LC rustls/native roots, PQ-first groups, HTTP/1 without ALPN, and fresh TLS session state per
connection. Pinned tungstenite forks and compression match v0.153.4; Go need not parse bodies for identity.

## Inference response

- The core preserves the upstream status but default-denies provider response headers. It allows
  only content/cache encoding metadata, retry and rate-limit fields, model/timing fields,
  `x-reasoning-included`, `x-models-etag`, and opaque `x-codex-turn-state`. Recognized
  `x-request-id`, `openai-request-id`, and `request-id` names are retained with every value replaced
  by the current gateway `req_*` alias. Lifecycle identity headers and unknown headers are removed.
- One raw provider request ID is selected in `x-request-id`, `openai-request-id`, `request-id`
  priority order. It must be 1-512 bytes of visible ASCII and crosses loopback only in the private
  `X-Mini-Sub2Api-Provider-Request-Id` header. The coordinator consumes and removes it, stores it in
  nullable request detail, and exposes it only through local `usage history` output.
- `ApiKeyPassthrough` response bodies remain byte-transparent. Subscription rewrites only
  enumerated lifecycle IDs on successful JSON/SSE responses. Any stateful Codex non-2xx response
  discards the raw provider body and returns a bounded core error with the gateway request ID and
  `never/upstream_response/delivered`; its safe status and allowlisted headers remain.
- The core adds `X-Mini-Sub2Api-Core-TTFB-Ms` after receiving upstream response headers.
- For an aggregated Codex response, the core adds the optional private
  `X-Mini-Sub2Api-Response-Terminal` header with `completed`, `failed`, or `incomplete`. The
  coordinator consumes it only for local accounting and never forwards it publicly. Streaming
  terminal types remain in the SSE body and do not use this header.
- Every streamed response declares the three failure trailers. They remain empty on clean EOF; a
  provider-body failure emits `X-Mini-Sub2Api-Failure-Phase`,
  `X-Mini-Sub2Api-Delivery-State`, and `X-Mini-Sub2Api-Retry-Advice` as the final trailer block.
- The coordinator removes internal headers, adds the public request id, and merges `upstream_ttfb;dur=<milliseconds>` into `Server-Timing`.
- The coordinator validates and republishes a failure trailer block without inserting bytes into
  the JSON/SSE body.
- The coordinator does not mutate SSE events. The core buffers one bounded stateful Codex SSE
  event, persists any new ID pairs, then emits that event with translated IDs; comments and non-data
  event fields are retained. `ApiKeyPassthrough` SSE remains byte-transparent.
- Client cancellation cancels the internal request and upstream response body.

`fixtures/sse_progress.json` defines matching Go/Rust observations for HTTP output deadlines and
fixed diagnostic categories. Nonempty text/reasoning/tool/media fields and completed items count as
output; status, unknown and empty events do not. Classification never proves valid completion or
changes API-key bytes. [HTTP limits](../../../docs/BEHAVIOR.md#completion-and-recovery) define timing.

## Responses WebSocket

The readiness capability `capabilities.responsesWebSocket=true` enables:

```text
GET /internal/v1/responses/ws HTTP/1.1
Authorization: Bearer <internal-token>
X-Mini-Sub2Api-Protocol-Version: 1
X-Mini-Sub2Api-Account-Ref: acct_<opaque-id>
X-Mini-Sub2Api-Pseudonym-Scope: psn_<sha256-derived-scope>
X-Mini-Sub2Api-Request-Id: req_<opaque-connection-id>
Connection: Upgrade
Upgrade: websocket
```

Validate the same peer/auth/version/account/scope/request-ID contract as HTTP. The coordinator dials
Core before accepting the public socket. API-key Core connects upstream before internal `101`;
Subscription upgrades internally first, then connects upstream from the first normalized create.

- One internal socket owns at most one provider socket. Core enforces branch/turn/socket admission;
  Go owns eight sockets/Key, credential revalidation, public overlap policy, operation accounting,
  first-frame/inter-turn/write deadlines and shutdown.
- Application frames are bounded UTF-8 JSON text. API-key valid frames are byte-exact. Subscription
  overlays creates and translates enumerated IDs on controls. `response.inject` retains only
  `type/input/response_id`, filters item schemas and preserves opaque tool payloads; other non-create
  frames remain byte-exact when IDs need no change.
- Preserve nonempty `x-codex-ws-stream-request-start-ms`; generate only missing/empty values.
  Native prewarm retains empty `turn_id` and absent `root_turn_id/turn_started_at_unix_ms`.
  Deferred handshake turn metadata comes from the normalized first frame. Hidden prewarm uses
  `request_kind=prewarm`, empty turn ID and no root/parent/start fields in both handshake and frame.
  A replacement whose first frame is public full-create instead uses that public turn identity.
- Before every create, re-read the fingerprint revision. Changed/unreadable revisions close both
  sockets with empty-reason `1012` before sending. Other events do not trigger this check.
  Observe upstream response IDs before translation, then persist mappings before downstream delivery.
- Provider beta is `responses_websockets=2026-02-06`. Subscription uses its account header and
  runtime identity; OAuth retains provider/extra/default/auth header construction and one default originator.
- Public/provider hops may negotiate deflate; the provider offer is
  `permessage-deflate; client_max_window_bits`. Internal loopback does not request compression;
  WS application data never uses zstd. Ping/pong/close/cancellation/backpressure stay connection-scoped.
- Bare Subscription may perform one hidden `generate=false` prewarm only without explicit
  `generate/previous_response_id/conversation/stream_id`; Codex-marked callers receive no duplicate setup.
  Automatic suffix reuse requires saved model/instructions/tools/tool-choice/parallel/reasoning/store/
  stream/include/service-tier/cache-key/text settings and completed input+output prefix equality on
  the same thread/socket. `client_metadata/stream_options` are excluded. Clear only internal chat-message
  metadata and exclude temporary emission-only IDs; semantic caller/provider IDs remain strict.
  Explicit previous responses keep append semantics and require full local history only for full sending/conversion.
- Proven-unsent setup allows at most one replacement full-WS attempt, without HTTP fallback.
  Attempted business inference is never silently replayed.
- Gateway failures after upgrade use `4500` with exactly `retryAdvice/phase/deliveryState` JSON.
  Standard protocol/policy/lifecycle/fingerprint close codes remain. Deferred Subscription rejection
  cannot become the original public HTTP rejection; API-key keeps pre-upgrade bounded HTTP rejection.

Compaction v2 uses ordinary Responses fields, including `compaction_trigger` and
`request_kind=compaction`; it adds no route. Retry markers use projected thread plus stable operation
evidence. They hold pending targets until accepted completion durably commits before delivery.
Failed/incomplete/error/non-2xx/disconnected attempts do not advance windows. Same-base successes
converge once; marker access refreshes retention/protects eviction. Replacement histories follow the
[completion contract](../../../docs/BEHAVIOR.md#completion-and-recovery). Explicit-reference intent
survives full WS reconstruction and prevents hidden setup/reuse.

API-key internal upgrade may carry the private provider-ID header. Deferred Subscription sends this
reserved loopback text control before application output, when a valid provider ID is available:

```json
{"type":"mini_sub2api.provider_request_id","providerRequestId":"provider-visible-ascii"}
```

Go validates/consumes it into current-operation diagnostics without starting TTFB or forwarding it.
Deferred rejection may send the control before structured `4500`, never a raw provider body.
Go reapplies the safe response-header allowlist and builds its own handshake; credentials, cookies,
forwarding/proxy fields, unknown headers and arbitrary extensions do not cross.

## Internal errors

Before any upstream response bytes are sent, core errors use the JSON shape in `fixtures/error.json` with one of these codes:

- `invalid_internal_auth`
- `unsupported_protocol`
- `invalid_request`
- `unknown_account`
- `state_unavailable`
- `credential_disabled`
- `credential_requires_login`
- `credential_busy`
- `upstream_connect_failed`
- `upstream_delivery_unknown`
- `upstream_response_failed`
- `upstream_handshake_rejected`
- `upstream_auth_failed`
- `internal_error`

Every error also carries `retryAdvice`, `phase`, and `deliveryState`. The coordinator accepts only
known codes, the matching request id, valid enum values, and a coherent retry/delivery pair before
mapping it to a stable OpenAI-shaped public error. It never exposes account existence, filesystem
paths, credentials, auth endpoint bodies, or internal process details.

## Retry contract

The v1 failure contract is a three-field tuple:

- `retryAdvice`: `safe`, `ambiguous`, or `never`.
- `deliveryState`: `not_delivered`, `possibly_delivered`, or `delivered`.
- `phase`: `internal`, `request`, `credential`, `upstream_connect`, `upstream_request`,
  `upstream_response`, `upstream_stream`, or `websocket_relay`.

`safe` is valid only with `not_delivered`; `ambiguous` only with `possibly_delivered`; `never` with
either `not_delivered` or `delivered`. A caller may retry `safe` according to its own rate/backoff
policy. It must treat `ambiguous` as possibly already executed and must not automatically replay it.
`never` is also not automatically replayed.

The core records the real transport boundary. An HTTP connect failure is safe; failure after the
send attempt but before response headers is ambiguous; an upstream response or later stream
failure proves delivery. For WebSocket, a `response.create` becomes ambiguous immediately before
the provider write, becomes delivered after the first provider application event, and returns to
idle after a terminal event. State-store failures retain that active delivery state instead of
resetting it to safe; an upstream response or application event is recorded before fallible response
ID translation. A deferred Codex handshake failure occurs before inference delivery.

The coordinator parses this metadata from pre-response JSON, HTTP trailers, and WebSocket `4500`
reasons. If an upgraded core socket fails without valid metadata, its own operation tracker falls
back conservatively: idle is safe, an active turn with no core event is ambiguous, and an active
turn after a core event is delivered. Malformed or incoherent `4500` payloads are never forwarded
as trusted metadata.

Neither layer retries transport, `429`, or `5xx` inference failures. OAuth may perform exactly one
forced credential refresh and one replay after an upstream `401`, before response bytes reach the
coordinator; this is credential recovery, not a general inference retry policy.

## Compatibility changes

The protocol has not been released outside this deployment, so the delivery-aware failure shape
replaces the earlier v1 boolean in place. After v1 is externally released, incompatible changes
must use a new `src/protocol/vN/` directory.
