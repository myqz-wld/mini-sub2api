# User-selected gateway policies

Last reconciled: 2026-09-05. Native comparison baseline: Codex v0.153.4.

This document records product rules the user requested or explicitly confirmed. It distinguishes
those decisions from delegated implementation defaults, native protocol behavior, and superseded
choices. It describes the current contract; it does not declare every current implementation
behavior correct. The source and capture comparison is summarized in
[NATIVE_PARITY.md](src/coordinator/integration/NATIVE_PARITY.md).

**Requested** means an explicit instruction is retained in the conversation or decision record.
**Confirmed** includes a proposal the user explicitly selected or approved. **Delegated** means a
detail chosen during authorized execution, without evidence of individual user selection.
Where exact personal selection is not established, this document says so.

The private `ref/` archive holds the historical decision ledgers named below. Their old instructions
are provenance, not new deployment or operational authorization. Later confirmed requirements take
precedence; historical limits and source-dependent behavior must not silently return.

## Routing and product scope

| Policy | Origin | Current scope |
|---|---|---|
| Go coordinator supervises a deployment-local Rust Codex Core | Confirmed: P1 D001, D006 | The coordinator manages public service, Keys and usage; Core owns credential material and upstream behavior. |
| Only the Codex adapter is currently supported; ordinary Responses callers also work | Requested: P10 D01 | Codex is the adapter scope, not a restriction on caller applications. |
| Public inference uses Responses HTTP/SSE and WS | Confirmed: P1 D003, P2; P10 D01/D04 | POST/GET `/v1/responses`; no Chat Completions API or `/v1/conversations` management. |
| Every upstream API-key request bypasses Codex emulation | Requested: P10 D02 | Native Codex and ordinary callers alike; no instruction, device, session, response-ID or format rewriting. Necessary authentication/routing/admission still apply. |
| Every Subscription request receives Codex emulation | Requested: P10 D03/D11 | Caller claims or Originator cannot bypass it. Compatibility reference is v0.153.4. |
| The gateway preserves HTTP versus WS | Requested: P10 D04 | No HTTP-to-WS or WS-to-HTTP conversion, including gateway recovery. A later HTTP request issued by the native client is a new caller-selected transport. |
| One distribution Key has one immutable credential binding | Confirmed: P1 D013 | Revoke/create a Key to change its binding. Multiple Keys may share one credential; no account pool or automatic switching. |
| Legacy `/responses/compact` is excluded from the public surface | Confirmed: P3 D7 | Native V1 compact rejection at this boundary is a scope result; it does not authorize adding that endpoint. V2/local Responses compaction is separate. |

## Device convergence and transport identity

**Device convergence is a user-selected gateway policy.** Native Codex has an installation identity,
but the gateway's decision to make many downstream installations appear as one device is a separate
product choice.

| Policy | Origin | Current meaning |
|---|---|---|
| Only `off` and `device` fingerprint modes; default `device` for new and existing records | Requested/confirmed: P3 D1/D2 | No `session` or `full` fingerprint modes. Stateful session handling does not add another fingerprint mode. |
| Device convergence applies to installation identity | Confirmed: P3 D5; P5 selected design | It does not merge conversations, threads, turns, context windows or tool-consumption state. |
| Subscription device identity converges by upstream ChatGPT account namespace | Confirmed: P5 D3/D12 and account-level selected design | Within the same persisted state, different downstream Keys and duplicate OAuth credential records for the same account share one installation UUIDv4. |
| OAuth refresh and process restart preserve that device identity | Confirmed: P3 D3; P5 durable-state design | The current UUID is generated and persisted. Independently initialized state directories need not generate the same UUID; old stateless cross-host derivation was superseded. |
| Installation carriers agree | Confirmed: P3 D4, narrowed by P10 | Subscription direct/compatibility headers, flat client metadata and serialized turn metadata use the selected installation ID. |
| `off` disables installation convergence, not Subscription emulation/privacy | P5 selected design; P10 routing precedence | Caller installation identities receive scoped one-to-one aliases. It does not expose raw identities or disable instruction/transport normalization. |
| API-key payloads and installation carriers remain transparent in both modes | Requested: P10 D02 | This supersedes the early API-key device-rewrite exception. Administrative mode/revision state and stale-policy checks still exist. |
| Mode changes require the credential disabled and active work drained/fenced | Confirmed: P3 D9 | Successful mutation does not silently re-enable the credential. |
| Existing WS checks the captured fingerprint revision before further application work | Confirmed: P3 D13 | A stale socket is closed before forwarding a request under an obsolete policy. |
| HTTP/WS pools and TLS resumption state are isolated by credential | Confirmed: P3 D8 | Device UUID scope and transport-pool scope differ: duplicate credentials may share an account device but retain separate pools and mode/revision sidecars. |
| TLS follows the pinned native transport construction, without operator-selected profiles or per-device ClientHello variants | Confirmed: P3 D6/D15 | No JA3/uTLS/profile control or randomized device-specific TLS template. Cryptographic randomness and extension permutation are not fixed fingerprint fields. |
| Per-credential proxy/source-IP settings were not added | Confirmed scope: P3 D16 | A Core-owned extension point exists; it is not an implemented egress-routing feature or permission for public IP probes. |

