# mini-sub2api Initial Service Plan

## Identity and status

- Plan ID: `MS2A-PLAN-001`
- Status: `completed`
- Approved at: `2026-08-18`
- Created at: `2026-08-18`
- Completed at: `2026-08-18`
- Target repository: `/Users/wanglidong/Repository/mini-sub2api`
- Completion baseline: `93220c57166280467ed16931aea43651c33f6cd1`
- Isolation: the new repository is isolated from neighboring repositories
- Planning source evidence: `/Users/wanglidong/Repository/codex` at commit
  `e597169e9a783156e50ae9765d891a3dd74df064`
- Archived plan path:
  `/Users/wanglidong/Repository/mini-sub2api/ref/plans/recent-3-days/PLAN_1_mini-sub2api-initial-service.md`

## Goal

Build a minimal sub2api service with a future-facing adapter boundary:

1. A public service listens on a configurable IP and port, including loopback.
2. Clients authenticate with downstream API keys issued by mini-sub2api.
3. Each downstream key resolves deterministically to one upstream credential/account.
4. The first core adapter is Codex, written in Rust. It forwards OpenAI Responses requests either
   to the Codex subscription backend with ChatGPT OAuth material or to the regular OpenAI API with
   an upstream API key.
5. Request history and token statistics are attributed to the downstream API key that made the
   request.
6. A basic CLI manages credentials/accounts, downstream keys, and usage history.
7. The design can add future application-specific core adapters without rewriting the public
   gateway or key/account management.

## Invariants

- Keep the product minimal: no dashboard, billing, quota engine, user roles, multi-tenant UI,
  prompt archive, response archive, or unrelated OpenAI-compatible endpoints in the first release.
- Never persist request bodies, response bodies, prompts, tool arguments, or generated content.
- Never store a recoverable copy of a downstream API key after creation; store only a high-entropy
  key identifier/prefix and a deterministic cryptographic hash.
- Every request and aggregate usage row remains attributable to one downstream API-key record,
  even when several keys map to the same OAuth account.
- Do not send real requests while implementing or testing in this workspace. All transport tests
  use loopback mock upstreams.
- Do not print, trace, snapshot, or include upstream/downstream credentials in errors or test
  fixtures.
- Public listening defaults to loopback. Binding a non-loopback IP must be explicit.
- The Codex adapter owns upstream request construction and transport. The coordinator owns public
  authentication, key/account resolution, process coordination, and usage persistence.
- Runtime operations that are exact (key lookup, hashing, routing, aggregation, persistence,
  request forwarding, and schema validation) are deterministic code, not LLM work.

## Confirmed scope and non-goals

### Confirmed

- New project under `/Users/wanglidong/Repository/mini-sub2api`.
- Full `project-engineering-foundation` setup after the implementation plan is approved.
- Codex core adapter implemented in Rust.
- Non-core coordination language delegated to engineering selection.
- Both Codex subscription OAuth and regular upstream OpenAI API-key credentials are supported.
- Downstream service is reachable at a configurable IP:port.
- Usage history and statistics use downstream API key as the primary dimension.
- Offline-only sandbox comparison; no real Codex/OpenAI request is authorized during development.

### Non-goals for the first release

- Acquiring ChatGPT OAuth credentials through a new login flow.
- Browser UI or HTTP administration API.
- Billing, charging, key-level budgets, rate limiting, or subscription pooling algorithms.
- Load balancing one key across multiple upstream accounts.
- Automatic model selection or prompt/body rewriting beyond the exact Codex adapter contract.
- Supporting providers/applications other than Codex.
- Persisting raw request or response content.

## Project evidence

### Codex authentication and routes

- `codex-rs/protocol/src/auth.rs` distinguishes regular API-key auth from ChatGPT auth and marks
  ChatGPT auth as using the Codex backend.
- `codex-rs/model-provider-info/src/lib.rs` uses
  `https://chatgpt.com/backend-api/codex` for Codex-backend auth and
  `https://api.openai.com/v1` for regular API-key auth.
- `codex-rs/model-provider/src/bearer_auth_provider.rs` applies
  `Authorization: Bearer <token>` for both modes and adds `ChatGPT-Account-ID` for a ChatGPT
  account when present.
- `codex-rs/core/src/client.rs` posts Responses requests to `/responses`; normal OpenAI and Codex
  routes use `store: false`, `stream: true` for Codex CLI turns, and include usage in the upstream
  `response.completed` event.
- `codex-rs/login/src/auth/storage.rs` shows the current `auth.json` shape: auth mode, optional
  `OPENAI_API_KEY`, and token data including access token, refresh token, and account id.
- Current official OpenAI documentation confirms that Codex clients support browser-based ChatGPT
  sign-in and regular API-key sign-in, but it does not publish a third-party OAuth client
  registration/stability contract for applications that reproduce the Codex login flow.
- Current official OpenAI documentation separately exposes Codex access tokens for trusted
  non-interactive workflows, but only for ChatGPT Business and Enterprise workspaces; those tokens
  are manually rotated credentials rather than consumer OAuth refresh tokens.

### Usage and timing

- `codex-rs/codex-api/src/sse/responses.rs` parses usage from `response.completed.response.usage`,
  including input, cached input, cache-write input, output, reasoning output, and total tokens.
- `codex-rs/core/src/client.rs` measures time to first output item and can request upstream timing
  metrics. A proxy can preserve upstream usage while independently measuring gateway/core TTFB and
  end-to-end duration.

### Existing minimal proxy precedent

- `codex-rs/responses-api-proxy` accepts only `POST /v1/responses`, replaces incoming
  authorization, forwards the body, and preserves streaming. This validates a narrow Responses-only
  proxy surface as a reasonable starting point.

### Local runtime evidence

