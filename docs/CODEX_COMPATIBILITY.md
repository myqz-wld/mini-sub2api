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
| User-selected output/parameter policy | Retain upstream reasoning ciphertext for ordinary/reviewer requests, filter public visibility by include; classifier requests keep their native empty include. | Maintains usable continuation state and the existing backend compatibility contract. It is not a universal Responses API promise. |
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
client tool execution or client memory-writing service. Invalid optional controls are logged and
ignored before native defaults; required identity and dependency validation still fails closed.

Previously added Lite setup attribution and HTTP body routing tokens were implementation deviations,
not required user customizations; the wire-shape corrections below remove them.

## Turn-start metadata and carrier ownership

Codex 0.156.0 `core/src/responses_metadata.rs` defines the full serialized body
`client_metadata["x-codex-turn-metadata"]` as canonical. Its HTTP compatibility header comes from
the same snapshot, with tool inventory omitted; the normal HTTP builder always includes body
client metadata. A WS handshake has no JSON body, while each response.create has its own metadata.
These phases must not be treated as interchangeable sources of turn state.

The gateway now preserves valid nonnegative i64 caller turn-start timestamps. Body metadata wins;
HTTP headers supply a timestamp only when the body metadata carrier is absent. Missing/invalid
values reuse the recorded turn time, or the server clock for a new turn. Explicit corrections are
retained for subsequent omitted values, including after restart. Prewarm/memory omit this field;
API-key bodies/frames stay transparent. Caller time never drives expiry or retention.

Earlier native assertions allowed timestamp replacement and only checked within-turn stability.
Those exceptions are removed: native tool-loop/root/child/fork/transport captures now require exact
caller timestamp equality. Root capture tests also check canonical metadata in HTTP/WS bodies and
HTTP body/header equality for sandbox, workspace and turn-start fields.

Header-only sandbox/workspace input remains a gateway compatibility boundary: generated body
metadata can replace that header input. It is not the normal native 0.156.0 HTTP request shape.
This timestamp change does not expand sandbox/workspace header fallback or alter permission policy.

Empty or partial body turn metadata receives missing generated `model` and `reasoning_effort`
values. Explicit values survive; complete native metadata remains byte-stable. Native sparse
memory and startup-prewarm shapes keep their existing exceptions. Final identity projection
derives header metadata from the canonical body; header-only model/effort extras do not override it.

## Complete history after expiry or restart

