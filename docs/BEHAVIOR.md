# Gateway behavior

[Setup and usage](../README.md) · [Operations](OPERATIONS.md)


### Credential routing

Each distribution key selects its bound upstream credential. There is no account pool or automatic
account switching. Caller markers such as `Originator` do not select emulation or authentication.

| Upstream credential | Behavior for every caller |
|---|---|
| API key | Request bodies, valid WS application frames and response bodies pass through unchanged, including Codex-marked callers. Authentication, routing, admission, usage accounting and reviewed response-header policy still apply. Identity/cache failures do not affect this route. |
| Codex subscription | Emulate the supported Codex v0.153.4 request shape, defaults and identity metadata. HTTP uses zstd level 3 upstream; WS application frames remain JSON. |

Subscription fingerprint mode defaults to `device`: one persisted installation UUID per upstream
ChatGPT account, shared across its distribution Keys and duplicate credentials. This does not merge
their sessions, turns or tool state. `off` uses scoped installation aliases instead of account-level
convergence; it still applies Subscription emulation. API-key carriers remain transparent in both
modes. Mode changes require a disabled, drained credential; stale WS revisions are rejected.
Transport pools and TLS resumption remain credential-isolated. Native transport construction has
no configurable JA3/uTLS profile or per-credential source-IP/proxy selection.

### Context and continuation

HTTP stays HTTP, including SSE/JSON response adaptation. WebSocket stays WebSocket during full
sending, optimization and recovery. Neither path falls back to the other transport.

| Subscription request | Upstream behavior |
|---|---|
| Full HTTP, ordinary or Lite | Send the complete validated request over HTTP. |
| HTTP with `previous_response_id` | Append all supplied input to that exact completed response's locally materialized context, then send full HTTP without the reference. Missing context fails before inference. |
| Full WS, ordinary or Lite | Send full WS, or use an eligible completed socket baseline to send a suffix and `previous_response_id`. |
| WS with `previous_response_id` | Continue through that live upstream socket when ownership, mappings and format permit. Complete local history is needed only when the selected transformation or replacement socket requires full sending. |

A valid previous response means append semantics: even apparently repeated history remains in the
input. Earlier-response forks use that response's context. Failed and incomplete responses never
become completed context baselines. Output item events and final output are reconciled once, and
response ownership/context is published before the corresponding public event.
Some upstream completions contain an absent or empty `output` footer after sending completed
items. Subscription JSON delivery reconstructs those items in output-index order; history and WS
reuse retain the observed output. A populated final array is used once. SSE/WS clients consume
the item events normally; the gateway does not add duplicate item events.

Session lookup is scoped by distribution key and upstream account namespace:

1. The original HTTP/WS handshake `session-id` header takes priority, followed by
   `client_metadata.session_id`, then turn metadata `session_id`.
2. Without an explicit session, a known `previous_response_id` restores its owning session.
3. Full history can match completed-response prefixes within a known session or the same key's
   anonymous-only pool. Explicitly identified sessions are excluded from that pool. Structured
   content, explicit IDs and tool dependencies determine eligibility before the longest match;
   recency does not resolve conflicting contexts.
4. A WS connection binds its session on the first request. Later frames inherit it and reject
   cross-session identities/references. Handshake turn/window fields apply to the first frame;
   later frames provide current turn evidence. Reconnection performs session lookup again.

`conversation_id`, cache keys, request IDs and thread/window identifiers do not locate a session.
Existing mapped top-level `conversation` references remain a compatibility boundary, and cannot
establish complete local history. This service does not provide `/v1/conversations` management.

Turns and responses are separate: valid explicit turn identity is authoritative; otherwise known
context and tool dependencies determine continuation. New user input after a quiescent completed
context starts a turn. Tool follow-ups retain their turn. Equivalent anonymous contexts can share
immutable content while retaining independent execution state. Each branch/turn and physical WS
allows one inference at a time. Identity edits commit only after admission; cache identity facts
publish after that commit.

Original first-request headers retain their child-thread lineage and window number. Later WS
frames inherit the bound branch and supply current window/turn metadata. Historical input keeps
its original turn and causal parent within the current thread, its ancestors or an explicitly
declared fork source. Unseen historical IDs reserve aliases until an actual request establishes
ownership. Independent root forks retain their own session; fork provenance grants no response
continuation authority.