- User-managed defaults are Go `1.26.4`, Node.js `22.22.3`, and Rust `1.96.0` through `mise`.
- Node.js `node:sqlite` works locally but emits an experimental-feature warning, making it a weaker
  foundation for durable credential/usage storage.
- Go provides a small static coordination binary and strong standard-library HTTP/process support;
  its SQLite driver would be the only material coordination-layer runtime dependency.

## Blindspot pass

1. The ChatGPT Codex backend is an implementation endpoint evidenced by Codex source, not a general
   OpenAI API compatibility promise. Keep it adapter-local and configurable; do not expose it as a
   generic provider abstraction.
2. OAuth access tokens expire. A service that copies one access token without a refresh/source
   policy will stop working. The first release needs an explicit credential-source contract.
   Codex source contains enough implementation detail to reproduce its current flow, but official
   documentation does not promise that flow as a stable third-party OAuth integration surface.
3. Multiple downstream keys sharing a subscription can have product/terms implications outside the
   code. The service will provide attribution, not claim that sharing is authorized for every
   account or deployment.
4. Streaming HTTP headers are committed before the stream completes. Full duration and final token
   totals cannot be added as ordinary response headers after completion. The metrics contract must
   distinguish TTFB, upstream usage in the SSE body, and persisted final duration.
5. A multi-language adapter boundary creates an internal authentication and supervision boundary.
   Internal endpoints must be loopback-only, authenticated with an ephemeral secret, and redact
   sensitive headers. Raw upstream credentials stay in the Rust adapter vault and never cross this
   boundary.
6. A CLI and a running server may update storage concurrently. The storage engine must provide
   transactions and locking; ad-hoc JSON state is not sufficient.
7. The meaning of “pure API key request” must be fixed: regular OpenAI Responses upstream auth is
   different from adding Chat Completions compatibility.
8. Automated detail retention, permanent aggregate retention, and explicit aggregate deletion must
   use an injected clock and transactional ordering so pruning cannot race aggregation.
9. Any non-loopback listener must be native HTTPS. Certificate identity/trust and renewal remain
   operator responsibilities; an API key alone never provides transport confidentiality.
10. The implementation must not silently fall back from an unavailable Codex core adapter to the
    regular OpenAI API-key path or vice versa.
11. A browser PKCE callback bound on the deployed server's loopback is not directly reachable from
    an operator's browser on another machine. Remote/headless deployments need device-code login or
    an explicit SSH callback tunnel.

## Candidate routes

### Route A — Go coordinator supervising a deployment-local Rust Codex adapter (selected)

- The Go coordinator owns the public IP:port listener, downstream API-key authentication, account
  lookup, SQLite storage, CLI, usage attribution, and adapter lifecycle.
- `serve` starts the Rust Codex adapter on an ephemeral loopback port and authenticates the private
  link with a random per-process secret.
- The coordinator sends a provider-owned account reference plus allowed request headers/body to the
  adapter. The Rust Codex adapter owns Codex OAuth secrets, refresh locking, upstream request
  construction, and transport; regular API-key secret references remain adapter-owned as well.
- Future adapters can be separate executables in any language if they implement the same narrow
  internal contract.

Benefits:

- Preserves the requested Rust Codex core while keeping orchestration language-independent.
- One public process/command for users; adapter isolation prevents future language choices from
  contaminating the gateway.
- Go is well suited to HTTP streaming, process supervision, CLI, and concurrency.

Costs and risks:

- Produces two binaries and a private HTTP boundary.
- The private adapter link still requires strict authentication/redaction, but raw upstream
  credentials do not cross it.
- Packaging and end-to-end tests must cover both toolchains.

### Route B — one Rust process with an internal adapter trait

- Public gateway, management, storage, and Codex adapter all live in one Rust workspace/binary.
- Future adapters can be Rust crates; non-Rust adapters would require a later process boundary.

Benefits:

- Smallest deployment and strongest compile-time integration.
- No inter-process authentication boundary.

Costs and risks:

- Does not preserve a language-independent core boundary.
- Makes the later multi-application/multi-language split a migration rather than an extension.
- Ignores the requested opportunity to choose a distinct coordination language.

### Route C — Go gateway with independently operated adapter services

- Same boundary as Route A, but users start/configure each core adapter themselves.

Benefits:

- Lowest supervisor complexity and clearest service separation.

Costs and risks:

- More operational steps and configuration for a deliberately minimal local service.
- Easier to misconfigure or expose the private adapter endpoint.

## Decision ledger

