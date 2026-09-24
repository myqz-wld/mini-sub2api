# Gateway behavior

[Setup](../README.md) · [Operations](OPERATIONS.md) · [Tests](../src/coordinator/integration/NATIVE_PARITY.md)

## Routing

Each distribution Key binds one credential; Keys may share it. There is no account pool, automatic
switching, Chat Completions or conversation-management API.

| Upstream | Every caller |
|---|---|
| API key | Bodies/valid WS frames remain byte-transparent; gateway auth, admission, usage and safe headers still apply. Caller `X-Codex-Routing-Hint` survives; omission stays absent. |
| Subscription | Codex 0.156.0 normalization and scoped identities; HTTP zstd level 3, WS JSON. The model/service tier determines the routing hint. |

HTTP stays HTTP and WS stays WS, including recovery. Nonblank `Originator` disables gateway-added WS
prewarm/automatic incrementality; it never bypasses emulation.

| Subscription input | Upstream send |
|---|---|
| Full HTTP | Full HTTP; public SSE or aggregated JSON follows caller preference. |
| HTTP + valid previous ID | Exact referenced local history + all current input, without the reference. Missing history fails. |
| Full WS | Full frame, or eligible same-socket baseline + suffix. |
| WS + valid previous ID | Valid remote continuation; socket/format/setup changes may require complete local history for full WS. |

A previous ID selects that response and appends all input, including repetitions. Prefix association
alone does not authorize WS reuse; compare actual settings, input/output, thread and connection.

## Sessions and matching

Within Key/account scope, locate by original handshake `session-id` → `client_metadata.session_id` →
turn metadata session ID → known previous response → eligible history/checkpoint → new session.
For native cache-affinity requests where `session-id` carries `prompt_cache_key`, an explicit body
session ID remains the owner. Scoped cache aliases may be shared across forks without merging their
sessions; arbitrary bare-client cache hints do not create session ownership.
The first WS frame binds the session; later frames reject cross-session evidence and supply current
turn/window metadata. Reconnect resolves again. Conversation/cache/request/thread/window IDs are not
session locators; mapped top-level `conversation` is compatibility evidence, not full local history.

OpenCode's enabled built-in OpenAI plugin sends `session-id`/`originator`. Custom-provider
`X-Session-Id`/`x-session-affinity` are not locators. Unidentified full requests match only the same
Key's anonymous pool; explicit sessions stay separate.

Compare normalized caller input and retained public-ID output before upstream identity/Lite conversion:

- Use ordered structured content and completed-response boundaries. Preserve duplicates, text,
  arguments and ciphertext; object-key order is irrelevant.
- Ignore empty annotations/logprobs on assistant `output_text` and completed status on
  message/direct-tool items. Nonempty decoration remains significant.
- An assistant message's top-level `metadata` may be omitted during lookup. Explicit metadata
  conflicts remain significant; stored histories and provider completion checks remain exact.
  This does not ignore nested business metadata or metadata on user/tool items.
- Message IDs may be omitted; direct-tool item IDs may be omitted only with the same valid
  `call_id`. Explicit and other resource/reference IDs remain strict.
- Validate IDs/dependencies before longest-prefix selection. Calls require nonblank IDs; each result
  consumes one known call. Conflicts cannot be resolved by recency.
- Current model/instructions/tools/settings do not determine association. Use current effective
  settings; ordinary bases never inherit. Historical developer/Lite input prefixes remain content.
- Missing/null reasoning ciphertext matches only a field Core previously hid in that verified
  history. Restore that field after full-context and source-thread/fork checks; removing an entire
  item or other content does not qualify. Exact storage and provider-output checks remain separate.

Without an eligible anonymous prefix, an exact last compaction item—including ID/ciphertext—may
locate a unique completed checkpoint in the same Key's anonymous pool. Normalize internal metadata,
restore session/thread/window, validate lineage/dependencies, and use the caller's current replacement
and settings. This grants neither a prior turn nor WS reuse. Changed/missing IDs, external summaries
and conflicting owners fail; checkpoint evidence shares history expiry, eviction and memory budgets.

Explicit turns win; tool follow-ups retain their turn, while quiescent new user input starts one.
Each branch/turn and physical WS permits one inference. Equivalent contexts may share immutable
content, never execution owners/tool consumption. Current-thread, ancestor or declared-fork history
still requires the same session, including remote-only continuation.

