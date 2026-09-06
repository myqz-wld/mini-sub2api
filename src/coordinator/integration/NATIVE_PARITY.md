# Native wire parity and capability matrix

Run `bash scripts/test-native-parity.sh` from the repository root after installing the pinned
prerequisites described in the root README. Ordinary tests remain available without a native Codex
installation through `bash scripts/test.sh`. Neither suite requires a real provider account.

## Capture method

The native suite executes the real **codex-cli 0.153.4** app-server and verifies its version. Its
model catalog comes from a checkout at **3d2ee51ca2d5db578f328aa75e20aa22c0197c9a**. Each subprocess
gets isolated temporary user-home and config/auth directories, a controlled project, an ephemeral thread, synthetic
OAuth tokens, loopback model/metadata endpoints, disabled unrelated services and a deny-only proxy.
On macOS the created subprocess also receives a sandbox policy denying non-loopback outbound
connections. A synthetic dynamic tool reply drives a real native tool loop without executing commands.

A wrapped TCP listener captures HTTP/1 request bytes and WS upgrade/frame bytes before the test
server decodes them. A separate parser validates client masking, fragmentation, control frames,
extended lengths and permessage-deflate context takeover. Comparing its result with the server
library detects capture/parser mistakes. Raw and decoded captures stay in bounded memory; each tap
allows at most 64 connections with 64 MiB per connection, and each application capture allows at most
128 requests and 64 MiB total encoded/decoded payload. No PCAP, request/response dump, native rollout,
production auth/config change or host-process mutation is part of the workflow. Test failures report
field names, counts or error codes, never payloads. Temporary synthetic auth is removed by test cleanup.

Each native scenario runs both directly against a responder and through the actual public Go
coordinator plus supervised Rust Core. API-key application payloads are compared byte for byte,
including compressed HTTP bytes. Subscription comparisons retain ordered content, tool definitions,
model settings and typed identity relationships; they do not strip every field named `id` recursively.
Opaque strings and objects in tool parameters, arguments and message content remain comparable.

TLS probes are separate loopback TCP endpoints that close after ClientHello, before certificate
verification and before any authorization/application data is sent. Comparisons cover cipher order,
extension membership, supported groups/versions, signature algorithms, key-share group and length,
and ALPN. Random bytes, session IDs and public key bytes are discarded. Two fresh native connections
establish stable capabilities; rustls extension permutation is handled by comparing membership.
This is **ClientHello parity**, not successful TLS or remote-provider interoperability evidence.

## Executed coverage

The following matrix combines actual native capture with the existing public integration and Core
boundary suites. Rows name reproducible tests rather than implying the entire Cartesian product.
Native test names live in `native_*_test.go` and require `-tags=nativeparity`.

