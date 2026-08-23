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
X-Mini-Sub2Api-Request-Id: req_<opaque-id>
Content-Type: application/json
```

- The TCP peer must be loopback.
- `Authorization` must match the startup token.
- Account references and request ids are opaque ASCII values, 1-128 characters after their prefix.
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
- `openai-beta`
- `openai-organization`
- `openai-project`
- `x-client-request-id`
- `x-codex-beta-features`
- `x-codex-turn-state`
- `x-codex-turn-metadata`
- `x-codex-parent-thread-id`
- `x-codex-window-id`
- `x-codex-installation-id`
- `x-openai-internal-codex-responses-lite`
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

The core constructs authoritative `Authorization` and `Host` headers. For subscription routes it
also constructs `ChatGPT-Account-ID` and supplies `originator` when missing. `OpenAI-Organization`,
`OpenAI-Project`, and the explicitly reviewed `X-Stainless-*` headers reach only regular OpenAI
API-key upstreams. Both layers remove cookies, proxy authentication, forwarding headers, content
length, transfer encoding, connection-specific headers, unknown `X-Stainless-*` headers, and any
unreviewed `X-Mini-Sub2Api-*` header.

For subscription-backed plain Responses bodies, the core maps input messages with role `system` to
role `developer`, preserving their content and instruction precedence for the internal Codex
endpoint. Regular OpenAI API-key request bodies remain byte-transparent.

For subscription upstreams, the core anchors the leading Codex `User-Agent` product/version token
to the Codex CLI `0.149.0` compatibility baseline. A recognized Codex product name and its suffix
are preserved; a missing or non-Codex value becomes `codex_cli_rs/0.149.0`. Regular OpenAI API-key
routes retain the public client's reviewed `User-Agent` unchanged.

## Credential-scoped device projection

The runtime protocol still carries only the opaque `X-Mini-Sub2Api-Account-Ref`; fingerprint mode
and installation identity never become v1 fields. The core resolves them from a private,
versioned sidecar for that credential. Both new and legacy credentials default to `device` unless
the operator explicitly selected `off`.

In `device`, the core sets `x-codex-installation-id` and converges recognized installation values
inside `client_metadata` and serialized `x-codex-turn-metadata`. It changes only the
`installation_id` member of turn metadata, preserving session, thread, turn, window, compaction,
subagent, and unknown future members. Carrier-free API-key bodies stay byte-exact. Unsafe encoded,
malformed, non-object, or oversized device projections fail before an upstream send. In `off`, the
core preserves caller-provided installation carriers under the existing auth-specific behavior.

One credential owns independent HTTP and WebSocket connection pools. HTTP uses transport-default
TLS; provider WebSocket uses AWS-LC rustls with native roots and HTTP/1. These fixed builders are
the Codex `0.149.0` compatibility split and do not vary by credential. Pool selection and device
projection require no coordinator parsing of request bodies or WebSocket frames.

## Inference response

- The core preserves the upstream status, safe end-to-end headers, JSON, and SSE bytes.
- The core adds `X-Mini-Sub2Api-Core-TTFB-Ms` after receiving upstream response headers.
- The coordinator removes internal headers, adds the public request id, and merges `upstream_ttfb;dur=<milliseconds>` into `Server-Timing`.
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
X-Mini-Sub2Api-Request-Id: req_<opaque-connection-id>
Connection: Upgrade
Upgrade: websocket
```

The coordinator validates the downstream key and dials this route before accepting its public
socket. The core validates the same loopback, internal-auth, version, account-reference, and
request-id constraints as the HTTP route, resolves vault-owned auth, and establishes the provider
WebSocket before returning internal `101 Switching Protocols`. Consequently a non-101 provider
response remains an HTTP handshake response all the way to the public client.

- One internal socket owns exactly one provider socket. The core does not pool sockets, count
  tenant/key connections, schedule turns, or enforce active-response admission.
- The coordinator exclusively owns the eight-per-key limit, per-turn revalidation and accounting,
  overlap policy, first-frame/inter-turn/write timeouts, and shutdown lifecycle.
- Application messages are UTF-8 JSON text and are limited to 16 MiB. The core relays valid
  non-create events without semantic translation.
- Regular API-key text frames are byte-transparent in `off`, and remain byte-transparent in
  `device` when they have no recognized body carrier. For subscription credentials, the core
  applies the HTTP compatibility normalizer only to `response.create`, preserving `type`,
  `generate`, `previous_response_id`, and `client_metadata`; device projection then converges any
  recognized installation carriers.
- The fingerprint snapshot used for the handshake is retained for that socket. Before each
  `response.create`, the core re-reads the sidecar revision; a changed or unreadable fingerprint
  closes the internal/public socket with empty-reason code 1012 before the create reaches upstream.
  Other valid application events do not trigger this stale-policy check and remain byte-exact.
- Provider handshakes use `OpenAI-Beta: responses_websockets=2026-02-06`. Subscription auth adds
  `ChatGPT-Account-ID`, supplies `originator` when absent, and keeps the reviewed `0.149.0`
  User-Agent anchor.
- The public coordinator hop may negotiate per-message deflate. The internal and provider hops do
  not request WebSocket compression.
- Ping, pong, close, cancellation, and backpressure remain connection-scoped. Neither layer
  reconnects or replays an active turn.

Codex `0.149.0` remote compaction v2 uses this same ordinary Responses path. Its
`compaction_trigger` input item and `request_kind=compaction` metadata pass through normal
subscription normalization and device projection; no additional public or internal route is
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
- `upstream_auth_failed`
- `internal_error`

The coordinator maps these to a stable OpenAI-shaped public error and never exposes account existence, filesystem paths, credentials, auth endpoint bodies, or internal process details.

## Retry contract

The core does not retry transport, `429`, or `5xx` inference failures. It may perform exactly one forced credential refresh and one replay after an upstream `401`, only before any response bytes have reached the coordinator.

## Compatibility changes

Any incompatible change creates a new `src/protocol/vN/` directory. v1 fixtures are immutable after release except for additive examples that do not change decoding requirements.
