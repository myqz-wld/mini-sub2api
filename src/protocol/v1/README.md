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
- `user-agent`
- `version`
- `openai-beta`
- `openai-organization`
- `openai-project`
- `x-client-request-id`
- `x-codex-beta-features`
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

The core constructs authoritative `Authorization` and `Host` headers. It separately derives caller
source from a syntactically valid, non-empty `Originator` and credential kind from the authenticated
internal account reference:

| Caller source | Credential kind | Profile |
| --- | --- | --- |
| No valid `Originator` | OpenAI API key | `BareOpenAi` |
| Valid `Originator` | OpenAI API key | `CodexOpenAi149` |
| Any source | Codex subscription | `CodexSubscription149` |

`Originator` selects formatting only. It never grants credential/account permissions or changes the
selected credential. `BareOpenAi` preserves reviewed HTTP body and WebSocket text-frame bytes.
Both Codex profiles make a minimal `0.149.0` overlay while retaining OpenAI API-key versus ChatGPT
subscription authentication boundaries. They replace `User-Agent`, `originator`, and `version` with
one runtime-derived canonical identity. Only `CodexSubscription149` constructs
`ChatGPT-Account-ID`. `OpenAI-Organization`, `OpenAI-Project`, and the explicitly reviewed
`X-Stainless-*` headers reach only API-key upstreams. Both layers remove cookies, proxy
authentication, forwarding headers, content length, transfer encoding, connection-specific
headers, unknown `X-Stainless-*` headers, and any unreviewed `X-Mini-Sub2Api-*` header.

Codex emulation begins with the full caller object, not a top-level field whitelist. Except for the
target-specific compatibility exceptions below, explicit Responses fields—including
`previous_response_id`, `conversation`, HTTP `background`, WS
`stream_id`,
`context_management`, output/tool limits, metadata, moderation, prompt/cache settings, safety
identifiers, sampling, truncation, tools, and image detail—remain authoritative. The supported
surface is the union of documented OpenAI Responses fields and completed Codex `0.149.0`
capture/source wire fields; other protocol members are removed. Arbitrary keys remain valid inside
documented opaque/free-form containers such as metadata maps, JSON Schema, `prompt.variables`, and
function/custom tool-call argument/input/output payload values; structured tools and shell/computer
outputs are still filtered. Web Search accepts only its domain-filter schema; File Search accepts
its recursive attribute-filter schema.
The overlay changes only required structure or absent defaults: API-key profiles keep explicit
`system` and `developer` roles distinct, while `CodexSubscription149` maps message role `system` to
the subscription-compatible `developer` role without changing content or ordering. String message
content becomes `input_text`, synthesized Lite instructions use `developer`, and historical
assistant strings become `output_text`. It never generates an HTTP `previous_response_id`; an
explicit one is forwarded unchanged. Non-Lite requests default omitted image detail to `high`;
Responses Lite leaves omitted detail absent while preserving an explicit supported detail value.
`CodexSubscription149` additionally removes `max_output_tokens`, `temperature`, and `top_p`, which
are public Responses controls but are rejected by the fixed Subscription target. API-key profiles
retain them.

For both Codex-emulated profiles, the core replaces the complete client identity with
`User-Agent: codex_cli_rs/0.149.0 (<runtime OS>; <runtime architecture>) <runtime terminal>`,
`originator: codex_cli_rs`, and `version: 0.149.0`. OS, architecture, and terminal are captured on
first use once per core process. The same normalization function feeds HTTP and WebSocket construction; no inbound
product, platform suffix, originator, or version survives. `CodexSubscription149` HTTP additionally
pins `Accept: text/event-stream`,
`Content-Type: application/json`, and level-3 zstd. `CodexOpenAi149` HTTP is deliberately not
zstd-compressed and retains API-key authentication; it must not inherit subscription-only account
or identity permissions.

The reviewed optional Codex request names `x-openai-internal-codex-residency`,
`x-responsesapi-include-timing-metrics`, `x-codex-inference-call-id`, `x-oai-attestation`, and
`x-openai-memgen-request` cross both Go filters. The core keeps timing WebSocket-only and inference
call IDs HTTP-only on OAuth routes, with the conditional raw order captured from `0.149.0`.

## Stateless pseudonymization and optional device convergence

Subscription identity is projected in two independent layers. First, every parseable subscription request
uses HMAC-SHA256 over the stable ChatGPT account namespace, downstream pseudonym scope, field domain,
and original identifier. The first 128 bits are emitted as UUIDv8. This rewrites installation,
session, thread, turn/root/parent-turn, window/parent-thread, client-request, item turn metadata, and
prompt-cache carriers consistently across headers, `client_metadata`, and serialized turn metadata.
The same account, downstream key, field, and source ID therefore produce the same result on every
machine; changing the account or downstream key produces a different namespace. Unknown metadata,
compaction state, subagent state, timestamps, and upstream response/item IDs are preserved.

Second, `device` replaces only the already-pseudonymized installation carrier with an account-level
UUIDv8 derived solely from the ChatGPT account namespace. `off` keeps the one-to-one installation
pseudonym instead. The fingerprint sidecar stores only mode and revision; no installation ID or
random pseudonym seed is persisted. New sidecars default to `device`. `BareOpenAi` HTTP bodies and
WebSocket application frames remain byte-exact in both modes; `CodexOpenAi149` is emulated without
subscription identity projection.