Explicit WS references validate the effective history's turn ownership with the same rules as full
HTTP/WS input. Bounded identity facts survive bulk-history expiry for this check. A previous-only
continuation restores its own thread's known fork provenance; a new branch supplies its own source
relationship. Automatic WS optimization also requires the saved and current thread to match.

The first upstream `x-codex-turn-state` from a handshake or `response.metadata` is retained within
its turn. A new turn starts without the prior token. That token is distinct from the generated or
mapped UUIDv7 turn ID. Optional v0.153.4 window, fork, trigger and history-ingest metadata is validated
and forwarded without becoming session identity. Context-window UUIDs receive scoped aliases.

A completed native WS startup prewarm can hand its first `response.metadata` routing token to
the first business turn on that same socket and thread. Failed prewarms, other sockets/threads and
later turns cannot inherit it. Bulk-history expiry preserves this live startup state.
Hidden setup also attaches the learned token before encoding the first business frame. Work on
another thread leaves the waiting owner's startup token available; failed setup and reconnection
discard unaccepted setup state.

Remote compaction V2 advances a window only after a matching completed response and exactly one
valid encrypted compaction item received through `response.output_item.done`. A final output array
alone cannot establish acceptance. Duplicate, missing or inconsistent compaction output cannot
publish a usable context or WS baseline. Local Responses compaction retains the native assistant-summary
completion rules.
An absent or empty final footer is allowed after that single valid item-done proof; it does not
relax the proof requirement.

HTTP `traceparent` and `tracestate` are preserved across both credential routes. Native WS tracing
remains in per-frame metadata. Nonreserved string entries in `x-codex-turn-metadata` survive body and
HTTP compatibility-header projection under the existing request/assembly limits. Native app-server
extras can exceed the separate config-file entry/key/value limits; canonical identity fields still
receive their scoped projections, and body-only tool namespace metadata stays out of headers.

Ordinary callers may omit workspace, agent and execution context. Core adds no discovered CWD,
AGENTS, Skills, permission messages or tool definitions. Generated protocol metadata keeps the
existing root-agent and turn-time defaults; missing `node_repl_auto_review_required` and
`node_repl_disabled` follow the pinned model catalog (currently Astra requires auto review; all
11 models default `node_repl_disabled` to false). Explicit valid caller values remain intact. These flags describe
client policy; the gateway does not run a REPL or enable tools. Subscription still removes the
established server-unsupported controls, including `max_output_tokens`, `temperature` and `top_p`.

### Retention and limits

Complete history, comparison snapshots and prefix-index membership expire after **3 hours of business
inactivity**, checked at lookup and by a 30-second sweep. Active operations are protected. Expiry does
not disconnect a live WS, rotate required aliases or clear its valid turn token. A full request can
rebuild materialized history. Expired HTTP increments fail; valid live WS increments may continue
through upstream history. A remote-only response stays unmaterialized locally until full history is
actually supplied.

| Resource | Default |
|---|---:|
| Accepted request and selected outgoing JSON/frame | 128 MiB |
| Collected response / WS response frame | 128 MiB |
| Retained output items per response | 8,192 |
| Context, index, live metadata and assembly reservations per Core | 2 GiB |
| Per distribution key | 1 GiB |
| Per session | 256 MiB |
| Completed response records per session | 8,192 |

The cache budgets account for retained objects and assembly reservations; they are not a process-RSS
limit. Protocol buffers have separate request/response bounds. A small WS delta is checked at its
selected frame size, independently of the expanded history size. Essential capacity is reserved
before inference. Completed body-retention overflow preserves otherwise valid delivery and marks
its context unavailable; it never publishes truncated history as usable. Compaction whose replacement
history depends on client retention choices, and interleaved injection, require a subsequent full
request to establish reconstruction proof.

[Shared limit defaults](../src/protocol/v1/go/limits.json) and the `MINI_SUB2API_LIMITS` JSON environment
variable apply across Go and Rust. Operators can override individual fields, for example:

```bash
export MINI_SUB2API_LIMITS='{"requestBytes":134217728,"outputBytes":134217728}'
```

Values must be positive integers, with `sessionBytes <= keyBytes <= globalBytes`. Unknown fields,
invalid types and invalid hierarchies are rejected. Caller metadata cannot override these settings.
Authentication/error-body limits remain separate. Each key may hold eight WS connections; existing
first-frame, idle and write limits remain 30 seconds, 5 minutes and 120 seconds.