| ID | Question and impact | Owner | Options (recommended first) | Evidence | Status | Answer |
|---|---|---|---|---|---|---|
| D001 | What process/language boundary should support future application cores? This fixes packaging, local secret transport, and adapter extensibility. | User | A: Go coordinator supervises Rust adapter; B: one Rust process; C: independently operated adapter services | Candidate routes and user multi-app constraint | Confirmed | Route A: Go coordinator supervises the Rust Codex adapter |
| D002 | How should upstream credentials be sourced and refreshed? This fixes secret persistence and OAuth lifecycle. | User | C1: Rust Codex adapter reproduces current Codex browser PKCE, device-code fallback, and refresh flow; C2: delegate login/refresh to installed Codex/auth storage; C3: use manually rotated Business/Enterprise Codex access tokens | Codex source spike plus official documentation support boundary | Confirmed | C1: Rust Codex adapter owns source-compatible browser PKCE, device-code fallback, and serialized refresh; compatibility risk accepted |
| D003 | Which public inference endpoint is in v1? This fixes compatibility and test scope. | User | A: only `POST /v1/responses`, usable by Codex configured with a downstream key and by direct Responses clients. B: also implement `POST /v1/chat/completions`. | Codex only uses Responses; existing proxy precedent | Confirmed | Only `POST /v1/responses` |
| D004 | What is the primary usage dimension? | User | A: downstream API key; B: upstream account | Explicit user clarification | Confirmed | Downstream API key for records and aggregates |
| D005 | May implementation tests send real OpenAI/Codex traffic? | User | A: no, loopback mocks only; B: opt-in live smoke test | Explicit user clarification | Confirmed | No real requests in the sandbox |
| D006 | Which coordination language should be used? | Engineering, delegated by user | Go; Node.js/TypeScript; Rust | Local runtime evidence and confirmed Route A | Confirmed | Go |
| D007 | Should raw prompts/responses be retained? | Engineering within explicit minimal/privacy bounds | No; yes | Minimal scope and usage-only requirement | Confirmed | Never persist raw request/response bodies |
| D008 | What is the default network bind? | Engineering within user’s configurable IP:port requirement | Loopback HTTP by default with TLS-required non-loopback binds; all interfaces by default | Security blindspot and D016 | Confirmed | Default `127.0.0.1:8787`; configurable address/port, with HTTPS mandatory outside loopback |
| D009 | How long should per-key request history be retained? | User | A: retain until explicit prune; B: fixed 90 days; C: configurable automatic retention | Historical usage requirement and minimal operations | Confirmed | Configurable, default 7 days |
| D010 | How are latency and usage exposed on streaming responses? | User | A: standards-compatible request-id/TTFB headers, untouched upstream usage in JSON/SSE, final normalized metrics in CLI history; B: inject a custom metrics SSE event immediately before `response.completed`; C: add a metrics lookup HTTP endpoint | HTTP streaming headers commit early; Codex stops reading immediately after `response.completed` | Confirmed | A: no wire mutation; TTFB/request id in headers, upstream usage preserved, final metrics in CLI history |
| D011 | Where should Codex OAuth and upstream API-key secrets be stored? | User | A: Rust adapter-owned `0600` file vault with atomic replacement; B: OS keyring by default with file fallback; C: external secret-manager references only | C1 auth ownership, portability, and minimal-dependency constraint | Confirmed | A: adapter-owned `0600` atomic file vault |
| D012 | When 7-day request details expire, what happens to older per-key aggregates? | User | A: retain daily aggregate totals indefinitely while deleting request rows; B: delete both details and aggregates; C: retain aggregates for a separate configurable period | Per-key history/statistics requirement | Confirmed | A: retain daily per-key aggregates indefinitely; allow explicit manual aggregate deletion |
| D013 | May an existing downstream API key be rebound to another upstream account? | User | A: mapping is immutable; revoke and create a new key to change accounts; B: allow rebind while segmenting history by mapping version; C: allow rebind and report combined history | Audit attribution and minimal schema | Confirmed | A: immutable key-to-account mapping |
| D014 | What should service-side credential removal do to upstream OAuth state? | User | A: mini-sub2api service-side disable/remove only; B: separate explicit OAuth `credential revoke` that revokes upstream then removes service-side state, while `disable` remains service-side/reversible; C: every service-side removal performs best-effort upstream revoke and deletes service-side state regardless of revoke outcome | Official docs, current Codex revoke implementation, external side effect and recovery policy | Confirmed | B: reversible service-side disable plus explicit upstream OAuth revoke; retain disabled vault if revoke fails |
| D015 | What is the default OAuth login flow for a remotely deployed service? | User | A: device-code first, with browser PKCE available explicitly and SSH callback guidance; B: browser PKCE first with device-code fallback; C: browser PKCE only | Remote deployment clarification and localhost callback boundary | Confirmed | A: device-code first for remote/headless deployments; explicit browser PKCE option |
| D016 | How is a remote-facing public listener protected in v1? | User | A: strict transport—plain HTTP only on loopback and HTTPS required for every non-loopback address; B: allow explicit non-loopback HTTP for trusted/VPN networks; C: unrestricted plaintext remote HTTP | Downstream API-key confidentiality, IP certificate verification, and minimal enforceability | Confirmed, revised at final review | A: plain HTTP only on `127.0.0.0/8` or `::1`; every LAN/VPN/public IP requires native HTTPS; proxy remains optional |

## Checkpoints

### Checkpoint A — route selection

Status: passed.

Confirmed route inputs:

- Go coordinator supervising a Rust Codex adapter.
- Public inference surface limited to `POST /v1/responses`.

### Checkpoint B — new evidence

Status: passed for D002 and the remote-deployment choices D015-D016.

Confirmed auth design input:

- D002=C1. The Rust Codex adapter implements and owns source-compatible OAuth login, device-code
  fallback, token storage, refresh-token rotation, and re-login state.

#### Spike S001 — Codex OAuth ownership and support boundary

- Question: can mini-sub2api own a stable, self-contained Codex OAuth login and refresh flow?
- Method: inspect the current official OpenAI authentication/access-token documentation and the
  local Codex `login` Rust crate at the recorded source commit. No login endpoint was called.
- Observed browser flow: Codex starts a loopback callback server, generates high-entropy `state`
  and PKCE S256 verifier/challenge values, opens the authorization endpoint, validates callback
  state, exchanges the authorization code for ID/access/refresh tokens, validates workspace
  identity, and persists the tokens.
- Observed headless flow: Codex requests a user/device code, shows the verification URL, polls for
  an authorization code, and then performs the same PKCE token exchange. Source explicitly treats
  device-code support as feature-gated and falls back to browser login.
- Observed refresh flow: before expiry or after one 401, Codex posts a refresh-token grant to the
  token endpoint, atomically replaces whichever ID/access/refresh tokens are returned, and
  distinguishes expired, reused, and revoked refresh-token failures.
- Critical concurrency rule: mini-sub2api must serialize refresh per upstream account so two
  requests cannot concurrently spend the same rotating refresh token.
- Official support finding: OpenAI documentation describes these sign-in methods for Codex clients
  but does not document third-party OAuth client registration or guarantee these endpoint/client
  details as a public integration contract. Reproducing the source flow is technically feasible
  but compatibility-sensitive.
