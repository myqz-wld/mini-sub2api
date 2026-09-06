#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Requires the real Codex 0.153.4 binary and its exact read-only source checkout.
# The tests fail on missing/wrong prerequisites; they never skip or contact a provider.
# Do not accept a stale Core binary inherited from another build or worktree.
env -u MINI_SUB2API_CORE_CODEX_BINARY \
  CARGO_NET_OFFLINE=true NO_PROXY="127.0.0.1,::1" no_proxy="127.0.0.1,::1" \
  mise exec -- go test -tags=nativeparity -race -count=1 -timeout=10m \
  ./src/coordinator/integration -run '^TestNative' -v "$@"
