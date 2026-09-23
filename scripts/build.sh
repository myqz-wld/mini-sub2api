#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
mkdir -p build/bin
version="0.1.0"
if ! full_commit="$(git rev-parse --verify HEAD 2>/dev/null)"; then
  full_commit="unborn"
fi

mise exec -- go run ./src/coordinator/cmd/generate-build-info \
  --repository "$repo_root" \
  --version "$version" \
  --output "$repo_root/build/bin/build-info.json"

if find src/coordinator -name '*.go' -print -quit 2>/dev/null | grep -q .; then
  mise exec -- go build -trimpath \
    -ldflags "-X main.version=$version -X main.buildCommit=$full_commit" \
    -o build/bin/mini-sub2api ./src/coordinator/cmd/mini-sub2api
fi

if [ -f src/core/codex/Cargo.toml ]; then
  # Encode each flag separately so local paths containing spaces remain one rustc argument.
  rust_flags="${CARGO_ENCODED_RUSTFLAGS-}"
  if [ "${CARGO_ENCODED_RUSTFLAGS+x}" != x ]; then
    read -r -a rust_flag_words <<< "${RUSTFLAGS-}"
    for rust_flag in ${rust_flag_words[@]+"${rust_flag_words[@]}"}; do
      rust_flags+="${rust_flags:+$'\x1f'}${rust_flag}"
    done
  fi
  for remap in "$HOME=/build-user" "${CARGO_HOME:-$HOME/.cargo}=/cargo" \
    "${RUSTUP_HOME:-$HOME/.rustup}=/rustup" "$repo_root=."; do
    rust_flags+="${rust_flags:+$'\x1f'}--remap-path-prefix=$remap"
  done
  MINI_SUB2API_BUILD_COMMIT="$full_commit" \
    CARGO_ENCODED_RUSTFLAGS="$rust_flags" \
    mise exec -- bash scripts/cargo.sh build --release -p mini-sub2api-core-codex
  cp "build/cargo-target/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}release/mini-sub2api-core-codex" build/bin/
fi

build/bin/mini-sub2api --check-installed >/dev/null
build/bin/mini-sub2api-core-codex --check-installed >/dev/null