- Alternative official automation credential: Codex access tokens are documented for trusted
  Business/Enterprise automation, with explicit expiration/rotation/revocation, but they are not a
  general consumer subscription OAuth replacement.
- Conclusion: C1 best satisfies a self-contained consumer-subscription service, but must be
  explicitly accepted as source-compatible rather than a stable public OAuth integration.
- Remaining risk: OpenAI can change or restrict the first-party client flow, scopes, device-code
  endpoints, or token behavior; the adapter must keep issuer/client id configurable and fail closed.

#### Spike S002 — supervised Rust adapter streaming through Go

- Question: can a Go coordinator supervise a Rust adapter and preserve Responses SSE without
  buffering the completed event?
- Method: compile dependency-free temporary Go and Rust programs under `/tmp`; Rust announced an
  ephemeral loopback port over a one-line JSON readiness handshake and emitted two HTTP chunked SSE
  events 150 ms apart; Go proxied chunks with an explicit HTTP flush. No external connection was
  made.
- Observed successful result: first public chunk at approximately 1 ms, completion at 155 ms,
  153 ms inter-event gap, adapter TTFB header preserved, and the usage-bearing completion event
  preserved byte-for-byte.
- Observed failed attempt: the first mock adapter did not reliably consume the complete inbound
  request before process exit, causing a TCP reset that discarded the terminal event. The harness
  was corrected to consume the declared body length before responding.
- Conclusion: Route A preserves streaming with explicit flush, provided the adapter fully consumes
  or deliberately closes request bodies, propagates stream read errors, and does not exit before
  the response finishes.
- Remaining risk: cancellation/backpressure behavior still needs implementation integration tests;
  the coordinator must stop the upstream stream when the public client disconnects.

#### Spike S003 — single-account refresh serialization and atomic persistence

- Question: can the Rust adapter prevent rotating-refresh-token reuse under concurrent requests
  with a minimal local vault?
- Method: run 16 concurrent dependency-free Rust workers against one stale account protected by an
  account mutex; the mock refresh rotated both tokens and persisted via `0600` temporary file,
  `sync_all`, and atomic rename.
- Observed result: all 16 workers received the new access token, exactly one refresh occurred, the
  final record contained the rotated refresh token, and file mode was `0600`.
- Conclusion: a per-account double-checked async mutex plus atomic vault replacement is sufficient
  for the selected single supervised adapter process.
- Remaining risk: multiple adapter processes would require an inter-process file lock or a
  transactional secret store; v1 must enforce one active Codex adapter per state directory.

#### Spike S004 — Codex handling of additive SSE metrics events

- Question: can the proxy append a terminal metrics event without changing Codex behavior?
- Method: inspect the current Codex Responses SSE parser and its terminal-event loop.
- Observed result: unknown event types are ignored, but the parser returns immediately after
  forwarding `response.completed`.
- Conclusion: an additive `mini_sub2api.metrics` event would have to be injected immediately before
  `response.completed` to be observable. Appending it after completion is ineffective for Codex.
- Remaining risk: although current Codex ignores an unknown pre-completion event, other strict
  Responses clients may not. The no-wire-mutation D010 option remains the compatibility-safe route.

#### Spike S005 — service-side credential deletion versus upstream OAuth revoke

- Question: what does credential removal change in mini-sub2api deployment state versus OpenAI
  upstream state?
- Method: inspect current official OpenAI authentication documentation and the current Codex
  `login/src/auth/revoke.rs` plus logout orchestration. No revoke request was sent.
- Observed official behavior: documentation describes `codex logout` as clearing current cached
  credentials; it does not describe account/subscription deletion.
- Observed source behavior: managed ChatGPT logout attempts a best-effort OAuth revoke using the
  refresh token (or access token if no refresh token exists), then deletes local auth even if the
  revoke attempt fails.
- Terminology for mini-sub2api:
  - service-side state: the database and credential vault on the server, VM, container, or Pod
    where mini-sub2api is deployed; it does not mean the API caller's computer.
  - `credential disable`: reject new routing through the credential but retain its service-side
    vault and metadata; reversible and does not contact OpenAI.
  - service-side removal: delete mini-sub2api metadata/vault; OpenAI account, subscription, and any
    still-valid upstream copy of the token are unaffected.
  - upstream OAuth revoke: invalidate the OAuth token at the authority; it does not delete the
    ChatGPT/OpenAI account or subscription.
- Scope boundary: regular upstream OpenAI API keys are not OAuth tokens. mini-sub2api v1 will not
  attempt to delete those keys at the provider; it can only remove its local secret/reference.
- Selected behavior: D014=B. If upstream OAuth revoke fails, retain the disabled service-side vault
  so an operator can retry; permit service-side-only forced deletion only through a second,
  explicit destructive confirmation.

#### Spike S006 — direct IP and TLS identity

- Question: can direct IP access use one simple transport rule without classifying private networks?
- Method: inspect current OWASP API/TLS guidance, IETF HTTPS IP-identity rules, and Tailscale's
  documented data-plane encryption. No service was exposed.
- Observed result: API credentials require transport protection; HTTPS clients validate a literal
  IP against an exact `iPAddress` subjectAltName; Tailscale encrypts permitted peer traffic
  end-to-end, while a generic LAN/VPC private address supplies reachability isolation but not
  application-layer encryption.
- Conclusion: allow plain HTTP only when every resolved listener address is loopback. Require native
  static-certificate HTTPS for every non-loopback LAN, VPN, or public address. Do not classify
  RFC1918/VPN networks and do not make a reverse proxy mandatory.
- Remaining risk: certificate mismatch, trust, expiry, or permissive firewall can still break or
  expose deployment. Startup diagnostics must print exact resolved listeners and TLS state.

