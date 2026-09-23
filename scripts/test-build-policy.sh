#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$repo_root/build"
scratch="$(mktemp -d "$repo_root/build/tls-policy.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
# Fixtures own their target and TLS environment, even inside a cross-build shell.
while IFS= read -r name; do
  case "$name" in
    CARGO_BUILD_TARGET|CARGO_NET_OFFLINE|POLICY_TEST_HOST|AWS_LC_SYS_*|AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_*|CC|TARGET_CC|CC_aarch64_unknown_linux_*) unset "$name" ;;
  esac
done < <(compgen -e)
mkdir -p "$scratch/work space/scripts" "$scratch/bin"
cp "$repo_root/scripts/cargo.sh" "$repo_root/scripts/prepare-openssl.sh" "$scratch/work space/scripts/"
export POLICY_TEST_LOG="$scratch/cargo.log"
export POLICY_NETWORK_LOG="$scratch/network.log"
export PATH="$scratch/bin:$PATH"
# macOS provides shasum; production OpenSSL preparation runs on Linux.
if ! command -v sha256sum >/dev/null; then
  cat >"$scratch/bin/sha256sum" <<'EOF'
#!/usr/bin/env bash
exec shasum -a 256 "$@"
EOF
fi
cat >"$scratch/bin/rustc" <<'EOF'
#!/usr/bin/env bash
printf 'host: %s\n' "${POLICY_TEST_HOST:-aarch64-apple-darwin}"
EOF
cat >"$scratch/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$POLICY_TEST_LOG"
printf 'zstd=%s\n' "${ZSTD_SYS_USE_PKG_CONFIG-unset}" >>"$POLICY_TEST_LOG"
printf 'aws-system=%s\n' "${AWS_LC_SYS_USE_SYSTEM-unset}" >>"$POLICY_TEST_LOG"
printf 'aws-static=%s\n' "${AWS_LC_SYS_STATIC-unset}" >>"$POLICY_TEST_LOG"
env | LC_ALL=C sort | sed -n '/^AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_/p; /^AWS_LC_SYS_NO_JITTER_ENTROPY/p' >>"$POLICY_TEST_LOG"
EOF
cat >"$scratch/bin/curl" <<'EOF'
#!/usr/bin/env bash
echo 'unexpected network access' >"$POLICY_NETWORK_LOG"
exit 99
EOF
cat >"$scratch/bin/policy-cc" <<'EOF'
#!/usr/bin/env bash
echo 'synthetic compiler 1.0'
EOF
# These failure-path tests never build; real locking/linkage is checked on Linux.
cat >"$scratch/bin/flock" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$scratch/bin/"*
cd "$scratch/work space"
ZSTD_SYS_USE_PKG_CONFIG=1 bash scripts/cargo.sh test --offline -- --exact
expected=$'test\n--locked\n--offline\n--\n--exact\nzstd=unset\naws-system=0\naws-static=1'
[[ "$(cat "$POLICY_TEST_LOG")" == "$expected" ]]
bash scripts/cargo.sh fmt --all -- --check
[[ "$(head -1 "$POLICY_TEST_LOG")" == fmt ]]
! grep -q -- --locked "$POLICY_TEST_LOG"

expect_failure() {
  local message="$1"
  shift
  rm -f "$POLICY_TEST_LOG"
  if "$@" >"$scratch/failure.log" 2>&1; then
    echo "Expected build-policy failure: $message" >&2
    exit 1
  fi
  grep -q "$message" "$scratch/failure.log"
  [[ ! -f "$POLICY_TEST_LOG" && ! -f "$POLICY_NETWORK_LOG" ]]
}
expect_failure 'requires a Rust target' bash scripts/cargo.sh build --target
expect_failure 'Unsupported pinned OpenSSL target' bash scripts/cargo.sh build --target riscv64gc-unknown-linux-gnu
export CC=policy-cc
expect_failure 'source is not cached' bash scripts/cargo.sh test --target aarch64-unknown-linux-gnu --offline
expect_failure 'source is not cached' env CARGO_NET_OFFLINE=true bash scripts/cargo.sh build --target=aarch64-unknown-linux-musl
expect_failure 'source is not cached' bash scripts/cargo.sh build --target=aarch64-unknown-linux-musl --frozen
printf 'corrupt archive\n' >build/native-openssl/openssl-3.6.4.tar.gz
expect_failure 'checksum mismatch' bash scripts/cargo.sh build --target=aarch64-unknown-linux-musl --offline

# Isolate wrapper routing from the expensive native build and exercise hostile inherited overrides.
cat >scripts/prepare-openssl.sh <<'EOF'
#!/usr/bin/env bash
[[ "$1" == aarch64-unknown-linux-musl ]] || exit 1
printf '/synthetic prefix\n'
EOF
OPENSSL_DIR=/wrong OPENSSL_STATIC=0 AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_LIB_DIR=/wrong \
  AWS_LC_SYS_NO_JITTER_ENTROPY=0 AWS_LC_SYS_NO_JITTER_ENTROPY_aarch64_unknown_linux_musl=0 \
  bash scripts/cargo.sh check --target aarch64-unknown-linux-musl
for line in \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_DIR=/synthetic prefix' \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_LIB_DIR=/synthetic prefix/lib' \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_INCLUDE_DIR=/synthetic prefix/include' \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_STATIC=1' \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_NO_VENDOR=1' \
  'AARCH64_UNKNOWN_LINUX_MUSL_OPENSSL_LIBS=ssl:crypto' \
  'AWS_LC_SYS_NO_JITTER_ENTROPY=1' \
  'AWS_LC_SYS_NO_JITTER_ENTROPY_aarch64_unknown_linux_musl=1'; do
  grep -Fxq "$line" "$POLICY_TEST_LOG"
done
[[ ! -f "$POLICY_NETWORK_LOG" ]]
echo 'Build policy checks passed.'
