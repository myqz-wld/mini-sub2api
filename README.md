# mini-sub2api

`mini-sub2api` is a deliberately small Responses API gateway. Clients use downstream `ms2a_…`
API keys; the service maps each key immutably to one Codex credential, forwards the request, and
records latency and token usage against the downstream key.

The deployment unit contains a Go coordinator and a supervised Rust Codex core:

```text
Responses client
  -> mini-sub2api (public listener, key auth, SQLite history)
       -> loopback-only authenticated protocol
            -> mini-sub2api-core-codex (credential vault, OAuth refresh, upstream transport)
                 -> Codex subscription backend or OpenAI Responses API
```

Only `POST /v1/responses` is public. There is no dashboard, administration HTTP API, billing,
quota engine, account pooling, Chat Completions endpoint, or request/response archive.

## Build and validate

The project pins Go 1.26.4 and Rust 1.96.0 through `mise` without changing global runtime defaults.

```bash
mise install
bash scripts/test.sh
bash scripts/build.sh
```

The installable unit is:

- `build/bin/mini-sub2api`
- `build/bin/mini-sub2api-core-codex`
- `build/bin/build-info.json`

Keep all three files in the same directory. Both binaries support `--version` and the
machine-readable `--check-installed` command. The latter reads adjacent build metadata and compares
it with the current local Git checkout; it never fetches a remote.

## Create an upstream credential

All examples use an isolated state directory. `MINI_SUB2API_STATE_DIR` can replace the global
`--state-dir` option.

Codex subscription OAuth uses device authorization by default, which works on remote/headless
servers:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal-subscription
```

Browser PKCE is available explicitly:

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential login codex --name personal-subscription --flow browser
```

The callback listens on the deployment server's loopback. For a remote deployment, prefer the
device flow. If browser PKCE is required, create an SSH local-forward for the callback port printed
in the authorization URL and open that `localhost` URL through the tunnel.

To use a regular upstream OpenAI API key, paste the key on stdin and finish with EOF. The Go
coordinator passes stdin directly to the Rust credential process; the secret is never placed in
argv, environment variables, or SQLite.

```bash
build/bin/mini-sub2api --state-dir ./state \
  credential add-api-key codex --name openai-api --secret-stdin
```

OAuth issuer/client and upstream URL overrides exist for controlled compatibility testing. Plain
HTTP override URLs are accepted only when their host is a literal loopback IP. All literal-loopback
upstream/auth URLs bypass environment proxies so their effective peer remains local.

## Create a downstream API key

List credentials, then create a key mapped to exactly one credential:

```bash
build/bin/mini-sub2api --state-dir ./state credential list

build/bin/mini-sub2api --state-dir ./state \
  key create --credential cred_EXAMPLE --name laptop
```

The full `ms2a_…` key is displayed once. The database retains only its SHA-256 hash and a short
display prefix. A key cannot be rebound; revoke it and create another key to change credentials.

```bash
build/bin/mini-sub2api --state-dir ./state key list
build/bin/mini-sub2api --state-dir ./state key revoke key_EXAMPLE --yes
```

## Start the service

The safe default is loopback HTTP:

```bash
build/bin/mini-sub2api --state-dir ./state serve
# http://127.0.0.1:8787
```

Plain HTTP is permitted only on IPv4 loopback (`127.0.0.0/8`) or IPv6 loopback (`::1`). Every
non-loopback LAN, VPC, VPN, wildcard, or public bind requires both a static certificate and key:

```bash
build/bin/mini-sub2api --state-dir ./state serve \
  --listen 192.0.2.20:8787 \
  --tls-cert ./server.crt \
  --tls-key ./server.key
```

For direct IP HTTPS, the certificate must contain that literal IP as an `iPAddress` subjectAltName
and clients must trust its issuer. mini-sub2api does not issue or renew certificates and has no
plaintext override.

A reverse proxy is optional. If one is used, terminate public HTTPS at the proxy and run
mini-sub2api on a loopback HTTP address in the same host/container network namespace. The proxy
must preserve streaming, discard client-supplied forwarding/hop-by-hop headers, and never connect
to a non-loopback plaintext mini-sub2api listener.

Request-detail retention is configurable; the default is seven days and `0` disables automatic
detail deletion:

```bash
build/bin/mini-sub2api --state-dir ./state serve --usage-retention-days 7
```

## Call the Responses API

```bash
curl --no-buffer http://127.0.0.1:8787/v1/responses \
  -H "Authorization: Bearer ms2a_EXAMPLE" \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_CODEX_MODEL","input":"Say hello","stream":true}'
```

The response adds `X-Mini-Sub2Api-Request-Id` and an `upstream_ttfb` entry in `Server-Timing`.
Upstream JSON/SSE bytes are otherwise preserved. Token usage remains in the upstream
`response.completed.response.usage` object; final normalized duration and usage are available in
CLI history.

To configure Codex as a client, add a custom Responses provider to `~/.codex/config.toml`:

```toml
[model_providers.mini-sub2api]
name = "mini-sub2api"
base_url = "http://127.0.0.1:8787/v1"
env_key = "MINI_SUB2API_API_KEY"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0

[profiles.mini-sub2api]
model_provider = "mini-sub2api"
```

Then set the downstream key only for the Codex process and select the profile:

```bash
MINI_SUB2API_API_KEY='ms2a_EXAMPLE' codex -p mini-sub2api
```

## Credentials, usage, and deletion

Credential disable is service-side and reversible; it does not contact OpenAI:

```bash
build/bin/mini-sub2api --state-dir ./state credential disable cred_EXAMPLE
build/bin/mini-sub2api --state-dir ./state credential enable cred_EXAMPLE
```

OAuth revoke is a separate, explicit upstream operation. It requires zero active downstream keys,
waits for in-flight requests, revokes upstream first, and removes service-side material only after
success. A failed revoke leaves the credential disabled and retained for retry.

```bash
build/bin/mini-sub2api --state-dir ./state credential revoke cred_EXAMPLE --yes
```

Regular upstream API keys can only be removed from mini-sub2api; provider-side key deletion remains
the operator's responsibility. Force deleting OAuth material without upstream revoke requires both
`--force-service-only` and `--yes`.

```bash
build/bin/mini-sub2api --state-dir ./state credential remove cred_EXAMPLE --yes
```

Usage commands are keyed by downstream API-key id:

```bash
build/bin/mini-sub2api --state-dir ./state usage history --key key_EXAMPLE --limit 100
build/bin/mini-sub2api --state-dir ./state usage stats --key key_EXAMPLE
build/bin/mini-sub2api --state-dir ./state usage prune --before 2026-08-01 --yes
```

Expired request details are deleted transactionally after aggregation. Daily per-key aggregates
remain indefinitely unless `usage prune` is explicitly given `--include-aggregates`. Add global
`--json` to management commands for stable machine-readable output.

## State, backup, and security

- The state root is mode `0700`; provider secrets live only in Rust-owned, atomically replaced
  `0600` vault files. The vault is access-controlled but not encrypted at rest. After removal, a
  non-secret `0600` receipt remains so an interrupted SQLite tombstone operation can be retried
  without repeating an already completed OAuth revoke.
- SQLite stores credential metadata, downstream key hashes, request timing/status, and token
  counts. It never stores provider secrets, downstream key plaintext, prompts, request bodies,
  response bodies, tool arguments, or generated content.
- The coordinator/core protocol binds only loopback and uses a random per-process bearer sent over
  stdin. Provider credentials never cross that protocol.
- Inference is never replayed for transport errors, `429`, or `5xx`. Codex OAuth permits exactly
  one refresh and one replay after a pre-response upstream `401`.
- ChatGPT subscription routing and OAuth are source-compatible with the referenced Codex
  implementation, not a documented third-party stability guarantee. Endpoint, scope, or client
  policy changes may require maintenance. Operators remain responsible for account/product terms.

For backup or restore, stop the service and copy the entire state root as one unit, preserving owner
and permissions. Do not copy a live vault: a restored rotating refresh token can be stale after the
original deployment refreshes it. One coordinator and one Codex core per state directory is the
supported topology.

## Development boundaries

All automated auth and inference tests bind literal loopback mock endpoints. They never contact
OpenAI, ChatGPT, or Codex services. The approved implementation plan is archived under `ref/plans/`
when the release review is complete.