Retain the first upstream turn token across same-turn retries/reconnects. HTTP learns it from
response headers and replays it only in headers; ordinary WS learns `response.metadata` tokens,
ignores the upgrade handshake token, and
carries the selected token in `response.create` metadata, preserving a native carrier's key order.
Synthesized metadata includes the token in native-style randomized key ordering. Routing tokens are
opaque header values with a separate 64 KiB limit, retain the first value (including an empty one),
and consume actual retained-state budget; the 512-byte logical-ID limit does not apply. WS metadata
accepts a string or the first array value, matching the pinned native parser.
Failed-turn facts follow idle expiry/capacity; admitted work is protected. Completed prewarm may
transfer its token only to the first business turn on that socket/thread. Hidden setup updates the
business frame before sending. Native memory-consolidation turn/root-turn IDs are projected when
present; absent memory turns remain absent. Core provides no memory-writing service.

## Instructions and Lite

| Caller format | Base and tools |
|---|---|
| Ordinary | Keep nonblank top-level instructions verbatim; omit missing/null/blank/nonstring bases. Top-level tools; omitted tools serialize as `[]`. |
| Ordinary → Lite | `additional_tools`, optional valid caller-base developer message, original input; remove top-level base/tools. |
| Formed Lite | Preserve input instructions; place an explicit valid top-level base after tools. |
| Inherited Lite increment | Validate referenced caller format/setup; do not repeat the prefix. |

No path inserts model-default bases or renders caller placeholders. Ordinary continuation does not
restore an omitted base. Preserve developer content/order/duplicates; Subscription maps
system→developer in place. [Pinned snapshots](../src/core/codex/prompts/codex-0.156.0/README.md)
are offline test fixtures only.

Lite UUIDv5 uses OID + thread UTF-8 as its namespace, then exact serialized tools bytes (`at_`) or
base text (`msg_`). Regenerate only Core-owned/proven native prefixes; retain durable aliases.
Generated setup carries no turn/time attribution: the tool prefix omits message metadata and the
base prefix also omits metadata when classification is disabled. Proven native setup preserves the caller's supplied fields.
Group loose functions/custom tools and function namespaces in encounter order, preserving duplicates
and the last nonblank description.