| Dimension | Cases and assertions | Tests |
|---|---|---|
| Native transport/credential/format | API key/Subscription × HTTP/WS × ordinary/Lite; first request, actual dynamic tool loop, next user turn; full HTTP, WS prewarm/delta, no added business inference | `TestNativeCodexCaptureHTTPAndWS`, `TestNativeCodexThroughGateway` |
| Ordinary callers | 54 cases: 2 credentials × HTTP JSON/HTTP SSE/WS × ordinary/converted Lite/formed Lite × session+turn/session only/anonymous; explicit tool increment, returned ID/call linkage, full HTTP reconstruction, unchanged-setup WS delta, developer order/duplicates | `TestNativeOrdinaryCallerMatrix` |
| Markers and compression | 6 cases: credentials × absent/invalid/Codex Originator; SDK-style caller headers; zstd bytes survive API-key forwarding, markers cannot bypass Subscription policy | `TestNativeOrdinaryCompressedAndSpoofedMarkers` |
| Request settings | Ordinary omitted base/tools use current defaults; changed Lite setup requires full reconstruction; unchanged setup remains incremental | `TestNativeOrdinaryOmittedSetupAppliesDefaults`, `TestResponsesProfileWebSocketOrdinarySecondTurnUsesDelta` |
| Model defaults | All 11 pinned catalog models: exact base text, reasoning/text defaults and Lite shape/IDs, with matched neutral-personality settings | `TestNativeAllCatalogModelDefaults` |
| Caller personality/base | Native pragmatic templates for gpt-5.5/5.4/5.4-mini pass unchanged; whitespace, Unicode and literal placeholders remain exact | `TestNativePragmaticTemplateSurvivesGateway`, `native_lite_prefixes_follow_projected_thread_payload_and_restart` |
| Deterministic Lite IDs | Independent Go SHA-1 UUIDv5 oracle over raw tools JSON and base bytes; stable same-thread repeats, separate threads, changed/reordered tools and changed base; arbitrary IDs require native proof; legacy mappings survive | `TestNativeLitePrefixContentAndThreadDimensions`, `request_normalizer::state_tests::lite_identity_tests` |
| Captured fixture continuation | Reuse actual native Lite bytes in memory; previous-only HTTP reconstruction and WS reconnect/full prewarm install valid prefix IDs without replaying a business request | `TestNativeCapturedLiteReconstructionAfterReferenceAndReconnect` |
| Wire headers and frames | Canonical UA, Originator, version policy, header spelling/common order, session/turn/window relationships, HTTP zstd magic, fresh WS nonce, negotiation, compression and decoded message order | `TestNativeCodexThroughGateway`, `TestNativeWireParserBoundaries` |
| TLS | HTTP and WS ClientHello against native under both credentials, including signature/key-exchange capabilities and ALPN; malformed parser input | `TestNativeTLSClientHelloParity`, `TestNativeHelloParserRejectsMalformedInput` |
| Scope and identity | Header priority/WS binding conflicts, response ownership after reconnect, key/account isolation, anonymous-only longest eligible prefixes, omitted IDs and independent execution ownership | `TestCodexProfilesRestoreWebSocketResponseOwnershipAfterReconnect`; `server_context_tests`, `subscription_state_tests`, `subscription_index_tests` |
| History lifecycle | Exact referenced parent/branch/window, duplicate text appended with previous ID, completed-output publication once, 3-hour bulk-history expiry/full rebuild, remote-only live WS and turn-token retention | `server_context_tests`, `responses_websocket_context_tests`, `subscription_state_tests` |
| Compaction/control | Commit only on completed, accepted compaction output; distinguish V2 from local summaries; inject, control translation and profile filtering | `TestCodexProfilesCommitCompactionOnlyAfterCompletedTerminal`, `invalid_compaction_output_never_commits_identity_or_context_windows`, `TestCodexProfilesTranslateTypedWebSocketControlFrames` |
| Delivery/failure | SSE/JSON, incomplete/failed terminals, response/item reconciliation, non-2xx privacy, mapping/state outage, downstream cancellation, bounded overlap/queues and no ambiguous replay | `responses_profile_http_identity_test.go`, `responses_profile_websocket_terminal_test.go`, `responses_profile_*state_outage_test.go`, `responses_profile_websocket_lifecycle_test.go`; Core transport/policy/delivery suites |
| Limits/concurrency | Actual 17 MiB HTTP and WS frames under both credentials; operator admission cap; full-frame vs cached-context limits; exclusive active turn and pressure/eviction without partial history publication | `TestResponsesAcceptsActualFramesAboveFormer16MiBLimit`, `TestOperatorRequestLimitAppliesToHTTPAndWebSocketAdmission`; `subscription_state_tests`, `responses_websocket_size_tests` |
| Credentials/security | Wrong auth, scoped disable/delete/revoke, import/refresh, interrupted cleanup, upstream secret isolation, TLS listener rules, device policy and transport isolation | `e2e_test.go`, `credential_*_test.go`, `responses_profile_credential_deletion_test.go`; coordinator storage/httpapi and Core OAuth/vault/transport suites |

Large-frame tests use a bounded 45-second write/capture deadline. The former general two-second
small-frame helper could fail under race instrumentation despite accepting and delivering the frame.
The test checks admission and exact payload/meaning, not a throughput service-level objective.

## Scenario and internal lifecycle comparisons

The `native_scenario_*` files add 499 leaf cases to the original 100-case suite. Seven ordinary
worker tasks compared exact source, real native calls and scoped ordinary-client fixtures. The
table distinguishes native-generated traffic from source-backed synthetic requests. Raw payloads
remain in memory. A separate policy-provenance task reconciled earlier user decisions in
the local, Git-ignored `USER_POLICIES.md`.