One credential owns independent HTTP and WebSocket connection pools. HTTP explicitly selects
reqwest/native-tls from the deployment runtime; provider WebSocket uses AWS-LC rustls with native
roots, a PQ-first key-group list, and an
HTTP/1 handshake without an ALPN offer. These fixed builders are the Codex `0.149.0` compatibility
split and do not vary by credential; TLS session state is fresh for every provider WebSocket.
Provider WebSockets use the same pinned OpenAI
`tokio-tungstenite`/`tungstenite` fork revisions and per-message-deflate offer as that release. Pool
selection and identity projection require no coordinator parsing of request bodies or WebSocket
frames.

## Inference response

- The core preserves the upstream status, safe end-to-end headers, JSON, and SSE bytes.
- The core adds `X-Mini-Sub2Api-Core-TTFB-Ms` after receiving upstream response headers.
- Every streamed response declares the three failure trailers. They remain empty on clean EOF; a
  provider-body failure emits `X-Mini-Sub2Api-Failure-Phase`,
  `X-Mini-Sub2Api-Delivery-State`, and `X-Mini-Sub2Api-Retry-Advice` as the final trailer block.
- The coordinator removes internal headers, adds the public request id, and merges `upstream_ttfb;dur=<milliseconds>` into `Server-Timing`.
- The coordinator validates and republishes a failure trailer block without inserting bytes into
  the JSON/SSE body.
- Neither layer adds, removes, reorders, or mutates SSE events.
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
request-id constraints as the HTTP route. API-key credentials establish the provider socket before
returning internal `101`; subscription credentials return the authenticated upgrade first and wait
for the first `response.create` so its model and service tier can drive the provider handshake.

- One internal socket owns exactly one provider socket. The core does not pool sockets, count
  tenant/key connections, schedule turns, or enforce active-response admission.
- The coordinator exclusively owns the eight-per-key limit, per-turn revalidation and accounting,
  overlap policy, first-frame/inter-turn/write timeouts, and shutdown lifecycle.
- Application messages are UTF-8 JSON text and are limited to 16 MiB. `BareOpenAi` keeps every
  valid text frame byte-transparent. Simulated `response.inject` retains only official top-level
  `type`, `input`, and `response_id`, applies the item-schema filter to `input`, and preserves
  function/custom payload values as opaque data. Other typed non-create events remain unchanged.
- Both Codex profiles apply the pinned request overlay to `response.create`, preserving explicit
  supported `type`, `generate`, `previous_response_id`, `conversation`, WS-only `stream_id`, and
  current Responses fields while removing unsupported protocol members. WS create strips HTTP-only
  `background` and implicit `stream`; HTTP strips WS-only `type`, `generate`, and `stream_id`.
  Arbitrary keys remain valid only in documented opaque/free-form containers. An
  existing non-empty
  `x-codex-ws-stream-request-start-ms` remains the CLI send time; only a missing or empty value is
  generated. After pseudonymization and any required device convergence, the complete native
  `0.149.0` prewarm shape
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
  `response.inject` receives schema filtering instead of byte-exact relay.
- Provider handshakes use `OpenAI-Beta: responses_websockets=2026-02-06`. Subscription auth adds
  `ChatGPT-Account-ID`; both Codex profiles use the runtime-derived canonical identity triplet.
  OAuth header emission retains the
  reviewed provider/extra/default/auth construction with one default-originator layout.
- The public coordinator and provider hops may negotiate per-message deflate. The provider sends
  the Codex `0.149.0` offer `permessage-deflate; client_max_window_bits`; the authenticated internal
  loopback hop does not request WebSocket compression.
- Ping, pong, close, cancellation, and backpressure remain connection-scoped. Neither layer
  reconnects or replays an active turn.
- A bare subscription socket may use one internal `generate=false` prewarm only when no explicit
  `generate`, `previous_response_id`, `conversation`, or `stream_id` carrier exists. Codex callers
  never receive this duplicate setup. Reuse emits a delta plus the completed response id only when
  every semantic property in the Codex `0.149.0` reuse projection and the completed input/output
  prefix match. The comparator clears only internal chat-message metadata; caller/provider item IDs
  remain semantic and must match. IDs temporarily synthesized for wire emission are excluded from
  the logical request snapshot, matching Codex's restore-before-baseline behavior. Tool-output
  continuation and any true semantic mismatch emit a full frame without an inferred previous id.
  WebSocket application messages never use zstd.
- Gateway failures after upgrade use application close code `4500`. Its reason is compact JSON with
  exactly `retryAdvice`, `phase`, and `deliveryState`. Protocol, policy, normal lifecycle, and stale
  fingerprint closes retain their standard WebSocket codes.

A subscription provider rejection after the public upgrade becomes a WebSocket close and cannot be
surfaced as the original public HTTP handshake. API-key credentials retain pre-upgrade provider
handshakes and bounded HTTP rejection mapping.

Codex `0.149.0` remote compaction v2 uses this same ordinary Responses path. Its
`compaction_trigger` input item and `request_kind=compaction` metadata pass through normal
subscription normalization, pseudonymization, and device convergence; no additional public or internal route is
required.

Successful internal upgrades may expose only `openai-model`, `x-codex-turn-state`,
`x-models-etag`, `x-reasoning-included`, `x-request-id`, and the core TTFB header to the
coordinator. The coordinator applies a narrower public allowlist and constructs its own WebSocket
handshake fields. Non-101 text/JSON bodies are bounded; cookies, forwarding fields, proxy auth,
credentials, arbitrary extension negotiation, and other hop-by-hop headers never cross.

## Internal errors

Before any upstream response bytes are sent, core errors use the JSON shape in `fixtures/error.json` with one of these codes:

- `invalid_internal_auth`
- `unsupported_protocol`
- `invalid_request`
- `unknown_account`
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
idle after a terminal event. A deferred OAuth handshake failure occurs before inference delivery.

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
