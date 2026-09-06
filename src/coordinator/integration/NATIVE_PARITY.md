# Native wire parity and capability matrix

Run `bash scripts/test-native-parity.sh` from the repository root after installing the pinned
prerequisites described in the root README. Ordinary tests remain available without a native Codex
installation through `bash scripts/test.sh`. Neither suite requires a real provider account.

## Capture method

The native suite executes the real **codex-cli 0.153.4** app-server and verifies its version. Its
model catalog comes from a checkout at **3d2ee51ca2d5db578f328aa75e20aa22c0197c9a**. Each subprocess
gets an isolated temporary config/auth directory, an empty project, an ephemeral thread, synthetic
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
| Compaction/control | Commit compaction only on completed terminal; inject, control translation and profile filtering; opaque provider references preserved by typed rules | `TestCodexProfilesCommitCompactionOnlyAfterCompletedTerminal`, `TestCodexProfilesTranslateTypedWebSocketControlFrames`, `TestResponsesProfileWebSocketInjectUsesProfileFiltering` |
| Delivery/failure | SSE/JSON, incomplete/failed terminals, response/item reconciliation, non-2xx privacy, mapping/state outage, downstream cancellation, bounded overlap/queues and no ambiguous replay | `responses_profile_http_identity_test.go`, `responses_profile_websocket_terminal_test.go`, `responses_profile_*state_outage_test.go`, `responses_profile_websocket_lifecycle_test.go`; Core transport/policy/delivery suites |
| Limits/concurrency | Actual 17 MiB HTTP and WS frames under both credentials; operator admission cap; full-frame vs cached-context limits; exclusive active turn and pressure/eviction without partial history publication | `TestResponsesAcceptsActualFramesAboveFormer16MiBLimit`, `TestOperatorRequestLimitAppliesToHTTPAndWebSocketAdmission`; `subscription_state_tests`, `responses_websocket_size_tests` |
| Credentials/security | Wrong auth, scoped disable/delete/revoke, import/refresh, interrupted cleanup, upstream secret isolation, TLS listener rules, device policy and transport isolation | `e2e_test.go`, `credential_*_test.go`, `responses_profile_credential_deletion_test.go`; coordinator storage/httpapi and Core OAuth/vault/transport suites |

Large-frame tests use a bounded 45-second write/capture deadline. The former general two-second
small-frame helper could fail under race instrumentation despite accepting and delivering the frame.
The test checks admission and exact payload/meaning, not a throughput service-level objective.

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
- Native Lite uses an empty top-level `instructions` carrier; emulation may remove it. Formed input
  instructions remain exact. Ordinary settings are evaluated for each request, including increments;
  omission does not inherit a prior ordinary caller base. Native Lite setup belongs to its prefix.
- Native ephemeral threads cannot be forked/resumed from a persisted rollout in v0.153.4. The test
  asserts native fork rejection and creates no rollout fixture. Gateway branch/restart behavior is
  exercised separately by synthetic integration/Core tests. Actual persisted-native resume/fork is
  not claimed as observed.
- The recorded ClientHello comparison was executed on macOS arm64. Remote certificates, HTTP/2/3,
  production rate limits, provider-side retention/retry contracts and real account entitlements are
  outside this loopback evidence. No real-provider request or paid inference is performed.

## Pinned source anchors

Paths below are relative to the pinned Codex checkout:

- `codex-rs/core/src/client.rs`: Lite prefix UUIDv5 generation, full request construction and WS reuse.
- `codex-rs/core/src/config/mod.rs`: default pragmatic personality selection.
- `codex-rs/models-manager/src/model_info.rs`: neutral template rendering when personality is disabled.
- `codex-rs/app-server/src/request_processors/thread_processor.rs`: fork requires stored source history.
- `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`: ephemeral threads and dynamic tools.

The user-provided moving sibling checkout was older than this release when tested. Validation uses
the exact pinned checkout and leaves the sibling checkout unchanged.