Current Subscription headers also use one process-stable Codex TUI identity: canonical UA,
`originator: codex-tui` and the pinned version. OS/architecture/terminal come from the Core's deployment
runtime, not an arbitrary caller header. This exact TUI/runtime selection is established by C18/C20
and R20/R22; the available records do not preserve a separate user-selection ledger for every literal
value. It should not be mislabeled as an individually user-chosen constant.

Implementation anchors: `vault.rs:108`, `request_state_editor.rs:113`, `fingerprint.rs`,
`fingerprint_projection.rs`, `request_profile.rs`, `codex_user_agent.rs`, `transport_registry.rs`,
all under `src/core/codex/src/`. The account installation UUID resides in request state; the
credential fingerprint sidecar contains mode/revision policy, not its own installation UUID.

## Sessions, context and continuation

| Policy | Origin | Current scope |
|---|---|---|
| Explicit session location is Key-scoped | Requested: P10 D05 | Original HTTP/WS handshake session identity → client_metadata.session_id → turn metadata session_id. The same selected identity under another Key is another conversation scope. |
| A known previous_response_id restores its original association when no explicit session is supplied | Requested: P10 D05 | Different responses can belong to one session; unknown references do not restore missing state. |
| First WS request locates/initializes and binds a session; later frames inherit | Requested: P10 D05 | Reject explicit cross-session references; reconnect performs location again. |
| conversation_id is excluded from session identity | Requested: P10 D05 | Existing mapped top-level conversation compatibility is separate and does not establish complete local history. Cache/request/thread/window hints cannot restore the old locator chain. |
| State must distinguish sessions, threads, logical turns and responses | Requested/confirmed: P10 D07 | A tool loop may contain several completed responses within one turn. Exact turn-inference and scheduler rules are delegated below. |
| Match structured history at completed-response boundaries using the longest eligible prefix | Requested: P10 D08/D13 | Search an identified session, or only the same Key's anonymous pool when unbound. Anonymous fallback must not search explicitly identified sessions. |
| Equivalent complete contexts may share content despite different response IDs | Requested/confirmed: P10 D13 | Shared content does not merge mutable owners, active turns or tool consumption. Exact references and WS ownership remain authoritative. |
| Use compressed comparison/indexing for efficiency | Requested: P10 D12 | The specific hash/interner/radix representation is an implementation selection. Repeated input occurrences still remain repeated. |
| Subscription requests are classified across HTTP/WS × full/incremental × ordinary/Lite | Requested: P10 confirmed requirements | Validate original fields, references and context before projecting; do not identify Codex or Lite from one unvalidated marker. |
| Invalid/noncanonical forms are repaired only without semantic loss, otherwise rejected | Requested: P10 confirmed requirements | Revalidate the repaired request; unsupported dependencies or incomplete history cannot be guessed. |
| Subscription HTTP increments become full HTTP requests or fail before inference | Requested: P10 D06 | Complete local context is required. A previous ID alone is insufficient. |
| WS may move from an increment to a full request within WS | Requested: P10 D10 | Requires available complete context and applicable recovery conditions; it does not authorize blind replay. |
| Local complete-history idle expiry is three hours, with generous capacity | Requested: P10 D09 | Active requests are protected. Complete requests can rebuild expired history; HTTP increments without it fail. |
| Bulk-history expiry alone does not invalidate live upstream WS history | Requested/confirmed: P10 D15 discussion | Valid ownership/mappings/connection still matter. A successful remote-only suffix does not magically materialize complete local history. |

