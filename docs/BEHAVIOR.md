# Gateway behavior

[Setup](../README.md) · [Operations](OPERATIONS.md) · [Tests](../src/coordinator/integration/NATIVE_PARITY.md)

## Routing

Each distribution Key binds one credential; Keys may share it. There is no account pool, automatic
switching, Chat Completions or conversation-management API.

| Upstream | Every caller |
|---|---|
| API key | Bodies and valid WS frames pass through unchanged; gateway auth, admission, usage and response-header policy remain. |
| Subscription | Codex 0.153.4 request format, defaults and scoped identities; HTTP zstd level 3, WS JSON. |

Caller `X-Codex-Routing-Hint` passes through for API-key HTTP/WS; a missing hint stays absent.
Subscription constructs its hint from the actual request model/service tier.

HTTP stays HTTP; WS stays WS, including recovery. Nonblank `Originator` only disables gateway-added
WS prewarm/automatic incrementality; it never bypasses emulation. Ordinary WS callers may receive
hidden `generate:false` setup and suffix sending when saved config/input/output and thread/socket match.

| Subscription input | Upstream send |
|---|---|
| Full HTTP | Full HTTP; return SSE or aggregated JSON as requested. |
| HTTP + valid previous ID | Exact referenced local history + all current input, without the reference. Missing history fails. |
| Full WS | Full frame, or eligible socket baseline + suffix. |
| WS + valid previous ID | Valid remote continuation; changed socket/format/setup may require locally reconstructed full WS. |

Previous ID means append, including repeated input. It selects that response, never the latest session
history. A local prefix match alone does not prove upstream WS reuse.

## Sessions and matching

Within Key/account scope: original HTTP/WS handshake `session-id` → `client_metadata.session_id` →
turn metadata session ID. Otherwise use a known previous-response association, then an eligible full
history prefix or verified anonymous checkpoint association, then initialize. WS first-frame selection
binds the connection; later frames reject cross-session identities, supply current turn/window
evidence, and reconnect locates again.
`conversation_id`, cache/request/thread/window IDs are not locators. Mapped top-level `conversation`
is only compatibility evidence, not complete local history.

OpenCode's enabled built-in `openai` plugin sends `session-id`/`originator`. Custom providers normally
send `X-Session-Id`/`x-session-affinity`, which are not gateway locators. Bare/custom requests without
references use full-history matching in the same Key's anonymous pool; explicit sessions stay separate.

Matching uses normalized caller input and retained output with public IDs, before upstream identity/Lite conversion:

- Compare ordered structured content, preserving duplicates, strings, arguments and ciphertext.
  Object-key order is irrelevant. Only completed-response boundaries are candidates.
- Ignore empty annotations/logprobs on assistant output_text and completed status on messages/direct
  function/custom-tool items. Nonempty decoration remains significant.
- Message IDs may be omitted. Direct function/custom-tool call/output IDs may be omitted only with
  the same valid call_id. Explicit IDs and other item/resource/reference IDs remain strict.
- Filter ID and dependency failures before choosing the longest candidate. Calls
  require nonblank IDs; results consume known calls once. Conflict never selects by recency.
- Current top-level instructions/tools/model/settings do not determine history association or
  make equivalent histories conflict. Use current effective settings; ordinary bases never inherit.
  WS reuse separately compares its actual transmitted configuration and input/output baseline.
- Historical developer messages and formed Lite input prefixes remain content: editing them can
  break the prefix. Exact content storage and provider-output comparisons remain separate.
- A coarse reasoning key may omit ciphertext, but eligibility accepts missing/null ciphertext only
  when Core recorded that field as hidden in this history. Full context equality still includes it.
  Restore only that verified field; source-thread/fork validation gates sending it. Other content, IDs and
  dependencies remain checked. Removing an entire reasoning item does not qualify as field omission.

