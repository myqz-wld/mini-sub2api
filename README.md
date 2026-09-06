# mini-sub2api

A small Responses API gateway with a Go coordinator and Rust Codex adapter. Each downstream key
binds to one Codex Subscription or OpenAI API-key credential. It supports HTTP JSON/SSE and
WebSocket `/v1/responses`, with per-key request status, latency and token usage.

| Upstream credential | Behavior for every caller |
|---|---|
| API key | Pass request/response bodies and valid WS application frames through unchanged. Authentication, admission and response-header policy still apply. |
| Codex Subscription | Emulate Codex v0.153.4, including request format, defaults and scoped identities. Native Codex, bare API and third-party Responses clients are supported. |

There is no account pool, automatic account switching, Chat Completions endpoint or administration HTTP API.

## Build

Go 1.26.4 and Rust 1.96.0 are pinned through `mise`:

```bash
mise install
bash scripts/build.sh
build/bin/mini-sub2api --version
build/bin/mini-sub2api --check-installed
```

Deploy all three files together: `mini-sub2api`, `mini-sub2api-core-codex` and `build-info.json`
from `build/bin/`. The installed check returns JSON and never fetches a remote repository.

## Quick start

Add a Subscription credential:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal
```

Or add an API key through standard input:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential add-api-key codex --name openai-api --secret-stdin
```

List credentials, then create a downstream key using the returned credential ID:

```bash
build/bin/mini-sub2api --state-dir ./state credential list
build/bin/mini-sub2api --state-dir ./state \
  key create --credential cred_EXAMPLE --name laptop
build/bin/mini-sub2api --state-dir ./state serve
```

The `ms2a_…` key is shown once. The default listener is `127.0.0.1:8787`:

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H 'Authorization: Bearer ms2a_EXAMPLE' \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

## Use with Codex

Add this provider to `~/.codex/config.toml`:

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

Set `supports_websockets = false` for HTTP-only use. Other Responses clients can use the same
base URL and downstream key. OpenCode's tested Responses provider uses HTTP.

## Context and instructions

- HTTP stays HTTP; WS stays WS. Subscription HTTP increments require complete local history
  and become full upstream requests. Missing history fails before inference.
- Full history expires after three idle hours. A full request can rebuild it; valid live WS
  references may continue without the expired local body. Identity mappings have separate retention.
- Valid nonblank caller base instructions win verbatim; absent or invalid ordinary bases use model
  defaults. Developer content, duplicates and order are preserved; Subscription changes system to developer in place.
- Lite conversion moves tools and the selected base into the input prefix. Native Lite prefixes
  follow the pinned thread/content UUIDv5 rules. Core does not discover caller workspaces, Skills
  or tools. Native code mode and ordinary direct tools retain their respective execution protocols.

See [gateway behavior](docs/BEHAVIOR.md) for session/turn ownership, device convergence, identity
privacy, limits, compaction and recovery; [operations](docs/OPERATIONS.md) covers authentication,
administration, retention and deployment.

## Security

Plain HTTP may bind only to loopback. Other listeners require native TLS; an optional reverse proxy
must forward to deployment-local loopback and preserve streaming/WS upgrades. Run one service per
state directory. Vault and identity files are private but **not encrypted at rest**. Downstream
keys are hashed; request/response bodies are not persisted.

## Validation

```bash
bash scripts/test.sh                         # Go/Rust, race, vet, Clippy, formatting
bash scripts/test-native-parity.sh           # Pinned Codex 0.153.4; loopback captures
mise exec -- python scripts/prepare-opencode-tests.py
bash scripts/test-scaffold-parity.sh         # Pinned OpenCode 1.18.29; loopback captures
```

The native suite requires the exact source reference described in the
[capture method and coverage matrix](src/coordinator/integration/NATIVE_PARITY.md). Prerequisite
mismatches fail explicitly. Default suites never call real providers. Separately authorized real
Subscription tests use `bash scripts/test-live-parity.sh --allow-real-subscription`; they send bounded
synthetic requests and require an existing login. Real API-key validation is not covered.

Source: `src/coordinator/` (public service/CLI/storage), `src/core/codex/` (adapter/vault),
`src/protocol/v1/` (internal contract). Generated artifacts stay in `build/`. The local, Git-ignored
`USER_POLICIES.md` contains the user's selected policies and sanitized capture examples.

## Disclaimer

This project is for personal learning and research. It is not an official OpenAI product and is not
intended for commercial or production use. Users are responsible for applicable laws and service terms.