## Instructions and environment metadata

| Policy | Origin | Current scope |
|---|---|---|
| Valid nonblank caller base instructions win and stay exact | Requested: P9; P10 preservation constraint | Preserve whitespace and literal caller placeholders. Do not split/replace a recognized template prefix. |
| Missing, null, blank or invalid-type ordinary instructions use the model default | Requested: P9 | This deliberately differs from native explicit empty/blank base choices. API-key forwarding is exempt. |
| Preserve developer messages, their contents, relative order and duplicates | Requested: P9 | No generic text deduplication or removal of template-looking developer content. |
| Subscription changes system role to developer in place | Requested: P9 | Content and relative position remain. API-key input stays unchanged. |
| Ordinary-to-Lite emits tools, one selected developer base, then original input | Requested: P9 | Remove the ordinary top-level base carrier; do not insert a second base. |
| Already formed native Lite keeps its input instructions | Requested: P9, refined by P10 validation | No implicit duplicate fallback; valid explicit top-level base follows tools. Valid inherited Lite increments do not repeat setup. |
| Render only gateway-owned default templates | Requested/confirmed: P9/P10 | Caller text is not rendered. Native literal examples such as connector placeholders remain legitimate. |
| Preserve a legal sandbox_mode and derive the sandbox implementation from Core's platform | Requested: P5 D2, user's 4A-v2 | Missing/invalid mode becomes danger-full-access/none; external and unrestricted modes retain their defined sandbox meaning. This is metadata, not execution of caller commands. |
| Preserve workspaces and caller context text | Requested: P5 D2 | Do not rewrite caller paths to imitate the deployment host. Native AGENTS/Skills/environment discovery stays the client's responsibility. |

## Identity privacy, response boundaries and durable state

| Policy | Origin | Current scope |
|---|---|---|
| Generate genuine UUIDv4 installation and UUIDv7 logical identities once, then persist/reuse | Confirmed: P5 selected design/D5 | HMAC is used for private lookup keys; do not forge timestamp UUIDs from arbitrary digest bytes. Native Lite setup separately uses its UUIDv5 content rule. |
| Keep one bounded versioned request-state file per upstream account namespace | Confirmed: P5 D3/D12 | It contains multiple Key scopes and records credential owners; remove shared state after the last owner is gone. |
| Caller-origin lifecycle IDs reverse to caller values; provider-origin IDs receive stable downstream aliases | Confirmed: P5 D13 | Subscription lifecycle/correlation domains only, under ownership checks. |
| Private state may retain schema-recognized raw ID pairs only | Confirmed exception: P5 D14 | Bounded owner-only state; request/response bodies, content and opaque arbitrary values remain excluded. |
| Do not recursively rewrite opaque text, encrypted content or external resource IDs | Confirmed: P5 D15 | Typed correlation domains are separate from strings that happen to look like IDs. |
| Corrupt/unsupported state fails only the affected account before delivery; preserve evidence | Confirmed: P5 D5/D6 | No silent reset, rotation or fallback to raw identity. |
| Keep request-state v1 without a historical compatibility layer | Confirmed: P7 D6 | Coordinator SQLite migrations are separate from this request-state format choice. |
| Inactive turn/item/compaction/wire-ID details become pruning-eligible after 30 days | Confirmed: P5 D4, with later capacity amendments | Identity nodes and active/live protections have their own rules. This is not the three-hour body-cache TTL. |
| Public provider request-ID header names may be retained with gateway aliases | Confirmed: P7 D2 | Applies at the public gateway header boundary. API-key exemption from body/frame lifecycle-ID rewriting does not remove this request-correlation header protection. |
| Retain one bounded raw provider request ID in request diagnostics | Confirmed: P7 D9 | Nullable SQLite detail value, subject to default seven-day detail retention; not normal logs or public raw headers. |
| Unknown provider response headers are removed unless explicitly safe | Confirmed: P7 D10 | This response-header policy does not establish that native W3C request headers should be removed. |
| Unsafe/malformed non-2xx bodies on the emulated path become bounded gateway errors | Confirmed: P7 D4 | Preserve the safe public error contract without forwarding unclassified provider content. |
| Commit compaction only from its matching successful completed response | Confirmed: P7 D5 | Failed/incomplete/error events do not commit. This is a necessary boundary, not permission to accept completed output that native compaction rejects. |
| Overlapping compactions from one committed base advance that base once | Confirmed: P7 D11 | Later same-base successful completions are idempotent. |

