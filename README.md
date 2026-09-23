# mini-sub2api

Go coordinator + Rust Codex adapter for `/v1/responses` over HTTP JSON/SSE and WebSocket.
Each distribution Key binds one credential; request status, latency and usage are recorded per Key.

| Upstream | Behavior for every caller |
|---|---|
| API key | Pass bodies/valid WS frames and caller `X-Codex-Routing-Hint` through; retain gateway auth, admission and response-header policy. |
| Codex Subscription | Emulate Codex 0.156.0 for native Codex, bare API and third-party Responses clients. |

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
Linux OpenSSL bindings match Codex 0.156.0. GNU builds use system OpenSSL; the x86_64/aarch64
musl targets vendor the pinned OpenSSL 3.6.3 source. See [Linux TLS builds](docs/CODEX_COMPATIBILITY.md#linux-tls-builds).

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

Add to `$CODEX_HOME/config.toml` (default `$HOME/.codex/config.toml`):

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

Set `supports_websockets=false` for HTTP only. Other Responses clients use the same base URL/Key.
OpenCode custom providers and bare clients can associate full histories without a session ID.

## Main rules

- HTTP stays HTTP; WS stays WS. HTTP continuation requires complete local history, which expires
  after three idle hours or earlier capacity eviction. Full input can rebuild it.
- Subscription preserves caller instructions and tool order, inserts no default base, and requests
  encrypted reasoning; `include` controls public visibility. Workspace, Skills and tools belong to the client.
- Native 0.156.0 captures check JSON field order/presence and Header order/casing; see the
  [measured compatibility limits](docs/CODEX_COMPATIBILITY.md#field-order-and-presence).
- Authorized [real upstream tests](docs/CODEX_COMPATIBILITY.md#real-upstream-continuation-evidence)
  cover multi-turn memory, tool outputs and WS reconnects; long opaque routing tokens have a
  separate bounded budget and no longer hit the logical-ID length limit.
- Success requires consistent terminal/output evidence. Failed or partial output cannot become
  reusable history; uncertain sends are not automatically replayed.
- Subscription text/reasoning deltas reuse bounded, validated ID mappings; new or changed state
  still uses the persistent transaction. See [cache bounds](docs/MEMORY.md#response-identity-work).
- HTTP SSE allows 300 seconds for the first output and between subsequent outputs; status events
  and heartbeats cannot extend it. Completed streams close after a bounded tail; stalled client
  writes time out after 120 seconds. [Diagnostic logs](docs/OPERATIONS.md#diagnosing-long-requests)
  correlate request stages, model/effort, typed failures and minute-spaced progress without payloads.

Plain HTTP binds only loopback; other listeners need TLS. Run one service per state directory.
Vault/identity files are private but unencrypted; request/response bodies are not persisted.

| Guide | Contents |
|---|---|
| [Behavior](docs/BEHAVIOR.md) | Matching, compaction, instructions, limits and failure semantics |
| [Codex compatibility](docs/CODEX_COMPATIBILITY.md) | 0.156.0 wire changes and actual CLI capture evidence |
| [Operations](docs/OPERATIONS.md) | Authentication, request diagnosis, administration and deployment |
| [Memory](docs/MEMORY.md) | Small-host sizing and OOM diagnosis; context budgets are not RSS caps |
| [Protocol](src/protocol/v1/README.md) | Private coordinator/Core wire contract |
| [Architecture](docs/ARCHITECTURE.md) | Runtime ownership, state lifetimes and admission |

## Validate

```bash
bash scripts/test.sh
bash scripts/test-native-parity.sh
mise exec -- python3 scripts/prepare-opencode-tests.py
bash scripts/test-scaffold-parity.sh
```

[Capture methods and matrix](src/coordinator/integration/NATIVE_PARITY.md) distinguish native clients,
bare fixtures and real-provider evidence. Normal suites are loopback-only; real calls require separate authorization.
Source lives in `src/coordinator`, `src/core/codex` and `src/protocol/v1`; output stays in `build/`.
Local instructions, deployment overrides and `ref/` records are excluded from distribution.

Personal learning/research software; not an official OpenAI product or intended for commercial/production use.
