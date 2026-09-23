#!/usr/bin/env bash
# Use through mise exec; all compile commands share the release TLS policy.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
subcommand="${1:?Usage: bash scripts/cargo.sh <cargo-subcommand> [args...]}"
shift
case "$subcommand" in
  build|check|clippy|test|run|rustc) ;;
  *) exec cargo "$subcommand" "$@" ;;
esac

# A caller's pkg-config opt-in would replace the locked zstd source with a system library.
unset ZSTD_SYS_USE_PKG_CONFIG

target="${CARGO_BUILD_TARGET:-}"
offline="${CARGO_NET_OFFLINE:-false}"
expect_target=false
for arg in "$@"; do
  if [[ "$expect_target" == true ]]; then
    target="$arg"
    expect_target=false
    continue
  fi
  case "$arg" in
    --) break ;;
    --target) expect_target=true ;;
    --target=*) target="${arg#--target=}" ;;
    --offline|--frozen) offline=true ;;
  esac
done
if [[ "$expect_target" == true ]]; then
  echo "--target requires a Rust target triple." >&2
  exit 2
fi
if [[ -z "$target" ]]; then
  target="$(rustc -vV | sed -n 's/^host: //p')"
fi

# aws-lc-sys can auto-detect an installed library despite Cargo.lock.
unset AWS_LC_SYS_SYSTEM_DIR "AWS_LC_SYS_SYSTEM_DIR_${target//-/_}"
export AWS_LC_SYS_USE_SYSTEM=0 AWS_LC_SYS_STATIC=1
export "AWS_LC_SYS_USE_SYSTEM_${target//-/_}=0" "AWS_LC_SYS_STATIC_${target//-/_}=1"

if [[ "$target" == *-linux-* ]]; then
  prefix="$(MINI_SUB2API_TLS_OFFLINE="$offline" bash scripts/prepare-openssl.sh "$target")"
  target_env="$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')"
  # Override all openssl-sys discovery paths, including inherited split paths.
  export "${target_env}_OPENSSL_DIR=$prefix"
  export "${target_env}_OPENSSL_LIB_DIR=$prefix/lib"
  export "${target_env}_OPENSSL_INCLUDE_DIR=$prefix/include"
  export "${target_env}_OPENSSL_STATIC=1"
  export "${target_env}_OPENSSL_NO_VENDOR=1"
  export "${target_env}_OPENSSL_LIBS=ssl:crypto"
  if [[ "$target" == *-musl ]]; then
    # The official Codex Linux release disables this unreliable musl path.
    export AWS_LC_SYS_NO_JITTER_ENTROPY=1
    export "AWS_LC_SYS_NO_JITTER_ENTROPY_${target//-/_}=1"
  fi
fi
exec cargo "$subcommand" --locked "$@"