### Checkpoint C — final review

Status: passed, then re-passed after the user simplified D016 during final review. The
decision-ledger audit found no unresolved material user-owned decisions.

Review challenges and resolutions:

- Provider secrets crossing the Go/Rust boundary: resolved by an opaque account reference and a
  Rust-owned vault; the internal bearer only authenticates the deployment-internal protocol.
- OAuth endpoint/client drift: accepted explicitly under D002; isolate it in the Codex adapter,
  keep compatibility overrides operator-only, mock every auth path, and fail closed.
- Remote browser callbacks: resolved by device-code-first D015; browser PKCE is explicit and
  documents callback tunneling.
- Rotating refresh-token races: resolved by one active core per vault, per-account double-checked
  locking, atomic token replacement, and concurrency tests.
- Streaming final metrics: resolved by D010; preserve the upstream stream, expose only TTFB in
  initial headers, and keep final normalized metrics in history.
- Direct IP/private-network exposure: resolved by strict D016; HTTP is loopback-only and every
  non-loopback listener requires native TLS, without private-network classification.
- Retention racing aggregation: resolved by transactional aggregate-before-delete ordering,
  injected-clock tests, permanent daily aggregates, and tombstoned identities.
- Duplicate inference on retries: resolved by prohibiting transport/429/5xx replay and allowing
  only one pre-response auth-refresh replay.
- Parallelization safety: T003 and T004 may run in parallel only after T002 freezes fixtures; T005
  integrates both, so no overlapping write sets are required.

Accepted residual risks:

- v1 has no downstream quotas, billing, per-key rate limits, or multi-account load balancing. A
  valid key can consume the mapped account's available capacity; upstream limits remain decisive.
- The `0600` vault is access-controlled but not encrypted at rest. Host/root compromise exposes
  provider credentials.
- ChatGPT subscription routing follows source-observed Codex behavior rather than a documented
  third-party OAuth/backend compatibility guarantee and may require maintenance after Codex changes.
- Native TLS loads operator-supplied static certificate files but does not implement ACME,
  automatic renewal, or certificate issuance.
- Direct IP HTTPS requires an operator-provided certificate whose identity matches the literal IP
  and whose issuer is trusted by clients; expiry/renewal is outside the service.
- One coordinator and one Codex core per state directory is the supported v1 topology; HA and
  cross-host core deployment are out of scope.

## Proposed selected design

Route A, Responses-only, and D002=C1 authentication are selected. The Rust Codex adapter owns
provider-specific credential material under a private adapter state directory. The Go coordinator
stores only the adapter/account reference and never receives OAuth or upstream API-key secrets.

Confirmed storage/observability behavior:

- The Rust adapter vault uses owner-only `0600` files and atomic replacement for token rotation.
- Request-detail retention is configurable with a default of 7 days.
- Expired request rows roll into permanent daily per-key aggregates. Operators may explicitly
  delete aggregates with a previewed/confirmed CLI prune command.
- A downstream API key's upstream account reference is immutable; moving traffic requires revoking
  the old key and creating a new one.
- Streaming responses receive an additive request-id header and standards-compatible
  `Server-Timing` TTFB entry, preserve upstream JSON/SSE usage unchanged, and expose final normalized
  duration/usage through the Go CLI history. No custom SSE event or public metrics endpoint is
  added.

The components are:

```text
public client
  -> Go coordinator (public IP:port, downstream key auth, SQLite, CLI)
       -> supervised loopback HTTP + ephemeral internal token
            -> Rust Codex core adapter (provider credential vault + refresh lock)
                 -> Codex subscription Responses endpoint OR OpenAI Responses endpoint
```

The coordinator selects exactly one adapter/account reference before dispatch. The Rust adapter
does not read the coordinator database and cannot select a different account. It resolves only that
account reference from its private vault. The response streams back without buffering the model
output. Usage is parsed deterministically and recorded against the resolved downstream key id.

For v1, the Go coordinator and supervised Rust core are one deployment unit on the same host or in
the same container/Pod network namespace. The public listener may bind a configured remote-facing
IP:port, while the coordinator-to-core link remains deployment-internal loopback. Separate-host
coordinator/core deployment is out of scope.

Confirmed D016 transport contract:

- Plain HTTP defaults to `127.0.0.1` and is accepted only when every resolved listener address is
  within IPv4 loopback `127.0.0.0/8` or IPv6 loopback `::1`.
- Any non-loopback bind—including LAN, VPC, WireGuard/Tailscale, wildcard `0.0.0.0`, or `::`—is
  rejected unless both `--tls-cert` and `--tls-key` are supplied and native HTTPS starts
  successfully. There is no plaintext override.
- Native HTTPS supports direct IP or hostname access. Certificate identity must match the address
  clients use and the client must trust its issuer.
- A reverse proxy is optional. When used, proxy examples preserve SSE delivery, overwrite/strip
  forwarded and hop-by-hop headers, expose HTTPS externally, and connect to mini-sub2api through a
  loopback HTTP listener on the same deployment host/namespace.
- mini-sub2api does not use client-supplied forwarding headers for authentication or other security
  decisions.

### Repository and artifact layout

```text
mini-sub2api/
├── Cargo.toml                         # Rust workspace
├── go.mod / go.sum                    # Go coordinator module
├── mise.toml                          # Go 1.26.4 and Rust 1.96.0
├── src/
│   ├── coordinator/
│   │   ├── cmd/mini-sub2api/
│   │   └── internal/{adapter,cli,config,httpapi,storage,usage}/
│   ├── core/codex/                    # mini-sub2api-core-codex crate
│   └── protocol/v1/                   # internal contract and shared fixtures
├── scripts/{build.sh,test.sh,...}
├── build/bin/                         # both generated binaries
└── ref/                               # foundation records and indexes
```

