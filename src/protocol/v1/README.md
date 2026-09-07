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

The supported shape follows the pinned Codex v0.153.4 source, commit
`3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`. Subscription requests are classified from original
transport, session/reference evidence, tools, instructions and input, before the wire overlay.
Caller format, complete effective context and transmitted format are distinct state. A model's
Lite default does not relabel an ordinary caller as native Lite.

| Caller transport and context | Selected upstream request |
| --- | --- |
| Full HTTP, ordinary or Lite | Full HTTP |
| HTTP with a valid completed `previous_response_id` | Exact parent history plus every supplied input item, sent as full HTTP without the reference |
| Full WS, ordinary or Lite | Full WS, or a strict reusable same-socket baseline plus suffix |
| WS with a valid completed `previous_response_id` | Same-socket upstream continuation when projection is possible; otherwise full WS requires complete local history |

Unknown ownership, dependencies, required aliases or full history fail with `state_unavailable`
before inference. Malformed evidence or an ambiguous/lossy shape fails with `invalid_request`.
An explicit valid previous response always means append semantics, including repeated input.
Deterministic repair is allowed only with proof of equivalent content, followed by validation.
There is no HTTP/WS transport conversion.

The overlay retains supported Responses fields and opaque/free-form schema payloads. It removes
unsupported structured members and Subscription output controls `max_output_tokens`, `temperature`
and `top_p`, and unsupported `metadata`, `user`, `prompt_cache_retention`, `safety_identifier` and
`truncation`. It supports native `stream_options.reasoning_summary_delivery=sequential_cutoff` and
explicit `access_programs.cyber` values `standard`, `daybreak_blue` and `daybreak_red`; it does not
invent an entitlement. HTTP removes WS-only `type`, `generate` and `stream_id`; WS removes HTTP-only
`background`. Both send `store: false` and `stream: true`. Lite uses `parallel_tool_calls: false`
and `reasoning.context: all_turns`.

Nonblank top-level base instructions are preserved verbatim, including surrounding whitespace and
caller template syntax. Missing, null, blank or non-string bases use the selected default when a
base is required. Ordinary Responses keeps the base at top level. Ordinary-to-Lite conversion emits
`additional_tools`, one base developer message, then original input, removing top-level instructions.
Already formed native Lite preserves input instructions and adds no fallback base; an explicit valid
top-level base goes after tools. Validated Lite WS increments can inherit their upstream prefix.
Developer messages and duplicates retain content and relative order. Subscription changes `system`
to `developer` in place; historical assistant strings use `output_text`. Non-Lite absent image detail
defaults to `high`; Lite leaves it absent. All eleven catalog models and both fallback sources are
snapshotted offline under native literal/template-variable rendering rules.

Caller inline IDs are projected through scoped mappings; omitted ordinary message IDs remain
omitted. Schema references must resolve their required mapping. Generated Lite prefix IDs use the
native UUIDv5 derivation: OID namespace plus resolved thread, then serialized tools or exact base
text. This is separate from identity projection for caller-defined items.

For HTTP, the caller's original stream preference is retained locally. A streaming caller receives
translated SSE. An omitted/false stream preference receives the final completed, failed or incomplete
response object as JSON. Aggregation and individual stateful events use the configured output bound.
API-key HTTP responses remain byte-transparent regardless of stream preference.

## Session, turn and context state

Core isolates state by upstream account namespace and the coordinator's distribution-key pseudonym
scope; it never receives the raw distribution key. Session evidence priority is original `session-id`
header, flat `client_metadata.session_id`, then nested turn metadata `session_id`. Without explicit
identity, a known `previous_response_id` restores its owner. Full requests may associate at completed
response boundaries within the known session or same-key anonymous-only pool. Explicit sessions are
excluded from the anonymous index. ID/dependency eligibility precedes longest-prefix selection;
fully validated equivalent contexts may share immutable content, never mutable execution ownership.

