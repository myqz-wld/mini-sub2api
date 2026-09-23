# Codex 0.156.0 compatibility

[Behavior](BEHAVIOR.md) · [Capture suite](../src/coordinator/integration/NATIVE_PARITY.md)

The Subscription emulator targets [Codex 0.156.0](https://github.com/openai/codex/releases/tag/rust-v0.156.0),
source commit `fe74a774532af67b5a4a3dec03ce9469e17f89af`. The release source and official macOS arm64
CLI were compared with 0.153.4. API-key bodies and valid WebSocket frames remain transparent.

## Behavioral ownership and assessment

Three sources determine the current behavior. Native-shaped output alone does not mean every
gateway decision is a Codex requirement or a separately selected user policy.

| Source | Current behavior | Purpose / assessment |
|---|---|---|
| User-selected gateway policy | Each Key binds one credential; API-key bodies/frames pass through, while every Subscription caller receives emulation. | Keeps credential routing explicit and preserves the API-key contract. |
| User-selected caller semantics | Preserve supplied instructions/tools/environment; omit absent or invalid bases; do not insert model-default prompts or discover client AGENTS/Skills. | Avoids changing the caller's task. Native CLI prompt/environment construction remains client-owned. |
| User-selected isolation | Scoped reversible session/thread/turn/item/response IDs; account-level installation convergence in device mode. | Separates callers and preserves correlation. UUID formats follow native conventions, but the aliasing and convergence policy is gateway-specific. |
| User-selected continuation | Keep the caller's transport; resolve eligible anonymous histories and referenced continuations; rebuild HTTP full context; expire local history after three business-idle hours. | Supports ordinary clients. These ownership, matching and retention rules extend native client behavior. |
| User-selected output/parameter policy | Always retain upstream reasoning ciphertext, filter public visibility by include; keep the established Subscription sampling/output-limit filter. | Maintains usable continuation state and the existing backend compatibility contract. It is not a universal Responses API promise. |
| Native protocol/model behavior | Ordinary/Lite layouts, model defaults, field omission/order, zstd/WS framing, UUIDv5 setup IDs, routing-token lifecycle, reuse comparisons and 0.156.0 control/metadata fields. | Follow exact pinned source and actual CLI captures. Native random key ordering is preserved rather than sorted. |
| Gateway implementation defaults | Pin a Codex TUI identity using the Core runtime; synthesize missing root-agent/timing/analytics metadata and model catalog flags; choose backend Guardian metadata for bare non-reviewer requests. | Supplies a consistent emulated session. Individual fallback values are not all explicit user choices or evidence of the caller's real execution environment. |
| Gateway implementation mechanisms | Infer omitted turns; suppress additional gateway prewarm/incrementality for callers with Originator; choose bounded storage, admission, retry and timeout mechanisms. | Prevents duplicated client automation and bounds resource use. Exact budgets/mechanisms are implementation choices, distinct from native wire behavior. |

The design is coherent for a stateful Subscription compatibility gateway: the caller owns task
content and tool execution, Codex defines the upstream protocol, and the gateway owns isolation,
continuation and resource limits. No blocking design inconsistency was found in this targeted
assessment and the retained regressions; this is not an independent whole-repository review.

The main compromise is synthesized execution metadata for ordinary clients. Missing sandbox values
use the established `danger-full-access` / `none` fallback; root-agent, analytics and review defaults
describe the emulated request, not verified client capabilities. Supplied supported values remain
authoritative within the identity/transport policy. Core has no native Code Mode JavaScript host,
client tool execution or client memory-writing service. Explicit nullable/public controls can also
differ from what the native CLI constructs. These differences should remain explicit compatibility
choices when updating the pin, rather than silently acquiring new defaults.

Previously added Lite setup attribution and HTTP body routing tokens were implementation deviations,
not required user customizations; the wire-shape corrections below remove them.

## Changes from 0.153.4

| Native change | Gateway behavior |
|---|---|
| `configuration_update` history items | Preserve `reasoning.effort`, order and repetition; add no item ID or turn stamp. |
| Turn analytics and execution metadata | Preserve `analytics_enabled`, model and effort; synthesized metadata reports analytics disabled. |
| Root fork cache affinity in `session-id` | Keep explicit body session ownership separate from the shared cache key, with scoped aliases. |
| Guardian inference uses `/responses` with `x-codex-guardian` | Retain the header, omit reviewer service-tier/routing hints and restore known parent response aliases. Missing/cross-Key references fail before inference. |
| Executed-tool result evidence | Preserve source lists and opaque result metadata without recursively interpreting IDs inside tool data. |
| Memory consolidation turn IDs | Preserve and project explicit turn/root-turn IDs while leaving absent memory turns absent. |
| Model catalog/personality changes | Use nine catalog profiles and literal upstream prompt fixtures; preserve caller instructions. Removed catalog entries follow normal prefix/fallback selection. |
| Rustls 0.23.45 | Match the release's rustls, webpki and AWS-LC versions for WebSocket TLS. |
| Late tool-result metadata and WS reuse | Changed result metadata, call name, arguments or position require a full request; unchanged evidence allows a delta. Configuration updates participate in reuse checks. |

The ordinary Responses request structure, HTTP zstd encoding, WebSocket `response.create` envelope,
prewarm/continuation protocol and `responses_websockets=2026-02-06` marker remain unchanged.
Legacy native `/responses/compact` capture expectations were retired with that upstream client path;
streamed and local compaction remain covered.

## Field order and presence

The follow-up audit compares ordered JSON tokens from captured bytes, before converting objects to
unordered Go maps. It checks every nested object and array, including serialized turn metadata;
missing/extra fields, duplicate keys, `null`, empty strings/containers and boolean values are distinct.
The same assertion now runs in the native message and transport-lifecycle comparison helpers.

| Surface | Verified rule / correction |
|---|---|
| WS envelope | `stream` follows `store`; it was incorrectly appended after other fields. |
| `client_metadata` | Native uses a randomized Rust `HashMap`; independent CLI processes produce different key orders. Preserve each complete native carrier's incoming order, including the slot replaced by a trusted routing token. Synthesized carriers remain randomized. |
| HTTP routing token | Learn it from response headers and replay it only as a header. Do not learn it from SSE `response.metadata` or add it to HTTP JSON. |
| Lite setup | Native `additional_tools` has no message metadata; its base message has an empty metadata object. Generated setup follows that shape; proven native prefixes retain supplied fields without invented `turn_id`/`create_time`. Business-item metadata remains intact. |
| No-tool ordinary requests | Emit `tools: []` when the caller omits tools, matching the native non-Lite builder. Lite still omits top-level tools. Explicit caller `null` follows the existing caller-control policy. |
| Built-in provider | OpenAI adds `Version: 0.156.0` and backend-gated `guardian_credits_requested`. A custom `/v1` provider is insufficient as the baseline for these fields. Preserve native optional metadata; synthesize the Guardian credit flag for bare non-reviewer Subscription callers. |
| HTTP/WS headers | Compare the complete ordered name list and original casing, with no optional-header exclusion in the built-in capture. Check stable values separately from scoped identity/credential/nonce values. |

The dedicated built-in fixture runs the official CLI in `app-server` mode through a loopback backend
alias and a separate synthetic account-discovery endpoint. HTTP is selected by the native client's
fallback after the fixture rejects WS upgrades. Two transports, two model formats and two independent
processes per combination exercise 28 requests: prewarm, a tool loop and a second turn.
All JSON shape and complete header order/presence/casing comparisons must match; these are assertions,
not an allowlist of observed differences.

## Actual CLI capture evidence

The official CLI and matching code-mode-host ran in isolated configuration/project directories with
synthetic credentials. Auth/inference endpoints were loopback mocks, with outbound networking blocked
for the native child. Raw bounded captures stayed in memory; retained results contain no payloads.

- Eight gateway cases captured 28 real native requests: HTTP/WS, ordinary/Lite and both credentials,
  including a tool result and a subsequent user turn.
- Six direct/API-key/Subscription cases captured 21 requests while changing reasoning effort from
  low to high. The capability was enabled explicitly in a temporary model catalog because it is opt-in.
- Six Code Mode cases exercised the matching native host and actual nested tool callbacks.
- HTTP and WebSocket ClientHello checks matched stable TLS capabilities for both credential types.
- The complete native comparison suite passed 1,080 leaf cases (1,161 checks) with the race detector,
  including model defaults, environment text, compaction, reconnects, continuations and failures.
  The final built-in fixture also passed all 28 requests after adding positive HTTP-header routing
  tokens alongside event-only negative controls. Earlier prefix/token exceptions are no longer allowed.
  After the final omitted-tools correction, all 114 affected native cases and the standard suite
  passed again, including the strict built-in captures.

These checks establish local client/gateway compatibility. They do not establish live-provider
acceptance, model entitlement or remote fingerprint classification. Subscription rewrites scoped
identities, credentials and turn timestamps; complete payloads/compressed bytes consequently differ.
The wire-order fixture uses HTTP/1.1 and WS; TLS evidence covers macOS ClientHello capabilities,
not Linux, negotiated HTTP/2 SETTINGS/HPACK or packet timing. Bare/third-party requests retain the
documented caller-control policy (including explicit nulls and omitted bases) and acquire no native
environment or Code Mode runtime. They are not claimed to reproduce every default CLI request.

## Local validation

Rust workspace tests (472 Core + 7 protocol), Go tests with the race detector, formatting, clippy,
vet, exact upstream prompt regeneration and the release build passed. Installed-artifact checks and scans of the
generated binaries found no private home/repository prefixes or deployment identifiers.
