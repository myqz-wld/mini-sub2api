#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# Requires the pinned OpenCode binary prepared locally; never downloads or contacts a provider.
env -u MINI_SUB2API_CORE_CODEX_BINARY CARGO_NET_OFFLINE=true \
  mise exec -- go test -tags=nativeparity,scaffoldparity -race -count=1 -timeout=10m \
  ./src/coordinator/integration -run '^TestOpenCode' -v "$@"
