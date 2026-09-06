#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" != "--allow-real-subscription" ]]; then
  echo "Live validation sends real Subscription requests. Pass --allow-real-subscription explicitly." >&2
  exit 2
fi
shift
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# Separate from every default/loopback suite. Import uses access credentials without refresh;
# the original login remains unchanged. API-key real requests are outside this fixture.
env -u MINI_SUB2API_CORE_CODEX_BINARY CARGO_NET_OFFLINE=true RUST_LOG=off \
  MINI_SUB2API_LIVE_SUBSCRIPTION=1 \
  mise exec -- go test -tags=nativeparity,scaffoldparity,liveparity -count=1 -timeout=20m \
  ./src/coordinator/integration -run '^TestLiveSubscription' -v "$@"