Private identity files retain UUID assignments and reversible schema-owned ID mappings, with no
request/response bodies or raw distribution keys. They remain schema v1, bounded to 512 MiB per
account namespace, with inactive detail eligible for pruning after 30 days. Live/retained context
mappings are protected independently of bulk history. Installation IDs use UUIDv4, session/thread/
turn IDs use UUIDv7, and generated Lite prefix IDs use native UUIDv5 thread/payload derivation.

### Delivery and recovery

A missing required mapping or context fails as `state_unavailable` before inference. Full WS sending
on a replacement connection requires complete local history. Automatic WS recovery permits at most one
additional attempt only while public inference is proven unsent; the upstream rejection retry
allowlist is empty. An attempted send, uncertain completion or any delivered current-response event
prevents hidden replay. OAuth handshake/authentication refresh remains bounded to its existing single
retry. A caller may submit a later full request after a surfaced failure.

Failure metadata exposes `retryAdvice`, `phase` and `deliveryState`. HTTP failed/incomplete/error
terminals remain valid Responses output and count as upstream errors. Subscription non-2xx bodies
are bounded gateway errors; API-key bodies stay transparent. Provider response headers are reviewed:
public request-ID headers use gateway aliases, while one bounded provider ID may be retained in local
diagnostics. Sandbox names follow the gateway OS, preserving permission meaning and caller workspaces.

See [the v1 protocol reference](../src/protocol/v1/README.md) for transport and failure contracts.

### Base and developer instructions

Subscription emulation prefers a nonblank caller `instructions` string verbatim, including whitespace
and literal template syntax. Missing, null, blank and non-string bases use the selected model default
only when a base is required.

| Caller shape | Placement |
|---|---|
| Ordinary Responses | Selected base stays in top-level `instructions`. |
| Ordinary converted to Lite | `additional_tools`, one selected-base developer message, then original input. Remove top-level `instructions`. |
| Already formed Lite | Preserve input instructions. An explicit valid top-level base is inserted after tools; otherwise add no fallback base. |
| Valid Lite WS delta | Inherit validated caller format and existing setup. Changed setup requires a complete full send when it cannot be represented as the existing delta. |

Developer messages retain content, duplicates and relative order. Subscription converts `system` to
`developer` in place. Caller format, actual upstream format and setup provenance stay distinct, so
ordinary-to-Lite conversion does not reclassify future ordinary input as native Lite.

The [offline snapshots and generator](../src/core/codex/prompts/codex-0.153.4/README.md) cover all eleven
catalog models plus generic and experimental fallbacks. Native template variables are rendered;
literal examples such as `{{connector_id}}` remain unchanged. Caller text is never rendered. Explicit
supported `access_programs` selection and `sequential_cutoff` summary delivery are forwarded per
response; the gateway does not synthesize account entitlements.

Lite prefix IDs follow Codex v0.153.4: derive a UUIDv5 namespace from the thread ID and the OID
namespace, then hash the serialized tools bytes (`at_`) or exact base text bytes (`msg_`). The gateway
verifies native prefix provenance before regenerating IDs for its scoped upstream thread and final
payload. Existing reversible aliases remain stable for live historical references. Arbitrary caller
item IDs continue through the ordinary mapping path. Object serialization order matters to these IDs;
re-encoding a captured tools object before replay can invalidate its original content-derived ID.

Ordinary-to-Lite tool conversion combines loose functions/custom tools and every default `functions`
namespace in encounter order, at the first groupable position. It preserves duplicate children and
uses the last nonblank namespace description, matching the pinned native producer.

## Native tool execution boundary

On the pinned Astra catalog, native Codex advertises `exec`/`wait` even when its code-mode host
is disabled. Enabling the host permits execution; it does not itself select the tool schema.
An explicit `code_mode.direct_only_tool_namespaces` setting can expose a namespace directly.
The default native path instead calls nested tools through `exec` and returns `custom_tool_call_output`.

The gateway preserves a native client's supplied tool protocol. Bare Responses and OpenCode callers
keep their own direct function calls; Core does not provide a JavaScript execution or exec/wait bridge.
Those direct tools passed bounded real Subscription checks, but their complete request and execution
logic do not thereby equal the default native code-mode path. This is a measured compatibility
boundary, not another user-selected gateway policy. See the
[capture matrix](../src/coordinator/integration/NATIVE_PARITY.md).