- First-party implementation stays under `src/`; root manifests contain build metadata only.
- The distributed unit contains `mini-sub2api`, `mini-sub2api-core-codex`, and `build-info.json`.
- Go uses one direct persistence dependency: a pure-Go SQLite driver. Rust dependencies are limited
  to HTTP/runtime, serialization, cryptography/PKCE, CLI, and error-handling crates required by the
  Codex adapter.
- Generated output is confined to `build/`; both `build/` and `dist/` remain ignored.

### Public Responses contract

- Expose only `POST /v1/responses`; all other public paths return an OpenAI-shaped `404` error.
- Authenticate `Authorization: Bearer <downstream-key>`. Invalid, revoked, or disabled mappings
  return `401` without revealing whether the account reference exists.
- Downstream keys use 256 bits of CSPRNG entropy with an `ms2a_` display prefix. Print the full key
  exactly once; persist only SHA-256 plus a short non-secret display prefix and immutable key id.
- Do not persist or log request/response bodies. Read each request into a bounded in-memory buffer
  so the single permitted pre-response auth retry can replay it; responses remain streaming.
- Strip inbound authorization, account-id, cookie, host, proxy-auth, forwarding, content-length,
  and hop-by-hop headers. Forward only the reviewed Responses/Codex compatibility allowlist. The
  Rust adapter constructs authoritative upstream auth/account headers.
- Preserve upstream status, safe response headers, JSON, and SSE bytes. Add a generated
  `x-mini-sub2api-request-id` and merge `upstream_ttfb;dur=<milliseconds>` into `Server-Timing`.
- Never append or mutate SSE events. Upstream `response.completed.response.usage` remains the
  client-visible token-usage source.
- Public cancellation propagates through Go to Rust and the upstream response body. Persist a
  partial request row with nullable usage rather than allowing detached inference to continue.
- Configure bounded header/body reads and upstream connect/header/idle timeouts without imposing a
  whole-response timeout that would kill valid long SSE streams. Do not retry inference on
  transport/429/5xx failures. The only request replay is one auth-refresh retry after an upstream
  `401` and before any response bytes reach the client.

### Coordinator-to-core protocol

- `mini-sub2api serve` starts exactly one Codex core for a state directory, binds it to an ephemeral
  loopback port, and sends a random 256-bit internal bearer through the child's stdin before closing
  that pipe. The token never appears in argv, environment, logs, or the database.
- Core stdout emits one bounded readiness JSON object containing protocol version, loopback port,
  pid, and build identity; all later logs use stderr and are secret-redacted.
- Internal inference uses `POST /internal/v1/responses` with protocol-version, account-reference,
  public-request-id, and internal authorization headers. Raw OAuth/API-key credentials never cross
  this boundary.
- Unknown protocol versions, missing internal auth, unknown account references, and non-loopback
  peers fail closed. The coordinator never falls back to another account or auth kind.
- The core returns upstream data plus a core TTFB header. Go owns downstream-key attribution,
  total-duration measurement, normalized usage persistence, and public response streaming.
- Process exit cancels in-flight requests and makes the public service unhealthy; restart uses a
  bounded backoff and never starts two cores against the same vault concurrently.

### Service-side persistence

- The service state root is mode `0700`. Coordinator SQLite, WAL files, and Rust vault records are
  reachable only through that directory; secret vault records are mode `0600`.
- Coordinator SQLite uses transactional migrations and these logical tables:
  - `credentials`: stable id, adapter, display name, auth kind, opaque core account reference,
    lifecycle status, timestamps, and tombstone state; no provider secret.
  - `api_keys`: stable id, display name/prefix, SHA-256 hash, immutable credential id, lifecycle
    status, creation/revocation timestamps.
  - `requests`: request id, api-key id, credential-id snapshot, UTC timestamps, terminal status,
    HTTP status, TTFB/duration, and nullable normalized token fields.
  - `daily_usage`: UTC day plus api-key id, request/status counts, duration totals, and token sums.
  - `schema_meta`: migration version.
- Completing a request updates its request row and daily aggregate in one transaction. Missing
  upstream usage increments the appropriate request/status counts but never invents zero tokens.
- `--usage-retention-days` defaults to `7`; `0` disables automatic detail pruning. Prune once at
  startup and every 24 hours using an injected clock. Aggregate first, then delete eligible detail
  rows transactionally. Daily aggregates persist until explicit manual deletion.
- Credential and API-key metadata are soft-deleted/tombstoned so historical aggregates retain
  stable attribution after secrets are removed.

### Rust credential vault and OAuth lifecycle

- One opaque account-reference file stores either Codex OAuth tokens/account identity or a regular
  upstream OpenAI API key. Writes use a same-directory temporary file, `sync_all`, mode `0600`, and
  atomic rename; directory metadata is synchronized where supported.
- Default remote/headless OAuth login is device-code. Browser authorization-code + PKCE S256 is an
  explicit flow with state verification and SSH callback guidance.
- Decode token expiry locally. Within the five-minute refresh window, use a per-account
  double-checked async mutex; apply returned token fields atomically, including refresh-token
  rotation. One upstream `401` permits one forced refresh and one retry.
- Expired, reused, revoked, identity-mismatched, or unparseable refresh state marks the credential
  `requires_login`; it never falls back to a different credential.
- `credential disable` is service-side and reversible. OAuth `credential revoke` requires no active
  downstream keys plus explicit confirmation, revokes upstream first, then removes the vault on
  success. On failure, retain a disabled vault for retry. Force-service-only deletion requires a
  second explicit destructive confirmation.
- Key/credential disable or revoke prevents new requests; already authenticated in-flight requests
  are allowed to finish. Upstream OAuth revoke additionally requires the core's active-request
  count for that account to reach zero.
- Regular upstream API-key removal deletes only service-side material; provider-side key deletion
  is outside v1.