| Family | Leaves | Added evidence |
|---|---:|---|
| Base/Lite | 124 | Typed/file/inline precedence, empty/blank choices, developer grouping, personality updates; independent tools/base/order UUIDv5 changes |
| Environment | 42 | Controlled AGENTS precedence and budgets; all 11 Skills routes, explicit selection/toggles, permission/mode/directory/catalog updates |
| Transport | 36 | Reasoning/encrypted history, two tool continuations, distinct routing tokens, startup adoption, actual WS reconnect and native-owned HTTP fallback |
| Identity | 104 | Actual fresh children/followup; source-backed header-only/root-fork/copied-history and two-Key controls; unavailable ephemeral fork attempts labeled |
| Compaction | 68 | Actual V2/local/V1/token-budget paths, empty tools, acceptance versus completion, window/history changes; 8 ordinary reconstruction cases |
| Failures/scheduling | 93 | 39 actual-native and 54 ordinary scenarios: failed/incomplete/disconnected responses, interruption, recovery, independent work and scope/admission conflicts |
| Trace/extra metadata | 12 | Actual public JSON-RPC trace parent and turn metadata; active/disabled OTEL, HTTP headers versus WS fields, reset/reserved keys; Unicode keys, more than 16 extras and values over 128 bytes |
| Lead comparisons | 20 | 8 strict root metadata/lifetime cases and 12 ordinary replays of actual native-resolved rich context, including JSON/SSE/WS and HTTP reconstruction |

Concurrent captures are matched using independent causal barriers and per-thread packet order.
Physical connection enumeration is not global arrival order. An initial race run exposed that
test-oracle assumption; the corrected comparator still consumes every frame, checks exact API-key
bytes, and validates typed Subscription relationships.

Tests with `KNOWN`, `OBSERVATION` or `conformance=false` record a baseline discrepancy or a deliberate
scope/policy difference. A successful runner therefore does not mean every request matches native.
The following eight gaps found at runtime baseline `1d8ac55` have been repaired. Their former
observation branches now require the corrected behavior, with API-key/direct-native controls:

| Repaired gap | Required regression behavior |
|---|---|
| Native parent carrier | Native flat x-codex-parent-thread-id, nested parent and headers share one Key-scoped alias |
| Header-only lineage | Original HTTP/WS headers retain child/window metadata; a later bound child frame can advance its own window |
| Independent root fork | Source-backed root forks retain independent session ownership and consistent provenance, including a source appearing later |
| Historical child turn | Actual HTTP followup and WS suffix both succeed; copied history keeps historical ownership/causality; unrelated owners remain rejected |
| Prewarm routing state | Completed native startup metadata supplies the first turn's token once; failed prewarm, wrong socket/thread and subsequent turns stay isolated |
| Compaction acceptance | V2 requires one encrypted compaction output; invalid, duplicated or contradictory output commits neither state layer nor a usable reference |
| W3C request headers | HTTP traceparent/tracestate survive public coordinator, internal adapter and Core forwarding for both credentials; WS frame placement stays native |
| Native custom metadata | Nonreserved native string extras survive body/header projection and reset with native turn input; canonical identity and header visibility rules remain enforced |

Core regressions additionally cover unknown history without invented ownership, item-ID omission,
durable alias reuse after reopen, failed-edit rollback, live prewarm state after history expiry,
stream-only compaction completion and output-index limits. These changes preserve the user policies.

## Independent follow-up regression coverage

Four independent source/capture rechecks and lead reproduction added 116 leaf checks in
`native_regression_*_test.go`, bringing the complete suite to **715 leaves**. The additions include
actual native requests, explicitly labeled ordinary synthetic requests and eight comparator-mutation
controls. This is not 715 native-generated scenarios or an exhaustive Cartesian product.

| Boundary | Required behavior and regression |
|---|---|
| Referenced history ownership | Full HTTP/WS and live WS references enforce the same eligible historical owners. Same-thread/ancestor references and declared fork sources remain supported; unrelated sibling history is rejected. `TestNativePreviousBranchHistory`, `TestNativeReferenceHistoryScope` |
| V2 item-event acceptance | Final-only output cannot advance a window or publish a usable reference. Actual native HTTP/WS repeated compaction exposes the stored window; ordinary WS tests reference usability. `TestNativeCompactionRequiresItemDone`, `TestNativeRejectedCompactionReference` |
| Lite namespace semantics | Merge every default namespace with loose functions/custom tools in encounter order before computing the emitted UUIDv5. Repeated/Unicode/empty/changed tools and reconnect preserve the correct setup. `TestNativeOrdinaryLiteNamespaceGrouping`, `TestNativeOrdinaryLiteToolChanges` |
| Hidden startup routing | The first business frame already carries the completed hidden setup token; tool continuations keep it. Explicit prewarm and API-key controls remain. `TestNativeOrdinaryPrewarmRouting` |
| Pending startup ownership | An intervening thread neither adopts nor consumes another thread's startup state; the matching thread can still adopt it. `TestNativePrewarmThreadIntervention` |
| Fingerprints and metadata | Additional reserved-name/lookalike/reset, header-only trace, duplicate-account/two-Key/device-off and API-key compression/marker controls pass. `TestNativeExtraMetadataNamespace`, `TestNativeHeaderOnlyTrace`, `TestNativeAccountDeviceScope`, `TestNativeAPIKeyModeMarkers` |
| Comparison sensitivity | Added semantic fields, content/order changes, collapsed IDs and unstable aliases fail the strict comparator; the unchanged control passes. `TestNativeTransportProjectionMutationControls` |