## Administration, usage and security

| Policy | Origin | Current scope |
|---|---|---|
| Core owns browser PKCE, device-code authentication and serialized refresh | Confirmed: P1 D002 | Do not delegate production auth lifecycle to a running installed Codex client. |
| Remote/headless login uses device-code first; browser PKCE remains explicit | Confirmed: P1 D015 | Authentication traffic in automated tests still uses loopback mocks. |
| Core credential material stays in an atomic owner-only 0600 vault | Confirmed: P1 D011 | The current vault is not encrypted at rest; do not claim that it is. |
| Disable is reversible and service-side; OAuth revoke is explicit | Confirmed: P1 D014 | Revoke upstream before deleting service state. If upstream revoke fails, preserve the disabled vault for retry/recovery. |
| Usage records and statistics are primarily grouped by downstream Key | Requested: P1 D004 | Immutable Key binding preserves attribution. |
| Request-detail retention is configurable, default seven days | Confirmed: P1 D009 | Distinct from context cache and identity-detail retention. |
| Daily per-Key aggregates survive detail pruning indefinitely | Confirmed: P1 D012 | Aggregate deletion is an explicit separate operation. |
| Streaming keeps standard response shapes and upstream usage semantics | Confirmed: P1 D010 | Request-ID/TTFB headers and normalized history/CLI metrics supply observability; do not invent additive protocol events for metrics. |
| Plain HTTP is allowed only on loopback; every non-loopback listener needs native TLS | Confirmed: P1 D016 | Includes LAN/VPN/public addresses; reverse proxy is optional. |
| Automated auth/inference tests do not contact real provider endpoints | Requested: P1 D005; current repository rule | Real native binaries use synthetic auth and loopback captures. |
| Do not log or snapshot secrets or request/response bodies | Confirmed privacy scope: P1/P3/P5 | The narrow private ID-pair and request-diagnostic exceptions above do not permit body retention. |
| README states the requested personal learning/research purpose | Requested: C19 | Documentation policy; the current Disclaimer section retains it. |

## Delegated defaults and implementation details

The user authorized execution after review; the P10 execution contract explicitly says this did
not individually select every value below. These are current configurable/default mechanisms,
not native constants or claims about who originally proposed them.

| Detail | Current selection |
|---|---|
| Request/selected outgoing frame and response bounds | 128 MiB |
| Retained context/index/live descriptors plus assembly reservations | 2 GiB per Core, 1 GiB per Key, 256 MiB per session; not an RSS limit |
| Response record/output-item caps | 8,192 |
| Expiry sweep | 30 seconds; lookups check expiry immediately |
| Exact direct session header/validation | Start with session-id only; reject same-priority conflicts; bounded opaque IDs |
| Inferred turns | Explicit turn first, then validated tool/context ownership; quiescent new user input starts another turn |
| Concurrency | One inference per resolved execution branch/turn and selected WS; reject conflicts. Different explicit turns on separate connections can overlap on one thread. Native public turn/start instead steers active-thread input into the existing turn. |
| Retry | At most one additional attempt for proven-unsent public inference; empty upstream rejection allowlist; no hidden replay after uncertain send or delivered events |
| Body-retention overflow | Finish valid delivery and mark context unavailable, never publish truncated usable history |
| Lightweight live WS retention | Bounded independently of bulk history, retaining required ownership/mappings/turn state |
| Optional ordinary message IDs | Guarded omitted non-reference ID association; explicit IDs and dependencies remain constraints |
| Compressed index | SHA-256 buckets with exact comparison, interned items, compressed radix edges, shared immutable blocks |
| Routing-state acquisition extension | Gateway handshake/metadata acquisition and HTTP body replay details go beyond the exact native producer paths; they are not separately user-selected fixed exceptions |