### CLI and configuration surface

```text
mini-sub2api serve [--listen IP:PORT] [--tls-cert FILE --tls-key FILE]
                   [--usage-retention-days N]
credential login codex --name NAME [--flow device|browser]
credential add-api-key codex --name NAME --secret-stdin
credential list | enable ID | disable ID | revoke ID | remove ID
key create --credential ID --name NAME | list | revoke ID
usage history [--key ID] [--since TIME] [--limit N]
usage stats [--key ID] [--since DATE] [--until DATE]
usage prune --before DATE [--include-aggregates] [--yes]
```

- Global `--state-dir` and `MINI_SUB2API_STATE_DIR` select deployment state. Flags override
  environment values; invalid or conflicting values fail before opening listeners.
- HTTP defaults to `127.0.0.1:8787`. TLS certificate and key must be supplied together. Every
  non-loopback bind is rejected without both; startup prints exact listener addresses and TLS state
  before reporting ready.
- Upstream OAuth/Responses endpoints and client id are adapter configuration, not public request
  parameters. Overrides exist for offline tests/controlled compatibility only and are never chosen
  by downstream clients.
- Secret input goes directly from operator stdin to the Rust credential subcommand; the Go
  coordinator wires the stream without parsing, echoing, or persisting it.
- Destructive CLI commands preview exact ids/counts and require `--yes` or an interactive
  confirmation. `--json` provides stable machine-readable output; default user-facing CLI copy is
  English (`en-US`) per `UI_COPY_LANGUAGE.md`.

## Model/LLM boundary

mini-sub2api introduces no planning or control-plane LLM calls.

- Semantic responsibility: only the upstream inference request explicitly submitted by the API
  client invokes a model.
- Minimum inputs: exactly the client’s approved Responses request after deterministic header and
  route adaptation; no account-management or history data is added.
- Output: the upstream Responses JSON/SSE stream, passed through without semantic rewriting.
- Deterministic assembly: key authentication, account lookup, URL selection, auth headers, request
  forwarding, SSE usage parsing, timing, persistence, and aggregation are ordinary code.
- Mechanical validation: schema/field assertions against mock upstream captures, hash lookups,
  database constraints, golden-free structured tests, and exact aggregate comparisons.

## Executable task breakdown

These tasks were approved and completed. Private function names were refined without changing the
confirmed public, security, or persistence contracts.

### T001 — Project foundation

- Task system id: `c218a67e-7f4c-496a-aed7-827271c464a1`
- Owner: implementation session
- Dependencies: final plan approval
- Write area: repository root, `scripts/`, `ref/`, `.cargo/`, toolchain metadata
- Work: initialize Git and the complete `project-engineering-foundation` structure; preserve this
  plan under `.ref/`; set English `en-US` CLI copy SSOT; pin Go 1.26.4 and Rust 1.96.0 through
  project-local `mise`; create root Go/Rust manifests; configure output under `build/`; install the
  advisory hook.
- Validation: foundation shape/index inspection, exact `.gitignore` entries, hook install,
  `mise current`, empty-workspace format/build smoke checks.
- Done: generated instructions contain no placeholders, all required ref indexes/helpers exist, and
  the new repository is isolated from neighboring projects.

### T002 — Shared internal protocol

- Task system id: `f07ee21e-4fd4-4f14-b37a-3146946c9c2d`
- Owner: implementation session
- Dependencies: T001
- Write area: `src/protocol/v1/` only
- Work: define the narrow coordinator-to-adapter request/response contract, redaction rules,
  internal authentication, startup handshake, error model, metrics fields, and version negotiation.
- Validation: the same protocol fixtures decode in Go and Rust; malformed/unauthenticated requests
  are rejected.
- Done: protocol version, readiness record, sensitive-header rules, request/response headers, and
  error codes are frozen for v1 fixtures.

### T003 — Rust Codex core adapter

- Task system id: `819cfcb9-fefb-4fe5-b412-69953f7a5371`
- Owner: implementation session
- Dependencies: T002
- Write area: `src/core/codex/` only
- Work: support Codex subscription and regular OpenAI API-key routes, provider-owned credential
  storage, the confirmed D002 login mode, serialized refresh-token rotation, strict header
  replacement, Responses streaming, usage parsing, TTFB/total duration measurement, and
  configurable auth/upstream URLs for mock tests.
- Validation: `cargo fmt --check`, scoped Clippy with warnings denied, Rust unit/integration tests
  against loopback mock auth/Responses upstreams, and a test guard that rejects non-loopback test
  endpoints.
- Done: both auth kinds stream Responses, OAuth/device/refresh/revoke behavior is mock-covered,
  vault permissions/atomicity and one-core lock are proven, and secrets never reach stdout/logs.
- Parallelization: after T002, this can run in parallel with T004 because write areas are disjoint.

### T004 — Go coordinator and persistence

- Task system id: `27460d73-419a-4612-841d-e0a28bec5c6c`
- Owner: implementation session
- Dependencies: T002
- Write area: `src/coordinator/` and coordinator-owned migration fixtures only
- Work: public listener, downstream key hashing/lookup, account-reference-to-adapter mapping,
  SQLite transactions, adapter supervision, request dispatch, per-key history, and aggregates. Do
  not store provider OAuth/API secrets in coordinator SQLite. Apply configurable request-detail
  retention with a 7-day default using deterministic maintenance.
- Validation: Go tests for key creation/revocation, concurrent storage access, routing, error
  redaction, exact per-key statistics, retention, native TLS/listener guards, child readiness, SSE
  flush/cancellation, and no-body-persistence assertions; run `gofmt`, `go test`, and `go vet`.
- Done: public contract, SQLite transactions, immutable mapping, listener safety, and adapter
  supervision pass with a fake core; no Rust implementation is required by these component tests.