Core regressions retain lightweight historical turn facts through bulk expiry, restore known fork
provenance on previous-only continuation, prevent automatic reuse across threads, and discard failed
or disconnected hidden setup state. The compaction event proof uses constant-size counts/digests,
independent of output-body retention.

Two standard Go tests independently check shutdown completion: the supervisor joins its owned
process, and a WS session waits for both pumps and terminal usage finalization before unregistering.
The recheck's intermittent directory-cleanup failures were preserved separately from business
assertions; the first uninstrumented failures were not assigned an invented cause.

Earlier probe corrections remain in the private review evidence: temporary fixtures inside the
checkout changed native project discovery; a root-return probe did not actually restore its thread;
flat routing metadata and hidden prewarms were initially overrestricted by worker assertions. The
corrected probes retain exact owner/socket, all-frame, field and inference-count controls.

Additional measured differences include Lite prefix attribution, HTTP body routing-token replay,
local turn-start timestamps, token-budget text UUIDs versus scoped metadata UUIDs, and ordinary
same-thread/different-turn scheduling. Their policy origins and limits are explicit in the user
policy document. None is automatically justified merely by existing in the implementation.

## Intentional differences and evidence limits

- Subscription pins `Version: 0.153.4` in addition to the native UA/Originator, even when the native
  app-server omits that separate header. Its common native header order and spelling remain checked.
- API-key traffic replaces authorization and filters credential-specific account/routing and hop
  headers. Native message bytes remain transparent; a new WS handshake has its own nonce and may
  order retained headers differently. Transport framing is rebuilt by the gateway.
- Scoped session, thread, item, response and device identities differ by design. Lite prefix IDs
  must describe the **emitted** thread/tools/base, after projection. Existing stored aliases are kept
  for compatibility even if a prior version generated a non-deterministic prefix alias.
- HTTP `response.metadata` turn-state is retained by Core under the selected turn policy. Native
  HTTP in this mock scenario does not itself replay that SSE metadata token. A new turn clears it.
- The native default personality feature selects pragmatic text for some models. Gateway fallback
  uses catalog default text. The defaults test disables native personality for a like-for-like
  comparison; a separate real-native test verifies that caller pragmatic text is preserved.
- Native Lite omits top-level `instructions` and `tools` during serialization. Its builder's empty
  instructions field is not a present empty wire member. Formed input instructions remain exact.
  Ordinary settings are evaluated for each request, including increments;
  omission does not inherit a prior ordinary caller base. Native Lite setup belongs to its prefix.
- Native ephemeral threads cannot be forked/resumed from a persisted rollout in v0.153.4. The test
  asserts native fork rejection and creates no rollout fixture. Gateway branch/restart behavior is
  exercised separately by synthetic integration/Core tests. Actual persisted-native resume/fork is
  not claimed as observed.
- The recorded ClientHello comparison was executed on macOS arm64. Remote certificates, HTTP/2/3,
  production rate limits, provider-side retention/retry contracts and real account entitlements are
  outside this loopback evidence. No real-provider request or paid inference is performed.
- Active OTEL tests use a bounded discard-only loopback collector with log/metrics exporters and
  user-prompt logging disabled. Rollout-enabled inference-ID generation remains source evidence:
  that native diagnostic path writes request/output artifacts, which this suite forbids.
- Environment tests do not certify every remote executor, Skills authority, permission profile or
  product-config extension. The gateway preserves resolved client context; it does not discover
  an unknown client's local environment or run the client's tools.

## Pinned source anchors

Paths below are relative to the pinned Codex checkout:

- `codex-rs/core/src/client.rs`: Lite prefix UUIDv5 generation, full request construction and WS reuse.
- `codex-rs/core/src/config/mod.rs`: default pragmatic personality selection.
- `codex-rs/models-manager/src/model_info.rs`: neutral template rendering when personality is disabled.
- `codex-rs/app-server/src/request_processors/thread_processor.rs`: fork requires stored source history.
- `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`: ephemeral threads and dynamic tools.

The user-provided moving sibling checkout was older than this release when tested. Validation uses
the exact pinned checkout and leaves the sibling checkout unchanged.