`conversation_id`, prompt-cache keys, request IDs and thread/window carriers do not locate sessions.
A mapped top-level `conversation` remains a separate compatibility path with unavailable local
reconstruction proof. No `/v1/conversations` management is provided. A WS binds its first resolved
session; later frames inherit it and reject cross-session identities/references. Original handshake
session evidence remains, while later frames provide current turn/window metadata. A replacement
connection resolves identity again.

A turn may contain several responses. Explicit valid turn identity is authoritative; otherwise
validated parent context and outstanding tool-call relationships determine continuation. New user
input after quiescent completion starts a new turn. Tool and empty continuations do not independently
start turns. Admission permits one operation per resolved branch/turn and selected physical WS.
Identity changes commit after admission succeeds; cache aliases publish after the identity commit.

The first upstream `x-codex-turn-state`, from upgrade headers or `response.metadata`, is retained
within its logical turn, separately from the client-created/mapped UUIDv7 turn ID. A new turn starts
empty. Optional native window number, context-window UUID, fork ordinal, trigger and history-ingest
metadata is validated and projected without becoming session identity.

Full history, index membership and WS comparison bodies expire after three hours of business
inactivity, with immediate lookup checks and a 30-second sweep. Active operations pin required data.
Expiry does not disconnect sockets or discard their valid ownership, aliases or turn token. A valid
live WS delta can continue remotely after expiry; its suffix/output does not make history complete.
HTTP expansion and full WS reconstruction require materialized local history. A supplied full request
can rebuild it. Output retention overflow preserves valid delivery and marks context unavailable;
truncated history is never reusable. Compaction requiring client retention/truncation choices and
interleaved control/injection similarly invalidate full-context proof.

The shared defaults are `go/limits.json`, embedded in both languages. `MINI_SUB2API_LIMITS` accepts
operator-only partial JSON overrides for `requestBytes`, `outputBytes`, `outputItems`, `globalBytes`,
`keyBytes`, `sessionBytes` and `sessionRecords`. Values are positive integers up to 2^40 and must
satisfy `sessionBytes <= keyBytes <= globalBytes`; unknown keys or malformed values fail startup.
`fixtures/limits.json` exercises the same defaults/overrides in Go and Rust.

| Bound | Default |
| --- | ---: |
| Ingress and selected outgoing request/frame JSON | 128 MiB |
| Collected output / provider WS frame | 128 MiB |
| Retained output items | 8,192 per response |
| Retained context/index/live metadata and assembly reservations | 2 GiB global / 1 GiB per key / 256 MiB per session |
| Response descriptors, including live-only records | 8,192 per session |

Shared allocations are accounted once. These are context/assembly budgets, not a process-RSS limit;
protocol buffers have separate bounds. Small WS increments are checked at actual transmitted size,
independently of expanded history. Essential capacity is reserved before inference. Authentication
and diagnostic/error-body bounds remain separate.

## Persistent relationship-aware pseudonymization

Subscription session/thread/turn identities use scoped persisted UUIDv7 values. Explicit parent,
fork or subagent relationships create child threads within the selected session. Prewarm retains
its empty turn ID. Installation uses UUIDv4. HMAC-SHA256 provides private lookup keys, not UUIDs.
OAuth credentials for one upstream account share a namespace; separate downstream scopes remain
isolated. API-key passthrough never accesses this graph.

One private `rs_<namespace-digest>.request-state.json` file holds each account namespace. Files are
0600, atomically replaced under a namespace lock, bounded to 512 MiB and remain serialization v1.
They store identity, lineage, window/compaction and reversible schema-owned ID mappings, with no
traffic bodies or raw distribution keys. Inactive detail is eligible for 30-day pruning; active,
retained and live-context dependencies remain protected. Ancestor eviction protects descendants or
removes the complete descendant graph. Corrupt, oversized or unsupported state is preserved and
returns `state_unavailable`; final-owner credential deletion removes it without decoding it.
Duplicate credentials retain shared state until their last owner is removed.