When anonymous callers replay complete content after local history is gone, old per-item turn
metadata no longer forces them back into the old session. A guarded import creates stable target
turn copies using existing same-Key/account identity state, preserving the source owners and
message/tool references. It does not copy hidden body state, enable reference-only recovery or
authorize unrelated explicit-session lineage. See [eligibility rules](BEHAVIOR.md#state-and-limits).

Regression captures cover ordinary/Lite HTTP JSON, SSE and WS across Core restart, followed by
another full turn. Unit cases cover expiry, source preservation, repeated/reference continuation,
restart, target ancestry, incomplete dependencies, retained/running sources, transaction rollback
and Key/account isolation. Actual OpenCode 1.18.29 custom-provider captures on gpt-5.4/Astra also
resume complete history after Core restart. In these two cases OpenCode omitted item turn metadata
even when the synthetic upstream returned it; the specific old-turn failure must not be attributed
to ordinary OpenCode behavior without request evidence. These checks use loopback endpoints only.

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

## Linux TLS builds

The project pins Rust 1.95.0 through `mise.toml`, matching the upstream
`codex-rs/rust-toolchain.toml` and official 0.156.0 release workflow. This removes
the compiler-version difference; it does not claim a measured wire improvement.
The gateway retains its existing Cargo release profile, so this is not full
build-environment identity with the upstream CLI.

Linux HTTP uses the native-TLS backend. The compatibility target is the **official Linux release**
of Codex 0.156.0, whose musl build supplies OpenSSL 3.6.4 outside Cargo. Its unchanged Cargo.lock
still contains OpenSSL 3.6.3; comparing lockfiles alone previously missed the actual release library.
The [official changelog](https://learn.chatgpt.com/docs/changelog) records the musl update.
The exact configuration and SHA-256 come from `.github/scripts/install-musl-openssl.sh` at the
source commit above; `.github/workflows/rust-release.yml` supplies the AWS-LC musl setting.

| Dependency | Pinned version |
|---|---|
| `openssl` Rust bindings | `0.10.75` |
| `openssl-sys` | `0.9.111` |
| Linked OpenSSL C library | `3.6.4`, static |
| OpenSSL source archive SHA-256 | `9bffaa1ad1e07b354c21bd3324ec02fa15579f45a7d0494b3e74bc449b7333ef` |
| Locked `openssl-src` fallback | `300.6.1+3.6.3`; bypassed by the release build |

`scripts/cargo.sh` prepares this exact library for x86_64/aarch64 GNU and musl targets, sets all
target-specific OpenSSL discovery/static-link options and adds `--locked`. GNU also uses the pinned
library as a deliberate gateway policy; a plain upstream GNU source build can use system OpenSSL.
The Core build script rejects a different OpenSSL version or missing static-link selection, and a
Linux runtime test verifies the linked version. macOS/Windows keep their native HTTP TLS backends.

On a Linux build host, install a suitable target C compiler/linker, Make, Perl, curl, sha256sum and
flock. `CC_<target_with_underscores>`, `TARGET_CC` or `CC` selects the OpenSSL compiler; each must
name one executable. Native builds default to `cc`, same-architecture musl cross-builds to
`musl-gcc`. Explicit Cargo `--target` and `CARGO_BUILD_TARGET` select the library target.

```bash
# Online preparation once per compiler/target/configuration; no provider traffic.
mise exec -- bash scripts/prepare-openssl.sh aarch64-unknown-linux-gnu
# Keep the same entry point for build, check, clippy and test.
mise exec -- bash scripts/cargo.sh test --workspace --offline
```

The source download uses HTTPS, retries and a fixed checksum. A lock serializes preparation;
incomplete builds are rebuilt, and the cache key includes the preparation script, target, compiler
version and C flags. `--offline`, `--frozen` and `CARGO_NET_OFFLINE=true` prohibit source downloads;
a missing source archive fails offline, and a checksum mismatch stops rebuilding. Neither case
falls back to a system library. Standard Go integration tests
use this same entry point offline, so prepare the cache first. Native sources/logs stay under
`build/native-openssl/` and are not distribution inputs. OpenSSL installs into a staging directory
with portable embedded paths; C and Rust path mapping protect release binaries from local prefixes.

The related audit checked 43 selected package identities (versions, source and checksums) across
reqwest/native-tls, hyper/h2, rustls/webpki/AWS-LC, upstream WebSocket forks, serde/JSON,
bytes/futures, zstd and zlib-rs. `bytes` was still pinned to 1.11.1 and futures to 0.3.32; these now
match upstream 1.12.1 and 0.3.34. The selected packages match the pinned source. Additional
tungstenite versions belong to inbound Axum or test clients. CLI, build helpers and other
platform/transitive dependencies remain project-owned; this is not a full lockfile identity claim.
The build entry disables zstd system discovery and AWS-LC system-library autodetection, enforces
static AWS-LC, and disables AWS-LC jitter entropy on musl as the official release does.

Validation on Linux ARM64 runs as an unprivileged user with container networking disconnected.
GNU and musl each pass 496 Core and 7 protocol tests, including real loopback HTTPS trust/hostname
checks and the linked OpenSSL 3.6.4 assertion. GNU clippy, cache-concurrency checks and shell
failure-path checks pass. On macOS, the standard test script passes 494 Core/7 protocol tests,
clippy, Go vet/race tests and hostile-proxy OAuth coverage; the release build and installed checks
pass. One existing manual resource benchmark remains ignored. Both musl architecture feature
graphs were checked; no x86_64 artifact was built or executed in this validation.
Both ARM64 release binaries execute successfully. ELF inspection shows GNU depends only on
libgcc_s/libm/libc, with no shared libssl/libcrypto; musl has no shared-library dependencies.
Both contain the OpenSSL 3.6.4 identity and pass build-prefix scans.

Matching versions and build configuration does not certify byte-identical TLS fingerprints.
Custom CA overrides now use native precedence (`CODEX_CA_CERTIFICATE` then `SSL_CERT_FILE`),
append configured roots and select rustls for HTTP. Both HTTP/2 and WSS were verified against a
loopback root/leaf CA; default HTTP native TLS remains unchanged.

## Field order and presence

The follow-up audit compares ordered JSON tokens from captured bytes, before converting objects to
unordered Go maps. It checks every nested object and array, including serialized turn metadata;
missing/extra fields, duplicate keys, `null`, empty strings/containers and boolean values are distinct.
The same assertion now runs in the native message and transport-lifecycle comparison helpers.

| Surface | Verified rule / correction |
|---|---|
| WS envelope | `stream` follows `store`; it was incorrectly appended after other fields. |
| `client_metadata` | Native uses a randomized Rust `HashMap`; independent CLI processes produce different key orders. Preserve each complete native carrier's incoming order, including the slot replaced by a trusted routing token. Synthesized carriers include learned routing state in randomization, including after hidden WS setup; the token is not always appended last. |
| HTTP routing token | Learn it from response headers and replay it only as a header. Do not learn it from SSE `response.metadata` or add it to HTTP JSON. |
| Infrastructure cookies | HTTPS and WSS share the restricted jar, including `__oailb`. Successful and rejected upgrades refresh it; explicit Cookie headers take precedence. Host/path/secure/expiry checks apply, and account/authentication cookies are excluded. |
| Function/schema serialization | Omit local `function.output_schema`, including namespace children. Place `minItems` after `items` and before composition/object fields, matching native schema serialization and Lite UUIDv5 input bytes. Response text schemas and schema property names remain intact. |
| Lite setup | Native `additional_tools` has no message metadata; its base message omits empty metadata with classification disabled. Generated setup follows that shape; proven native prefixes retain supplied fields without invented `turn_id`/`create_time`. Business-item metadata remains intact. |
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

### Ordinary and third-party wire checks

Bare JSON/SSE/WS clients and actual OpenCode 1.18.29 now use an ordered wire oracle in addition to
their existing content/continuation checks. Request/item field order is read from the pinned native
Rust definitions and calibrated against independent actual CLI captures with the built-in OpenAI
provider. The baseline includes a tool loop and a second user turn. Full header names, order and
original casing are compared, including separately captured HTTP routing-token variants; stable
header values are checked separately from credentials, scoped identities and runtime values.

Checks cover top-level presence, explicit nulls, absent/empty bases, ordinary/Lite item envelopes,
tool and schema ordering, nested serialized turn metadata and default boolean/identity carriers.
The synthetic flat `client_metadata` map retains native random ordering; it is not sorted to match
one CLI process. Caller business values remain checked by the existing message/tool oracles.

These checks exposed late mutations that earlier unordered comparisons missed: scoped IDs added
after Lite-prefix/tool-return normalization and `previous_response_id` inserted by automatic WS
continuation appeared at the end of objects. Final Subscription projection/setup/continuation now
restores protocol field order without reconstructing tool/schema bytes or opaque content.

Explicit gateway policies remain part of the oracle: absent bases stay absent; caller nullable
controls are not conflated with omission; unsupported Subscription metadata/sampling/output controls
are removed; `stream_options` retains only supported sequential reasoning-summary delivery.
The subsequent fidelity repair narrows roots/ToolSpec/ResponseItem to pinned native types, resolves
model effort aliases, constructs role-specific choice/include/text combinations, orders inner turn
metadata, preserves imported output IDs and uses metadata events for ordinary WS routing state.
Managed auth now follows reload → refresh with at most three inference attempts. HPACK Authorization
uses native without-indexing (`0f08`), verified from a production-built request on a loopback peer.
Ordinary HTTP pools are rebuilt for each authentication recovery attempt; classifiers retain pooling. OAuth form/query/header
order, exact large integers and free output-schema order were also verified. SSE BOM/CR support is
a compatibility improvement: native 0.156.0 also failed those bounded fixtures. These local results
do not establish byte equality for every release/platform/optional-role combination. A legacy local
shell mock was accepted by native, but no local_shell_call appeared in its subsequent request, so
producer-specific metadata fidelity remains unproven.

Complete and incremental WS requests are validated as separate native schemas: a missing reference
requires complete input, while null/empty references and a suffix without its reference fail.
This is protocol alignment under those policies, not a claim that arbitrary third-party prompts,
tool catalogs or execution environments reproduce a complete default native request.

### Subscription response privacy

The public header policy also applies to `headers` in response metadata/error events. Unknown,
credential, cookie and installation headers are removed; provider request IDs use gateway aliases.
The HTTP request ID is used for HTTP metadata and the gateway connection ID for WS metadata.
Routing state is learned privately before filtering. Existing allowed routing-state, model,
rate-limit and timing headers remain protocol-visible.

Schema-owned errors in JSON/SSE/WS retain a fixed vocabulary of native error codes/types, replace
provider messages with a generic message and discard arbitrary error extensions. The native rate
limit parser's numeric retry delay is retained with a one-day bound, without surrounding provider
text. Error events preserve translated `stream_id` correlation; `error: null` is treated as an
absent nested error so flat `code/message` are sanitized and retained. Subscription SSE preserves
matching event names and data, strips event IDs/retry extensions
and normalizes comments/empty heartbeats. API-key response bodies/frames keep their existing
transparent behavior. User metadata, model text, tool arguments/results and ciphertext are opaque;
this boundary does not claim arbitrary-content DLP.
Incomplete-response reasons are also restricted to known protocol categories. The error regression
derives 13 classifications from the pinned SSE consumer, including fatal `invalid_prompt`, so
redaction cannot accidentally turn those categories into generic native retryable errors.

### Caller validation on 2026-09-23

- The 12 bare ordered-wire cases and 18 actual OpenCode cases pass. The broader native/OpenCode
  run initially passed 1,109 of 1,111 leaves; its two failures exposed the missing fatal error code.
  All 24 failure/recovery cases subsequently passed, alongside the 13 new source-derived error
  cases and the final 88-case affected matrix. Together these provide final passing evidence for
  1,124 distinct native/OpenCode leaves; overlapping reruns are not additional cases.
- Standard checks passed 481 Core tests (one opt-in benchmark ignored), 7 protocol tests, all Go
  race packages, formatting, Clippy and vet. Final privacy changes also passed focused checks.
- Eighteen real Subscription cases passed on gpt-5.5/Astra: 12 bare five-turn JSON/SSE/WS ×
  full/reference cases, plus actual OpenCode text, read-tool and five-turn memory on both models.
  Their 78 paired business requests (60 bare, 18 OpenCode) passed ordered wire checks. Four hidden
  WS setup requests are separate from that business count. Original login contents stayed unchanged.
- Initial live fixtures shared a 12-request relay across too many bare cases; each five-turn case
  now gets its own bounded relay. The WS oracle was also corrected to validate legitimate full
  fallback separately from incremental requests, with missing-reference negative controls. Affected
  live cases were repeated successfully; these were fixture changes, not relaxed delivery bounds.

The live relay still uses Go TLS/HTTP/1.1. These application-level captures do not certify negotiated
TLS/HTTP2 fingerprints. No real API-key credential or additional third-party client was available.

### Native client captures

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

These loopback checks establish local client/gateway compatibility. The separately authorized live
checks below establish provider acceptance for their tested scenarios. Subscription rewrites scoped
identities and credentials; complete payloads/compressed bytes consequently differ. Historical runs
also allowed turn timestamp replacement; the follow-up above removes that exception.

## Real upstream continuation evidence

On 2026-09-22, explicit authorization enabled isolated tests against the Codex Subscription backend
using an existing login. The official 0.156.0 CLI ran both directly and through the gateway; Astra
used its matching Code Mode host and actual nested tool callbacks. Refresh tokens were not copied,
the original login was verified unchanged, and raw requests/responses remained memory-only.

| Live matrix | Passed cases | Assertion |
|---|---:|---|
| Native direct / gateway / captured gateway × gpt-5.5 / Astra × HTTP / WS configuration | 12 | Three user turns, exactly one real tool callback, and a remembered synthetic label/count without restating them |
| Ordinary caller × two models × SSE / WS × full / referenced history | 8 | Five turns, state changes and reasoning visibility/restoration through both continuation forms |
| Ordinary tool / schema × two models × SSE / WS | 4 | Actual tool output round trip and requested structured result |
| Tool return after closing the original WS × two models | 2 | Referenced tool result restored on a new socket, followed by a successful new user turn |

The four captured-gateway cases compared **18 paired requests** (8 HTTP, 10 WS) at native ingress
and Core egress before forwarding to the real provider. Assertions cover recursive JSON field order,
presence/null/empty values, scalar content with scoped identity/timestamp projection, and complete
header order/casing with stable values. Native-direct WS is a requested capability and can fall back;
the captured cases assert the actual HTTP/WS transport. Repeated validation runs are not extra cases.

Live testing found a concrete failure that short mock tokens missed: an upstream HTTP 200 supplied
a **780-byte** `x-codex-turn-state`, which exceeded the gateway's 512-byte logical-ID validator and
became a gateway 503. Routing state now has separate HTTP-header validation and a **64 KiB** bound;
persisted logical IDs retain their 512-byte bound. Tokens stay memory-only and consume actual
Core/Key/session state budget. Native first-token ownership, string/first-array metadata values and
present empty strings are preserved. Long-token HTTP, WS, prewarm, turn-reset and capacity regressions
now run locally, including official CLI captures with long synthetic tokens.

The relay preserves request bodies/zstd, WS frames and header bytes except destination/path/Host;
it requires the same live credential before forwarding. Its TLS connection uses Go and HTTP/1.1,
so paired live captures prove the application request checks above, not native TLS/HTTP/2 equivalence.
Direct CLI and ordinary gateway live cases use their own provider connections. Rejection, frame-byte
preservation and request-bound tests for the relay use synthetic credentials with no provider dial.

Neither these live cases nor macOS ClientHello checks certify Linux TLS, negotiated HTTP/2
SETTINGS/HPACK, packet timing or remote fingerprint classification. Live coverage uses two models
at low reasoning effort; all-model entitlement, forced live compaction and exhaustive failure paths
remain outside this run. Bare/third-party requests retain the documented caller-control policy
(including omitted bases and native typed controls) and acquire no native environment or Code Mode runtime.
They are not claimed to reproduce every default CLI request.

## Local validation

Rust workspace tests (478 Core + 7 protocol), Go tests with the race detector, formatting, clippy
and vet passed after the live-routing corrections. Affected actual-CLI loopback captures passed
again with long routing tokens and strict wire checks. The release build and installed-artifact
checks passed; binary scans found no private home/repository prefixes or deployment identifiers.
Exact upstream prompt regeneration was verified during the initial 0.156.0 alignment.

## Follow-up producer and boundary repairs

The follow-up at baseline `e0681ac` separates tool-output encrypted content from message content,
keeps the classifier source/parent/cache identity and its uncompressed HTTP/Lite WS transport,
and omits detached Memory's four inner thread-identity fields. Full first replay preserves valid
provider output item/call IDs even with explicit identity; known aliases and scope checks remain.
Basic Guardian fixes output strictness to false, optional invalid controls use native defaults,
reasoning content without reasoning_text is omitted, and whitespace-only instructions survive.

Unsupported application JSON controls are logged/ignored by Subscription Core; protocol Ping/Pong
and valid creates remain usable. Standalone SSE error does not start the one-second response tail,
so a delayed failed footer is delivered within the existing idle budget. Custom CA parsing accepts
OpenSSL trusted labels, trims trailing X509_AUX, handles mixed bundles and rejects invalid roots.

Final HTTP/WS peer tests cover the admitted parent chain, role controls, exact encrypted payload
preservation, replay identities, invalid type/null and opaque-value controls. Native release versus
gateway loopback captures passed ordinary/trusted-AUX/mixed CA bundles over HTTP and WS.

A subsequent official-release classifier capture completed four direct/gateway HTTP/WS cases,
each with two successful parent tool turns and two classifier requests. It uses the built-in
provider, explicit auto review, a registered synthetic app-server tool and isolated loopback peers.
The failed earlier activation used manual review mode and an unavailable execution tool.

The four gateway classifier requests now match native ingress in complete ordered protocol shape,
inner metadata insertion order, header order/casing and stable header values. Body values match
after validating consistent scoped identity translation; arbitrary business values are unchanged.
The outer client_metadata HashMap has no fixed iteration-order requirement. The capture exposed
and corrected late default-originator placement, inner JSON insertion order and an invented WS
request timestamp. Classifier HTTP remains uncompressed and reuses one connection across both
requests; WS retains the Lite header and negotiated compression. The source thread, parent turn,
provider response and guardian-v2 cache key remain linked after the successful parent exchange.

These are local macOS arm64 HTTP/1.1 and WS application/TCP captures. They do not certify a full
TLS/HTTP2/platform matrix, real scoring quality, ciphertext decryption, or an actual Memory
producer. No live provider request or deployment was used for this follow-up.
