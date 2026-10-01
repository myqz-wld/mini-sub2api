# mini-sub2api

A small Responses API gateway for Codex subscriptions and API keys, built with Go and Rust.
Supports HTTP/SSE, WebSocket, credential-bound access keys and usage tracking.
Subscription compatibility targets Codex **0.159.2**, including history prewarm and scoped Lite interruption.

## Quick start

Install the project runtimes with [mise](https://mise.jdx.dev/), then build and start:

```bash
mise install
bash scripts/build.sh

export MINI_SUB2API_STATE_DIR=./state
build/bin/mini-sub2api credential login codex --name personal
build/bin/mini-sub2api credential list
build/bin/mini-sub2api key create --credential cred_EXAMPLE --name laptop
build/bin/mini-sub2api serve
```

Replace `cred_EXAMPLE` with the credential ID from `credential list`.
Save the generated `ms2a_` key; it is shown only once.

The default listener is `127.0.0.1:8787`. Remote access requires
[TLS configuration](docs/OPERATIONS.md#deployment).

## Connect a client

Use `http://127.0.0.1:8787/v1` as the base URL and your `ms2a_` key for authentication.
For the Codex CLI, follow the [client setup](docs/OPERATIONS.md#codex-client).

Guardian auto-review preserves its non-strict output schema when `guardian_review` turn
metadata identifies the request, including custom providers without the reviewer header.

Complete anonymous user/agent-message and tool history can recover ordinary turns and compaction after
cached history expires or Core restarts; the same [history validation rules](docs/BEHAVIOR.md#state-and-limits) apply.
Agent `NEW_TASK` inputs start an inferred turn when no explicit turn ID or pending tool call
keeps the current turn active. V2 compaction preserves eligible agent messages for later
response-reference continuation using the native message-kind and per-item size rules.

After a failed WebSocket response, open a new connection for subsequent work.
The gateway retires the failed stream and never automatically replays its request;
a matching failure footer may supply usage for up to one second before closure.
Failure-tail accounting does not wait for the client to acknowledge closure.
Upstream `x-retry-metadata`, including `NO_MORE_RETRY`, survives in response headers
and streamed error events so clients retain the upstream retry signal.

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H 'Authorization: Bearer ms2a_EXAMPLE' -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

## Development

```bash
bash scripts/test.sh
```

Build output stays in `build/`. See the [capture tests](src/coordinator/integration/NATIVE_PARITY.md)
for native Codex and third-party client validation.

## Documentation

- [Operations](docs/OPERATIONS.md): credentials, administration and deployment
- [Behavior](docs/BEHAVIOR.md): request handling, history and limits
- [Codex compatibility](docs/CODEX_COMPATIBILITY.md): version alignment and validation
- [Memory](docs/MEMORY.md): sizing and diagnostics
- [Architecture](docs/ARCHITECTURE.md) · [Internal protocol](src/protocol/v1/README.md)

Personal learning/research software; not an official OpenAI product or intended for commercial/production use.
