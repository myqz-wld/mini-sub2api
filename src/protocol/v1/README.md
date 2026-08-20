# Coordinator/Core Protocol v1

This protocol is private to one mini-sub2api deployment unit. It connects the Go coordinator to a supervised provider core over loopback HTTP. It is not a public provider API.

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
  }
}
```

The coordinator rejects a protocol version other than the exact string `1`.

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
to the Codex CLI `0.147.0` compatibility baseline. A recognized Codex product name and its suffix
are preserved; a missing or non-Codex value becomes `codex_cli_rs/0.147.0`. Regular OpenAI API-key
routes retain the public client's reviewed `User-Agent` unchanged.

## Inference response

- The core preserves the upstream status, safe end-to-end headers, JSON, and SSE bytes.
- The core adds `X-Mini-Sub2Api-Core-TTFB-Ms` after receiving upstream response headers.
- The coordinator removes internal headers, adds the public request id, and merges `upstream_ttfb;dur=<milliseconds>` into `Server-Timing`.
- Neither layer adds, removes, reorders, or mutates SSE events.
- Client cancellation cancels the internal request and upstream response body.

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