Response ownership and ID mappings publish before created IDs become visible. Complete context,
terminal state and index membership publish before completion delivery. Output-item events and
final output are reconciled once in order. Failed/incomplete attempts never become complete context
baselines. Definition carriers allocate aliases; historical provider references must resolve a
provider-origin mapping. Translation covers schema-owned lifecycle IDs and leaves opaque values,
file/vector-store/model/connector/prompt IDs and encrypted content alone. The enumerated reversible
ID-pair exception never permits persistence of message content, arguments, credentials or workspaces.

Standard turn, prewarm and compaction metadata retains legal sandbox permission meaning, with the
runtime-specific sandbox implementation. Missing/invalid sandbox evidence uses danger-full-access /
none. Body and compatibility header use the same normalized metadata; workspaces remain unchanged.

One credential owns HTTP and WS transport contexts. HTTP uses reqwest/native-tls. Provider WS uses
AWS-LC rustls with native roots, a PQ-first key-group list and HTTP/1 without ALPN, with fresh TLS
session state on each connection. The pinned OpenAI tungstenite forks and native compression offer
remain aligned with v0.153.4. Coordinator body parsing is unnecessary for identity projection.

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

## Responses WebSocket

The additive readiness capability `capabilities.responsesWebSocket=true` enables this internal
route:

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

The coordinator validates the downstream key and dials this route before accepting its public
socket. The core validates the same loopback, internal-auth, version, account-reference, and
request-id constraints as the HTTP route. `ApiKeyPassthrough` establishes the provider socket before
returning internal `101`. Subscription returns the authenticated internal upgrade
first and waits for the first identity-projected `response.create` before establishing the provider
socket.

- One internal socket owns at most one current provider socket. Core enforces branch/turn and
  physical-socket admission; the coordinator owns public connection quotas and accounting.
- The coordinator owns the eight-per-key limit, per-turn credential revalidation and accounting,
  public overlap policy, first-frame/inter-turn/write timeouts, and shutdown lifecycle.
- Application messages are UTF-8 JSON text, bounded by configured request/output limits (128 MiB
  by default). `ApiKeyPassthrough` keeps every
  valid text frame byte-transparent. Simulated `response.inject` retains only official top-level
  `type`, `input`, and `response_id`, applies the item-schema filter to `input`, and preserves
  function/custom payload values as opaque data. Stateful Codex create/inject/control frames
  translate enumerated IDs; another typed non-create frame remains byte-exact when no ID changes.
- Subscription applies the pinned request overlay to `response.create`, preserving explicit
  supported `type`, `generate`, `previous_response_id`, `conversation`, WS-only `stream_id`, and
  current Responses fields while removing unsupported protocol members. WS create strips HTTP-only
  `background` and sends `stream: true`; HTTP strips WS-only `type`, `generate`, and `stream_id`.
  Arbitrary keys remain valid only in documented opaque/free-form containers. An
  existing non-empty
  `x-codex-ws-stream-request-start-ms` remains the CLI send time; only a missing or empty value is
  generated. After pseudonymization and any required device convergence, the complete native
  `0.153.4` prewarm shape
  keeps its empty `turn_id` and intentionally absent `root_turn_id` and
  `turn_started_at_unix_ms`; incomplete or non-native shapes are still normalized. The first OAuth
  routing hint comes from that frame, and later creates reuse the same provider socket. The deferred
  provider handshake derives its bounded `x-codex-turn-metadata` from the normalized first frame,
  so pre-upgrade header metadata cannot be combined with a native prewarm snapshot. A synthesized
  hidden prewarm independently replaces public turn identity with `request_kind=prewarm`, an empty
  turn id, and no root-turn, parent-turn, or turn-start fields; its handshake and frame metadata
  match. If hidden setup requires a replacement socket, that new handshake uses the public turn
  identity because its first frame is the public full create.
