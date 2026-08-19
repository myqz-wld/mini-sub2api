---
changelog_id: 3
changed_at: 2026-08-19
---

# CHANGELOG_3_codex-request-alignment: Codex subscription request alignment

## Summary

Ordinary Responses requests routed to a Codex subscription are now fitted into the current native
Codex request envelope. Current Codex headers and encoded bodies survive the coordinator/core
boundary, while caller-provided tools remain exact and regular API-key forwarding stays transparent.

## Changes

### Request normalization (`src/core/codex/`)

- Added source-derived profiles for current Codex Lite/non-Lite models.
- Structured string input, placed instructions/tools as Codex does for the selected profile, and
  filled the native request-control, reasoning, text, cache, and client-metadata fields.
- Preserved the caller's exact tool array and never added a mini-sub2api tool.
- Added consistent UUID-shaped session/thread/turn/install/window metadata when the client omits it.
- Kept already-native and encoded request bodies unchanged, and kept regular OpenAI API-key routes
  byte-transparent.

### Header forwarding (`src/coordinator/`, `src/core/codex/`)

- Preserved `Content-Encoding`, `originator`, `session-id`, `thread-id`, and the Responses-Lite
  marker through both internal boundaries.
- Continued stripping downstream authorization, forwarding headers, and other unreviewed headers.
- Preserved a supplied Codex originator; subscription requests without one use `mini_sub2api`.

### Operator contract (`README.md`)

- Documented the transformation and exact-tool boundary.
- Explicitly configured `supports_websockets = false`; WebSocket Upgrade/fallback emulation is not
  part of v1.

## Validation

- `bash scripts/test.sh`
- 28 Rust core tests and 2 Rust protocol tests
- Cargo fmt, Clippy with warnings denied, and debug build
- Go fmt, vet, and all coordinator tests under the race detector
- Real Go coordinator to Rust core to loopback-upstream parity test; no external model request

## Do Not Split Protection

None. New normalization logic and tests are split into dedicated modules; all first-party source
files remain at or below 500 lines.

## Notes

- Related review:
  [`REVIEW_3_codex-request-parity.md`](../../reviews/recent-3-days/REVIEW_3_codex-request-parity.md).
- The conversion aligns transport and request structure. It intentionally does not fabricate the
  Codex coding prompt, project context, conversation history, or tools absent from the client.