Without an eligible anonymous prefix, the last compaction item can locate a uniquely owned,
completed checkpoint in the same Key's anonymous pool. Match the entire item, including ID and
ciphertext, with existing internal-metadata normalization. Restore session/thread/window, validate
lineage and dependencies, then send the caller's current replacement and settings. This neither
inherits the turn nor authorizes WS reuse. Missing/changed IDs or external summaries grant no
fallback; conflicting owners fail. Verified checkpoint-only windows also enter the prefix index.
Checkpoint indexes share source-history expiry, eviction and memory budgets.

Explicit turns take priority; tool follow-ups retain their turn, and quiescent new user input starts
one. Equivalent contexts share immutable content, not active/tool state. Each branch/turn and WS
allows one inference. Historical ownership permits current-thread, ancestor or declared-fork history;
fork provenance grants no cross-session reference authority, including remote-only WS continuation.

Keep the first upstream handshake/metadata turn token until a new turn. Completed prewarm may hand
it to the first business turn on the same socket/thread; another thread cannot consume it. Hidden
setup updates the business frame before sending. Memory metadata uses its native turn-free shape;
Core does not implement a memory-writing service.

## Instructions and Lite

| Caller format | Base and tools |
|---|---|
| Ordinary | Nonblank top-level instructions stays verbatim; omit missing/null/blank/nonstring bases. No model-default insertion. Top-level tools. |
| Ordinary → Lite | additional_tools, optional valid caller-base developer message, original input; remove top-level base/tools. |
| Formed Lite | Preserve input instructions. Insert an explicit valid top-level base after tools; otherwise no fallback. |
| Inherited Lite increment | Validate the referenced caller format/setup; do not repeat its prefix. |

Caller/upstream formats stay distinct. Developer content/order/duplicates survive; Subscription
system→developer happens in place. Caller placeholders stay literal. Effective history settings use
the same no-default base policy; ordinary continuation does not restore an omitted prior base.
[Snapshots](../src/core/codex/prompts/codex-0.153.4/README.md) cover 11 catalog models and fallbacks
for offline comparisons/tests only; production requests never load these prompt defaults.

Lite UUIDv5 derives a namespace from OID + thread UTF-8, then hashes exact tools bytes (at_) or base
text (msg_). Regenerate only Core-owned/proven native prefixes; preserve existing aliases. Group loose
functions/custom tools and all functions namespaces in encounter order, retaining duplicates and the
last nonblank description.

Preserve supplied workspace/personality/AGENTS/Skills/permissions/environment text. Core discovers
none of these or client tools. Sandbox meaning survives while implementation follows Core OS.
Root-agent/time and model REPL flags use existing defaults; explicit valid values survive. Flags do
not enable execution. Filter server-unsupported max_output_tokens/temperature/top_p; retain supported
access programs, sequential_cutoff, HTTP trace headers and allowed turn metadata.

Native exec/wait exposure and host availability differ. Bare API/OpenCode retain direct tools;
Core provides no JavaScript bridge or claim of full default-native code-mode equivalence.

## Reasoning visibility

Subscription always adds `reasoning.encrypted_content` to upstream `include`, preserving other
entries/order. Missing `include` returns ciphertext by default; explicit lists return it only when
requested, and null requests no optional ciphertext. Other types/non-string entries fail before
inference. API-key requests and responses remain transparent.

Before filtering public JSON/SSE/WS, retain full output internally. Hide only reasoning items'
`encrypted_content`; keep summaries/content/IDs, compaction ciphertext and opaque tool data.
Visibility belongs to each request/WS create. Full sending preserves supplied ciphertext and
restores verified hidden fields regardless of current `include`; explicit previous inputs still
append wholly. WS reuse checks the restored request against its real baseline. Hidden-field
provenance expires/evicts with history; persisted aliases cannot recover ciphertext. Core neither
decrypts nor invents encrypted state.

## State and limits

Full history/settings/comparisons expire after **3 business-idle hours**, checked on lookup and a
30-second sweep; capacity may evict sooner, active work is protected. Live WS facts/tokens survive
bulk expiry. Remote continuation does not restore missing local bodies; full input can. Compaction
windows use the same limits and expiry; unsupported compaction or interleaved injection may still
require client replacement history before HTTP reconstruction.

