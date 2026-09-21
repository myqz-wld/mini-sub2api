# Retained validation evidence

[Architecture](ARCHITECTURE.md) · [Capture tests](../src/coordinator/integration/NATIVE_PARITY.md)

This index retains compact final validation, commit and deployment metadata. Historical drafts,
request-capture bodies, copied test code, execution scripts and session logs were removed during
2026-09-21 documentation cleanup. Current runtime contracts live in Architecture, Behavior and Protocol.

The 44 historical JSON files were moved byte-for-byte after privacy checks. Paths inside them describe the
original validation workspace; they are provenance, not a promise that old scratch files remain.
Counts overlap across suites and dates. These records do not claim a new provider run or deployment.
Detailed final plans/reviews/changelogs remain local under `ref/`, excluded from distribution.

## Stream lifetime

[Diagnosis](evidence/stream-lifetime/diagnosis.json): three actual Go/Rust loopback cases reproduced
heartbeat-only and completed-but-open responses remaining in progress until transport termination.
An HTTP duration measures relay lifetime, not model execution time; the precise production stall
cannot be inferred without event-level diagnostics. The subsequent
[repair validation](evidence/stream-lifetime/validation.json) covers event idle, bounded terminal tails,
client writes and scoped resource release. This record describes local validation before deployment.

## Plan 10

Stateful Subscription architecture and compressed history indexing.
Local final record: `ref/plans/recent-month/PLAN_10_http-full-request-continuation.md`.

## Plan 11

Pinned native wire comparison and gateway capture validation.
Local final record: `ref/reviews/recent-month/REVIEW_31_native-wire-parity.md`.

[reviewed-files.json](evidence/plan-11/reviewed-files.json)

## Plan 12

Context, instruction and identity source audit.
Local final record: `ref/reviews/recent-month/REVIEW_32_native-context-source-audit.md`.

[lead-validation.json](evidence/plan-12/lead-validation.json)

## Plan 13

Native scenario/policy audit; final decisions remain in the local records.
Local final record: `ref/reviews/recent-month/REVIEW_33_native-scenario-policies.md`.

## Plan 14

Eight native lifecycle, compaction and metadata repairs.
Local final record: `ref/reviews/recent-month/REVIEW_34_native-compatibility-repairs.md`.

[delivery.json](evidence/plan-14/delivery.json)

## Plan 15

Independent capture follow-ups and seven bounded repairs.
Local final record: `ref/reviews/recent-month/REVIEW_35_native-capture-followups.md`.

[delivery.json](evidence/plan-15/delivery.json) · [scope-validation.json](evidence/plan-15/scope-validation.json)

## Plan 16

Ordinary-caller metadata and actual client validation.
Local final record: `ref/reviews/recent-month/REVIEW_36_ordinary-caller-live-captures.md`.

[delivery.json](evidence/plan-16/delivery.json) · [scope-validation.json](evidence/plan-16/scope-validation.json)

## Plan 17

Final capture/memory validation and delivery; old capture bodies were removed.
Local final record: `ref/reviews/recent-month/REVIEW_37_final-captures-and-delivery.md`.

[delivery.json](evidence/plan-17/delivery.json) · [releases.json](evidence/plan-17/releases.json) · [validation.json](evidence/plan-17/validation.json)

## Plan 18

History-prefix compatibility across bare/OpenCode callers.
Local final record: `ref/reviews/recent-month/REVIEW_38_history-prefix-compatibility.md`.

[delivery.json](evidence/plan-18/delivery.json) · [validation.json](evidence/plan-18/validation.json)

## Routing Hint Fix

API-key routing-hint transparency.
Local final record: `ref/reviews/recent-month/REVIEW_39_api-key-routing-hint.md`.

[commit.json](evidence/routing-hint-fix/commit.json) · [validation.json](evidence/routing-hint-fix/validation.json)

## Continuation Semantics

Separate configuration comparison, association and continuation semantics.
Local final record: `ref/reviews/recent-month/REVIEW_40_continuation-semantics.md`.

[validation.json](evidence/continuation-semantics/validation.json)

## Compaction Continuation

Retain verified compaction replacement windows.
Local final record: `ref/reviews/recent-month/REVIEW_41_compaction-continuation.md`.

[commit.json](evidence/compaction-continuation/commit.json) · [validation.json](evidence/compaction-continuation/validation.json)

## Compaction Association

Associate anonymous requests using verified compaction checkpoints.
Local final record: `ref/reviews/recent-month/REVIEW_42_compaction-association.md`.

[commit.json](evidence/compaction-association/commit.json) · [validation.json](evidence/compaction-association/validation.json)

## History Association

Keep history association independent of current configuration.
Local final record: `ref/reviews/recent-month/REVIEW_43_history-association.md`.

[commit.json](evidence/history-association/commit.json) · [validation.json](evidence/history-association/validation.json)

## Reasoning Visibility

Acquire encrypted reasoning, apply caller visibility and restore verified fields.
Local final record: `ref/reviews/recent-month/REVIEW_44_reasoning-visibility.md`.

[commit.json](evidence/reasoning-visibility/commit.json) · [validation.json](evidence/reasoning-visibility/validation.json)

## Release Check

Full local/live validation and the corresponding historical release.
Local final record: `ref/reviews/recent-month/REVIEW_45_release-check.md`.

[aws-verify.json](evidence/release-check/aws-verify.json) · [delivery.json](evidence/release-check/delivery.json) · [live-summary.json](evidence/release-check/live-summary.json)

## Response Integrity

Completion/footer/stream consistency repairs.
Local final record: `ref/reviews/recent-month/REVIEW_46_response-integrity-fixes.md`.

[aws-verify.json](evidence/response-integrity/aws-verify.json) · [build-info.json](evidence/response-integrity/build-info.json) · [validation.json](evidence/response-integrity/validation.json)

## Partial Output Completion

Reject completion with unresolved output items.
Local final record: `ref/reviews/recent-month/REVIEW_47_partial-output-completion.md`.

[aws-verify.json](evidence/partial-output-completion/aws-verify.json) · [build-info.json](evidence/partial-output-completion/build-info.json) · [validation.json](evidence/partial-output-completion/validation.json)

## SSE Error Terminal

Preserve a valid failed footer after an SSE error.
Local final record: `ref/reviews/recent-month/REVIEW_48_sse-error-failed-terminal.md`.

[aws-verify.json](evidence/sse-error-terminal/aws-verify.json) · [build-info.json](evidence/sse-error-terminal/build-info.json) · [validation.json](evidence/sse-error-terminal/validation.json)

## No Default Base

Keep caller-only instruction bases and test-only native snapshots.
Local final record: `ref/changelogs/recent-month/CHANGELOG_37_caller-only-base-instructions.md`.

[commit.json](evidence/no-default-base/commit.json) · [compaction-followup.json](evidence/no-default-base/compaction-followup.json) · [validation.json](evidence/no-default-base/validation.json)

## OOM Memory Accounting

Allocation-free counting, small-host mitigation and the deployed local snapshot.
Local final record: `ref/reviews/recent-week/REVIEW_49_oom-memory-accounting.md`.

[build-info.json](evidence/oom-memory-accounting/build-info.json) · [deployment.json](evidence/oom-memory-accounting/deployment.json) · [manifest.json](evidence/oom-memory-accounting/manifest.json) · [provenance.json](evidence/oom-memory-accounting/provenance.json) · [validation.json](evidence/oom-memory-accounting/validation.json) · [verification.json](evidence/oom-memory-accounting/verification.json)