- Parallelization: after T002, this can run in parallel with T003 because write areas are disjoint.

### T005 — CLI

- Task system id: `330d9a3c-cb4c-4457-99cf-ee4f6d2c13ed`
- Owner: implementation session
- Dependencies: T003 and T004
- Write area: Go CLI command modules
- Work: implement the confirmed credential login/add/list/enable/disable/revoke/remove commands,
  key create/list/revoke, serve flags, usage history/stats/prune, confirmations, and JSON output.
- Validation: command tests with isolated temporary state; generated key appears once; secrets never
  appear in list/history output.
- Done: every command has help, stable exit codes, structured JSON coverage, destructive previews,
  and service/core orchestration without secrets in argv/environment.

### T006 — Cross-language offline integration

- Task system id: `8ce71983-c9a6-4790-a80c-f5d44c44f442`
- Owner: implementation session
- Dependencies: T003, T004, T005
- Write area: integration harness and fixtures
- Work: launch mock upstream, coordinator, and supervised Rust core; exercise subscription and
  regular API-key modes, streaming and non-streaming behavior, invalid/revoked keys, and two
  downstream keys mapped to one OAuth source.
- Validation: captured upstream path/headers/body, public stream parity, no secret leakage, and
  request/aggregate attribution to the exact downstream key.
- Done: tests cover HTTP loopback, rejection of all non-loopback HTTP, native HTTPS with a test CA,
  streaming/non-streaming, cancellation, auth refresh/401, revoked state, retention, and process
  restart—entirely on loopback/mock endpoints.

### T007 — Packaging, docs, and final records

- Task system id: `f570e3a5-fe31-4f9f-97e0-de1e4f016cd3`
- Owner: implementation session
- Dependencies: all implementation tasks
- Write area: README, build metadata, scripts, `ref/`
- Work: build both binaries and shared `build-info.json`; expose `--version` and
  `--check-installed`; document direct IP, loopback HTTP, native TLS, optional proxy, OAuth compatibility,
  backup/restore, and CLI examples; archive changelog, this approved plan, and any required review
  record; install/refresh the advisory hook.
- Validation: format, lint, unit/integration tests, release builds, CLI help/version/freshness,
  file-size guardrail, clean Git status except intended changes.
- Done: `scripts/test.sh` and `scripts/build.sh` pass from a clean checkout under `mise`, artifacts
  contain matching build metadata, no source module exceeds 500 LOC without recorded justification,
  and `.ref/` has no unclassified scratch artifacts.

## Cross-task validation strategy

1. Unit tests validate deterministic auth/header construction, hashing, parsing, and aggregation.
2. Component tests use only loopback mock upstreams and assert full structured requests.
3. Cross-language protocol fixtures are consumed independently by Go and Rust.
4. End-to-end tests launch all local processes and prove that no public downstream credential is
   sent upstream and no upstream credential is returned to the client.
5. Two downstream keys mapped to one OAuth source generate separate history and aggregate totals.
6. Streaming tests split SSE frames across arbitrary chunk boundaries before usage parsing.
7. Non-streaming tests expose final duration/usage deterministically; streaming tests assert the
   confirmed no-wire-mutation D010 contract.
8. Build/test commands run through project-local `mise` without modifying user-global versions.
9. Cancellation tests disconnect the public client mid-stream and prove the coordinator cancels the
   adapter/upstream body while writing a partial terminal history record.
10. Refresh tests synchronize multiple requests at the expiry boundary and assert exactly one token
    grant, one atomic persist, and no use of the retired refresh token.
11. Retention tests use an injected clock to prove the 7-day boundary and the confirmed D012
    aggregate policy without wall-clock sleeps.

## Risks and rollback

- Adapter protocol churn: version the internal contract from the first implementation and reject
  unknown versions.
- OAuth compatibility drift: the adapter owns refresh and must fail closed with `requires_login`
  when source-compatible OAuth behavior changes or token rotation cannot be completed safely.
- Secret leakage: central redaction tests, no body persistence, sensitive headers excluded from
  logs, loopback-only adapter.
- Partial usage records on disconnect: store terminal status (`completed`, `upstream_error`,
  `client_disconnected`) and nullable usage fields rather than inventing zero usage.
- Schema migration failure: run transactional migrations and keep the previous schema version
  untouched on failure.
- Implementation rollback: because this is a new repository, abandon the unmerged implementation
  branch/repository; do not mutate neighboring repositories or Codex source.

## Execution state

- Completed step: T001-T007 are complete. The project foundation, shared protocol, Rust Codex core,
  Go coordinator/storage, CLI, cross-language loopback-only integration suite, release artifacts,
  documentation, and final records are implemented.
- Verification performed: `scripts/test.sh`, race-enabled Go tests, Rust workspace tests, Clippy
  with warnings denied, Go vet, actual Go-to-Rust process integration, native TLS with a test CA,
  device OAuth/refresh/revoke mocks, cancellation, restart, retention, build, version, and installed
  freshness checks. No OpenAI, ChatGPT, or Codex endpoint was contacted.
- Initial repository state was committed to `main` as `93220c5` after all planned gates passed;
  build output remains ignored.
- Review result: two independent Codex reviewers, used under the user's explicit same-adapter
  exception because other channels were unavailable, completed the implementation, post-fix, and
  residual passes. All accepted findings were fixed and both residual reviewers returned `PASS`.
- Final records:
  - `ref/reviews/recent-3-days/REVIEW_1_mini-sub2api-security-lifecycle.md`
  - `ref/changelogs/recent-3-days/CHANGELOG_1_initial-service.md`

## Final status and handoff

The approved minimal v1 is complete and committed. Use `README.md` for installation, operation, and
CLI examples. Future changes must preserve the confirmed minimal scope and follow `CLAUDE.md`,
including loopback-only automated testing and review-expiry rules.
