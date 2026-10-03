# mini-sub2api

A lightweight Responses API gateway for Codex subscriptions and API keys.
Supports HTTP/SSE, WebSocket and usage tracking.

Subscription WebSockets connect upstream after the first valid request, without a separate
handshake probe. See [WebSocket metadata limits](docs/CODEX_COMPATIBILITY.md#subscription-websocket-upgrade-metadata).

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

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H 'Authorization: Bearer ms2a_EXAMPLE' -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

## Development

Run `bash scripts/test.sh` for local checks.

## Documentation

- [Operations](docs/OPERATIONS.md): credentials, administration and deployment
- [Behavior](docs/BEHAVIOR.md): request handling, history and limits
- [Codex compatibility](docs/CODEX_COMPATIBILITY.md): version alignment and validation

Personal learning/research software; not an official OpenAI product or intended for commercial/production use.