Preserve caller workspace/personality/AGENTS/Skills/permissions/environment text. Core discovers or
executes none of these or client tools. Legal sandbox meaning survives with Core OS implementation;
root-agent/time/REPL defaults and valid overrides do not enable execution. Filter unsupported native roots, including `max_tool_calls`, `top_logprobs`, `background`, `prompt`
and `stream_id`, with bounded diagnostics. Supported field details live in the
[protocol](../src/protocol/v1/README.md#subscription-request-preparation).

Valid numeric item `create_time` survives absent optional IDs. Retained history preserves first-assigned
time/turn metadata through expansion/matching; explicit caller values and content remain intact.
For `turn_started_at_unix_ms`, preserve nonnegative i64 values from canonical body turn metadata.
Only an absent body carrier permits HTTP-header fallback; WS handshake timestamps never seed create
frames. An explicit value updates that turn's recorded time, including a correction to an earlier
fallback. Omitted/null/invalid values reuse the recorded value; a new turn uses the server clock.
Prewarm/memory omit this field. Retention uses independent server activity clocks, not caller time.
Native exec/wait exposure and host availability differ. Bare API/OpenCode keep direct tools; Core
provides no JavaScript bridge or claim of complete default-native code-mode equivalence.

Codex 0.156.0 `configuration_update` input items retain their reasoning effort and history position
in stored history, without item IDs or turn stamps. The fixed native feature policy disables effort
updates, so all roles omit those items from the sending copy after identity/admission checks. Native analytics flags and opaque executed-tool result metadata
survive normalization. Guardian requests retain `x-codex-guardian`; reviewer requests omit ordinary
service-tier/routing hints, and their parent response IDs require an existing mapping in the same Key.
Bare non-reviewer Subscription callers receive the backend Guardian credit metadata flag. WS delta
reuse compares late executed-tool result metadata and its call binding; changed evidence requires
full input. See [wire-shape evidence and limits](CODEX_COMPATIBILITY.md#field-order-and-presence).

Tool declarations use native function/namespace/tool_search/web_search/custom types; history uses
ResponseItem separately, including image_generation_call and local_shell_call. Tool schema positions
lower `const` to a one-element `enum`, keep the native schema subset and sort property names. Enum
business objects and free output schemas keep their content/order. JSON numbers retain arbitrary
precision. Output format names are `codex_output_schema`; ordinary strictness is true and reviewer
strictness may be false. `ultra` resolves by the model catalog and `persistent` becomes `disabled`;
local metadata may retain the selected alias. Unsupported/default tiers and `summary:none` are
omitted; `flex` remains available. Lite removes image detail, including in structured tool outputs.
Reasoning omits empty content arrays but preserves explicit null. Inner turn metadata follows native
struct order plus sorted extras; outer client metadata keeps its separate HashMap ordering policy.
Legacy item_reference carriers still require scoped ownership, then are logged and omitted from
native sends; callers should supply complete items when replaying content. Named function outputs can omit call_id; explicit null and missing references still fail validation.
The pinned CustomToolCallOutput type still requires call_id. Anonymous reasoning lookup ignores
status, agent, empty content and non-model metadata while checking IDs and ciphertext; provider
item-done/terminal consistency remains strict.

## Reasoning visibility

Ordinary and synchronous reviewer Subscription requests send exactly
`include:["reasoning.encrypted_content"]` and `tool_choice:"auto"`. Async Guardian classifiers send
`include:[]`, `tool_choice:"none"`, no text controls and no invented tier. Missing `include` exposes ciphertext by default; explicit lists expose it only when
requested; null hides optional ciphertext. Other types/non-string entries fail before inference.

Retain complete output before filtering public JSON/SSE/WS. Hide only reasoning
`encrypted_content`; preserve summaries, IDs, compaction ciphertext and opaque tool data.
Visibility is per request/create. Full sending restores verified hidden fields regardless of current
visibility; explicit previous inputs still append wholly. WS reuse compares the restored request.
Provenance expires with history; persisted aliases cannot recover ciphertext. Core never decrypts
or invents state. API-key traffic remains transparent.

## State and limits

Full history/settings/comparison data expire after **3 business-idle hours**, on lookup and a
30-second sweep. Capacity may evict sooner; active work is protected. Live WS facts/tokens survive
bulk expiry. Remote continuation cannot rebuild missing bodies; complete caller input can.
Compaction/injection may require a client replacement before full reconstruction.
Anonymous full replay can import old `internal_chat_message_metadata_passthrough.turn_id` metadata
after source bodies expire, are evicted or disappear on restart. Initial import requires no explicit
session/current-turn/thread lineage, bound WS session, selected baseline/checkpoint, external
conversation or response reference. The caller must supply a user-led history with closed tool
dependencies: messages, direct function/custom calls and results, supplied reasoning ciphertext,
and ordinary/Lite setup. Conflicting declarations of one item ID, item references,
missing ciphertext on reasoning items and unsupported
opaque control items do not qualify. No source body or active source work may remain available.

Each imported turn receives a separate stable identity in the target thread. Its original owner and
aliases are never reassigned. Later full replay, eligible references, descendants and declared forks
can reuse the target's copies. Known downstream aliases still reverse first. Newly imported native output item/call declarations
retain valid native IDs through the existing scoped upstream map; their call/result pairs remain
intact. A prefix never authorizes an external response/item reference. The gateway uses only supplied content and same-Key/account identity evidence;
it never reconstructs expired ciphertext or fetches another scope's history. This checks protocol
self-containment, not equality with unavailable old text. Explicit unrelated-session copies still
require their declared ownership relationship; stable original-session reconstruction is unchanged.
The three-hour history TTL is a code constant; `CONVERSATION_IDLE_TTL` is not a setting in this project.

| Resource | Default |
|---|---:|
| Request / outgoing frame / collected response or frame | 128 MiB each |
| Context/index/live facts/assembly: Core / Key / session | 2 GiB / 1 GiB / 256 MiB |
| Records/session; output items/response | 8,192 each |
| WS/Key; first-frame / idle / write timeout | 8; 30 s / 5 min / 120 s |
| HTTP event idle / per-write timeout | 300 s / 120 s |
| HTTP SSE first output / subsequent output idle | 300 s / 300 s |
| HTTP terminal tail: Core / coordinator fallback | 1 s / 2 s |

These are accounting budgets, not RSS limits. Reserve essential state before inference and measure
WS deltas at their actual size. Oversized retained output may finish valid delivery without publishing
partial history. Override byte/item [defaults](../src/protocol/v1/go/limits.json) using `MINI_SUB2API_LIMITS`:
known fields, positive integers up to 2^40, and `sessionBytes <= keyBytes <= globalBytes`.
See [Memory](MEMORY.md) for small hosts.

Device mode converges account installation UUIDv4 across duplicate credentials/Keys; off uses scoped
aliases while keeping emulation. Logical UUIDv7 identities stay Key-isolated. Mode changes require
disabled/drained credentials; stale WS revisions fail. Ordinary and synchronous-review HTTP clients live for one inference plus internal retries; async
classifiers retain a credential-scoped pool. Infrastructure cookies remain process-wide. Pools/TLS
resumption remain credential-isolated,
without configurable JA3/uTLS or per-credential source-IP/proxy selection.

Private schema-v1 identity files store bounded typed ID pairs, never bodies/raw Keys or arbitrary
text/ciphertext/resource rewrites. Corruption fails for that account without reset/raw-ID fallback.
The file bound is 512 MiB/account; inactive details are pruning-eligible after 30 days, protecting
live/retained dependencies. Installation/session/thread nodes have no fixed TTL.
Startup cleanup rechecks final-owner deletion under lock; unreadable ownership preserves evidence.
Usage details default to seven days; daily aggregates survive.

## Completion and recovery

SSE requires a completed/failed/incomplete/error terminal. EOF, keepalives and `[DONE]` alone cannot
prove success. Premature EOF preserves delivered bytes and reports `upstream_stream / delivered / never`
through failure trailers. Observation follows `outputBytes`, resumes after oversized events, and
never lets later success overwrite failure. API-key payload bytes remain unchanged.

HTTP SSE has separate event and output timers, starting when response-body observation begins.
Only complete, nonempty data events reset event idle. The first output must arrive within 300 seconds;
later output gaps have the same bound. Nonempty text/refusal, reasoning, tool arguments/input/code,
media payloads and completed output items renew output idle. Status/metadata, unknown events, empty
payloads, comments, partial frames and `[DONE]` do not. Go and Rust use the same classification fixtures;
classification is an observation, not completion proof. Subscription SSE/JSON share the Core guard;
Go also guards API-key SSE. From the first terminal, Core receives at most one second of tail data
and validates buffered fragments; Go enforces a two-second fallback. EOF ends earlier. Later
events cannot extend the tail or alter an already closed response. Non-SSE passthrough uses byte
progress; error-envelope inspection is bounded to 300 seconds. Each downstream write/flush has a
120-second deadline. Timeouts/cancellation release request-owned resources; progressing streams
have no total-duration cap. Timeout values are fixed, outside `MINI_SUB2API_LIMITS`.

Publish valid context before public completion; reconcile item-done/final output once. An empty footer
can reuse completed items without duplicate events. Every started item—including created/in-progress
snapshots, item-added and deltas—must finish with matching item-done or a complete final item at the
same index/identity. Missing/truncated footers, unfinished status, conflicting output or unavailable
bounded completion proof reject success across JSON/SSE/WS/prewarm. Cache pressure alone may still
permit delivery when separate completion facts fit. Failed/incomplete responses never become baselines.

Subscription SSE may forward `error` followed by one valid matching `response.failed`, including
projected IDs, details and usage. The first error releases the lane; bounded validation facts stay
reserved until the footer/EOF/tail deadline/cancellation. New output, later success, duplicate/conflicting terminals
fail. Error-only EOF and a valid failed footer need no extra failure trailers, but usage records the
request as failed. Neither path publishes history or commits compaction. JSON aggregation always
rejects an error event. Valid SSE prefix events survive later malformed/oversized chunks; failure
trailers cross the internal HTTP hop even after a previously delivered terminal.

V2 compaction requires matching completion and exactly one valid encrypted item-done; a final array
alone is insufficient. Concurrent same-base commits advance once. With complete source history,
matching observed output and capacity, publish a replacement window under the response ID:

- Explicit V2 retains caller user/system/developer context and formed Lite setup; the actual compaction
  item replaces covered assistant/reasoning/tool/compaction output and removes the trigger.
- In-band compaction retains that item and later output, plus formed Lite's leading setup.
  Ordinary-to-Lite setup still comes from each request's settings.

Full reconstruction uses this exact window and cannot revive removed references. Explicit-reference
intent survives WS expansion and disables hidden prewarm/automatic incrementality. Unsupported or
ambiguous checkpoints, local summaries and dropped calls require complete client replacement; Core
does not decrypt, persist bodies, discover fresh environment or invent truncation choices.

Only proven-unsent business inference permits one extra attempt; the rejection allowlist is empty.
Attempted/uncertain send or delivered events forbid hidden replay. OAuth recovery has a separate bound.
Errors expose `retryAdvice/phase/deliveryState`; missing required state fails before inference.
Subscription non-2xx bodies become bounded errors; API-key bodies pass through.

WS body controls require active session/thread/socket ownership; without a response ID use the bound
operation. Injection invalidates history but keeps validated dependencies; unknown control semantics
also invalidate dependency reuse. Completion cannot restore stale history; full input can rebuild it.
Payload-free transport controls remain forwardable.

Public `X-Mini-Sub2Api-Request-Id` identifies the HTTP request/WS handshake; WS operation IDs remain
in local usage. `Response.id` is the continuation reference. Provider request-ID headers use aliases;
one bounded raw ID may be kept privately. See the [protocol](../src/protocol/v1/README.md).