- The fingerprint snapshot used for the handshake is retained for that socket. Before each
  `response.create`, the core re-reads the sidecar revision; a changed or unreadable fingerprint
  closes the internal/public socket with empty-reason code 1012 before the create reaches upstream.
  Other valid application events do not trigger this stale-policy check; only simulated
  `response.inject` receives schema filtering. Stateful Codex response events are first observed
  with raw upstream IDs by the socket continuation, then translated and durably persisted before
  being sent downstream.
- Provider handshakes use `OpenAI-Beta: responses_websockets=2026-02-06`. Subscription auth adds
  `ChatGPT-Account-ID`; Subscription uses the runtime-derived canonical identity triplet.
  OAuth header emission retains the
  reviewed provider/extra/default/auth construction with one default-originator layout.
- The public coordinator and provider hops may negotiate per-message deflate. The provider sends
  the Codex `0.153.4` offer `permessage-deflate; client_max_window_bits`; the authenticated internal
  loopback hop does not request WebSocket compression.
- Ping, pong, close, cancellation, and backpressure remain connection-scoped. An attempted public
  inference is never silently replayed. Proven-unsent setup may use at most one replacement connection, with a full WS request and no HTTP fallback.
- A bare subscription socket may use one internal `generate=false` prewarm only when no explicit
  `generate`, `previous_response_id`, `conversation`, or `stream_id` carrier exists. Codex-marked callers
  never receive this duplicate setup. Reuse emits a delta plus the completed response id only when
  the saved model, instructions, tools, tool choice, parallel calls, reasoning, store, stream,
  include, service tier, cache key and text settings match and input starts with completed input plus
  output. `client_metadata` and `stream_options` do not participate in that settings comparison. The
  comparator clears only internal chat-message metadata; caller/provider item IDs
  remain semantic and must match. IDs temporarily synthesized for wire emission are excluded from
  the logical request snapshot, matching Codex's restore-before-baseline behavior. A semantic mismatch
  prevents automatic reuse. Explicit scoped previous-response continuations
  retain append semantics and require complete local history only for full sending or conversion.
  WebSocket application messages never use zstd.
- Gateway failures after upgrade use application close code `4500`. Its reason is compact JSON with
  exactly `retryAdvice`, `phase`, and `deliveryState`. Protocol, policy, normal lifecycle, and stale
  fingerprint closes retain their standard WebSocket codes.

A provider rejection for Subscription after the public upgrade becomes a
structured WebSocket close and cannot be surfaced as the original public HTTP handshake.
`ApiKeyPassthrough` retains the pre-upgrade provider handshake and bounded HTTP rejection mapping.

Codex `0.153.4` remote compaction v2 uses this same ordinary Responses path. Its
`compaction_trigger` input item and `request_kind=compaction` metadata pass through normal Codex
normalization, pseudonymization, and device convergence; no additional public or internal route is
required. Compaction retry markers include the projected thread plus a stable operation carrier
where one exists, so retries are idempotent while later or cross-thread compactions advance
independently. A marker first records only a pending target; the committed thread window remains
unchanged until the matching public `response.completed` is translated and durably committed before
delivery. Failed, incomplete, error, non-2xx, and disconnected operations do not advance it.
Different pending operations from one committed base converge when the first completes, and later
same-base completions are idempotent. Accessing a marker refreshes its retention timestamp and
protects it from pruning or capacity eviction during that edit.

Successful internal API-key upgrades use the same strict response-header policy and may additionally
carry the private provider request-ID header. Deferred Codex handshakes instead send a reserved
loopback text control before application output when a provider request ID is available:

```json
{"type":"mini_sub2api.provider_request_id","providerRequestId":"provider-visible-ascii"}
```

The coordinator validates and consumes this event without starting TTFB or forwarding it publicly;
the current WebSocket operation stores the value. A deferred provider rejection emits this control
when available, then close 4500 with structured failure metadata and no raw provider body. The
coordinator repeats the public header allowlist and constructs its own WebSocket handshake fields.
Cookies, forwarding fields, proxy auth, credentials, lifecycle headers, arbitrary extension
negotiation, unknown headers, and other hop-by-hop headers never cross.

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
