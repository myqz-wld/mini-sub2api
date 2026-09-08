# mini-sub2api

Go coordinator + Rust Codex adapter for `/v1/responses` over HTTP JSON/SSE and WebSocket.
Each distribution Key binds one credential; request status, latency and usage are recorded per Key.

| Upstream | Behavior for every caller |
|---|---|
| API key | Pass bodies/valid WS frames and caller `X-Codex-Routing-Hint` through; retain gateway auth, admission and response-header policy. |
| Codex Subscription | Emulate Codex 0.153.4 for native Codex, bare API and third-party Responses clients. |

No account pool, automatic switching, Chat Completions or admin HTTP API.

## Build

Go 1.26.4 and Rust 1.96.0 are pinned through mise:

```bash
mise install
bash scripts/build.sh
build/bin/mini-sub2api --version
build/bin/mini-sub2api --check-installed
```

Ship both binaries and build-info.json from build/bin together. Installed checks never fetch Git remotes.

## Start

```bash
export MINI_SUB2API_STATE_DIR=./state
build/bin/mini-sub2api credential login codex --name personal
# Alternative: credential add-api-key codex --name openai-api --secret-stdin
build/bin/mini-sub2api credential list
build/bin/mini-sub2api key create --credential cred_EXAMPLE --name laptop
build/bin/mini-sub2api serve
```

The downstream ms2a_ key is shown once. The default listener is 127.0.0.1:8787:

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H 'Authorization: Bearer ms2a_EXAMPLE' -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

## Codex client

Add to ~/.codex/config.toml:

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

Set supports_websockets=false for HTTP only. Other Responses clients use the same base URL/Key.
OpenCode custom-provider requests and bare full histories can associate without a session ID;
its enabled built-in OpenAI plugin supplies session-id. See [behavior](docs/BEHAVIOR.md) for matching rules.

## Main rules

- HTTP stays HTTP; WS stays WS. Subscription HTTP increments require complete local history;
  WS reuse separately checks the actual settings, input/output and connection.
- Full-history association survives configuration changes. An exact verified anonymous compaction
  item can restore its session/thread/window; accepted replacement windows support later increments.
- Full history expires after three idle hours. Full input can rebuild it; valid live WS references
  and persisted identities have separate lifetimes.
- Subscription always requests encrypted reasoning. Caller `include` controls public visibility;
  Core retains it and restores verified fields it previously hid from matched histories.
- Preserve valid caller base instructions, developer order and duplicates; never insert a default
  base. Subscription maps system→developer in place. Lite uses scoped deterministic prefix IDs.
- Streaming success requires a terminal event. Truncated streams report failure; body-changing WS
  controls invalidate stale local history.
- Workspace/Skills/tools belong to the client. Core does not discover or execute them; native code
  mode and ordinary direct tools keep their respective protocols.

Plain HTTP binds only loopback; other listeners need TLS. Run one service per state directory.
Vault/identity files are private but unencrypted; request/response bodies are not persisted.
[Operations](docs/OPERATIONS.md) covers auth, administration and deployment.

## Validate

```bash
bash scripts/test.sh
bash scripts/test-native-parity.sh
mise exec -- python3 scripts/prepare-opencode-tests.py
bash scripts/test-scaffold-parity.sh
```

[Capture methods and matrix](src/coordinator/integration/NATIVE_PARITY.md) distinguish native clients,
bare fixtures and real-provider evidence. Normal suites are loopback-only; real calls require separate authorization.
Source lives in src/coordinator, src/core/codex and src/protocol/v1; generated output stays in build/.
The local ignored USER_POLICIES.md records selected policies and historical capture examples.

Personal learning/research software; not an official OpenAI product or intended for commercial/production use.