Other existing operational constants (for example per-Key WS admission and socket deadlines) are
documented in README/source. They should not be promoted to individually requested policies merely
because they exist in code.

## Superseded decisions

| Earlier rule | Current replacement |
|---|---|
| Device UUID owned separately by each local credential | P5 account-level device state, with per-credential mode/revision/pools |
| API-key native emulation or device-carrier rewriting | P10 unconditional API-key passthrough |
| Off means exposing caller installation identity unchanged on Subscription | Scoped installation pseudonyms; off only changes convergence cardinality |
| Stateless UUIDv8/HMAC output identities and automatic same-input convergence across hosts | Persisted genuine UUIDv4/v7; independent new state directories can differ |
| Codex v0.149.0 reference | v0.153.4; preserve explicit gateway exceptions |
| Old source-dependent routing and conversation/cache/request-ID locator fallbacks | Credential-based routing and the P10 explicit locator/anonymous pool contract |
| HTTP increments forwarded as explicit previous-response requests on emulated paths | Full HTTP reconstruction or pre-inference error |
| Unknown-field deletion based only on an old fixed evidence union | P10 lossless classification/repair and current native-supported semantics; do not silently drop newly supported structured or extensible data |
| Earlier request/state byte caps | Current resource-specific limits; 3-hour bulk history, 30-day identity-detail eligibility and 7-day request details are distinct |
| Ten-minute or one-day history-cache proposals | Final user choice: three hours |

Historical one-off instructions to push, deploy to a named host, omit a deployment backup, or remove
a worktree apply to their authorized delivery. They are not permanent automatic deployment rules.
The current comparison request explicitly uses ordinary sub-sessions and excludes simple-review/
deep-review workflows; that is a collaboration preference, separate from product behavior.

## Native behavior and observed defects are not extra user policies

Native Lite UUIDv5 derivation, instruction grouping, request reuse eligibility, routing first-value
lifetime, trace placement and accepted compaction transitions come from the pinned native source.
The gateway should follow those semantics subject to the explicit policies above.

The Plan 13 captures found unintended differences: prewarm routing-token loss, rejected historical
child turns, premature compaction-window commit, inconsistent parent carriers, lost header-only
lineage, independent root-fork rejection, removed W3C HTTP headers and dropped supported custom turn
metadata. Those are not fixed requirements; the subsequent compatibility repair fixed all eight and
converted their observations to conformance regressions. Extra Lite-prefix attribution and token-budget text IDs
versus projected metadata also require explicit evaluation; preserving caller text and typed privacy
domains does not by itself prove that every resulting native relationship is consistent.

## Decision sources

- **P1**: `ref/plans/recent-month/PLAN_1_mini-sub2api-initial-service.md`, D001–D016.
- **P2**: `ref/plans/recent-month/PLAN_2_responses-websocket-passthrough.md`.
- **P3**: `ref/plans/recent-month/PLAN_3_credential-device-fingerprint.md`, D1–D16.
- **P4**: `ref/plans/recent-month/PLAN_4_source-aware-responses-emulation.md`; many routing/filter choices were superseded.
- **P5/P6**: `ref/plans/recent-week/PLAN_5_request-identity-state.md` and `PLAN_6_request-identity-review-fixes.md`.
- **P7**: `ref/plans/recent-week/PLAN_7_codex-profile-identity-hardening.md`, D1–D12.
- **P9/P10**: `ref/plans/recent-3-days/PLAN_9_caller-instruction-preservation.md` and `PLAN_10_http-full-request-continuation.md`.
- **P10 execution contract**: `ref/architecture/plan-10/plan10-execution-contract.md`; selected defaults and supersession of earlier open recommendations.
- **C18/C20**: archived `CHANGELOG_18_runtime-codex-transport-identity.md` and `CHANGELOG_20_codex-tui-identity.md`.
- **C19**: `ref/changelogs/recent-month/CHANGELOG_19_codex-base-instructions.md:46`, the requested README disclaimer.
- **R20/R22**: archived runtime transport identity and Codex TUI identity reviews.
- **Current source/testing**: [README.md](README.md), [NATIVE_PARITY.md](src/coordinator/integration/NATIVE_PARITY.md), and the Plan 13 policy-provenance and capture reports under `ref/architecture/plan-13/`.