| Resource | Default |
|---|---:|
| Request / outgoing frame / collected response or frame | 128 MiB each |
| Context/index/live facts/assembly: Core / Key / session | 2 GiB / 1 GiB / 256 MiB |
| Records/session; output items/response | 8,192 each |
| WS/Key; first-frame / idle / write timeout | 8; 30 s / 5 min / 120 s |

These are accounting budgets, not RSS limits. Measure a selected WS delta at its actual size.
Reserve essential state before inference; body overflow may finish delivery without publishing
partial history. Override [defaults](../src/protocol/v1/go/limits.json) through MINI_SUB2API_LIMITS JSON:
positive integers, known fields, sessionBytes <= keyBytes <= globalBytes.

Device mode converges installation UUIDv4 across Keys/duplicate credentials for an account. Off uses
scoped aliases, still with emulation. Logical IDs use UUIDv7 and stay Key-isolated. Mode changes need
disabled/drained credentials; stale WS revisions fail. Pools/TLS resumption are credential-isolated;
no configurable JA3/uTLS or per-credential source-IP/proxy mechanism is provided.

Private schema-v1 files contain bounded typed ID pairs, never bodies/raw Keys. Do not rewrite arbitrary
text/ciphertext/resource IDs. Corruption fails for the affected account without reset/raw-ID fallback.
Files are bounded to 512 MiB/account; inactive details are pruning-eligible after 30 days, protecting
live/retained mappings. Installation/session/thread nodes have no fixed TTL; usage details default to seven days.

## Completion and recovery

SSE needs a completed/failed/incomplete/error terminal; EOF, keepalives and `[DONE]` alone do not
mean success. Premature EOF keeps delivered bytes, reports `upstream_stream / delivered / never`
in failure trailers and records an upstream error. The usage observer follows `outputBytes`
(128 MiB default), resumes after an oversized SSE event, and never lets success overwrite failure.
These checks change accounting/diagnostics, not API-key payload bytes.

Publish valid context before public completion. Reconcile item-done/final output once; an empty footer
uses completed items for JSON/history, without duplicate stream events. Failed/incomplete responses
are not baselines. V2 compaction requires matching success and exactly one valid encrypted item-done;
a final array alone is insufficient. Overlapping same-base commits advance once.

When complete source history and a single matching observed compaction item are available, Core
stores a replacement window under that response ID before completion is delivered:

- Explicit V2 with a final compaction_trigger retains caller user/system/developer messages and
  formed Lite setup, preserving content/order. The actual compaction item replaces old assistant,
  reasoning, tool and compaction items; the trigger is removed.
- In-band generation keeps the compaction item and subsequent output. Formed Lite also retains its
  leading tools/developer setup; ordinary-to-Lite setup still comes from each request's settings.

Later HTTP increments and required full WS sends reconstruct from this exact window. Full-history
matching cannot revive item references removed by compaction. Explicit-reference intent survives
full WS reconstruction and disables extra prewarm/automatic incrementality. Core does not decrypt
opaque content, persist bodies, discover fresh environment or apply untransmitted truncation.
Local summaries, multiple/unobserved checkpoints, unsupported explicit input kinds and dropped tool calls require
a client-supplied replacement. A complete window supplied by the client remains authoritative.

At most one extra attempt is allowed while business inference is proven unsent; the rejection retry
allowlist is empty. Attempted/uncertain send or delivered events prevent hidden replay. OAuth refresh
has its separate bounded retry. Errors expose retryAdvice/phase/deliveryState; missing required state
fails before inference. Subscription non-2xx bodies become bounded errors; API-key bodies stay transparent.

Subscription WS body controls must belong to the active session/thread/socket. Without a response ID,
use that bound operation. Injection invalidates full history but retains validated dependency facts;
unknown body-control semantics also invalidate dependency reuse. Completion cannot restore the old
history. Full client replacement can rebuild it; payload-free transport controls remain forwardable.

HTTP/WS handshake req_* is returned in X-Mini-Sub2Api-Request-Id; WS operation IDs stay in local usage.
Response.id is the continuation reference. Public provider request-ID headers use aliases; one bounded
original provider ID may be retained privately. See the [protocol](../src/protocol/v1/README.md).
