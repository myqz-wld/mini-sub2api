#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if find src/core -name Cargo.toml -print -quit 2>/dev/null | grep -q .; then
  mise exec -- cargo fmt --all -- --check
  NO_PROXY="127.0.0.1,::1" no_proxy="127.0.0.1,::1" \
    mise exec -- cargo test --workspace
  HTTP_PROXY="http://127.0.0.1:1" HTTPS_PROXY="http://127.0.0.1:1" \
    ALL_PROXY="http://127.0.0.1:1" http_proxy="http://127.0.0.1:1" \
    https_proxy="http://127.0.0.1:1" all_proxy="http://127.0.0.1:1" \
    NO_PROXY="" no_proxy="" \
    mise exec -- cargo test -p mini-sub2api-core-codex \
      oauth_login::tests::device_flow_uses_loopback_mock_and_persists_tokens -- --exact
  mise exec -- cargo clippy --workspace --all-targets -- -D warnings
  mise exec -- cargo build -p mini-sub2api-core-codex
fi

if find src/coordinator -name '*.go' -print -quit 2>/dev/null | grep -q .; then
  unformatted="$(mise exec -- gofmt -l src/coordinator)"
  if [ -n "$unformatted" ]; then
    echo "Go files require gofmt:" >&2
    echo "$unformatted" >&2
    exit 1
  fi
  mise exec -- go vet ./src/coordinator/...
  NO_PROXY="127.0.0.1,::1" no_proxy="127.0.0.1,::1" \
    MINI_SUB2API_CORE_CODEX_BINARY="$repo_root/build/cargo-target/debug/mini-sub2api-core-codex" \
    mise exec -- go test -race ./src/coordinator/...
fi
