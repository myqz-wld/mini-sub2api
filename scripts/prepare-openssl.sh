#!/usr/bin/env bash
# Prints only the installation prefix; build diagnostics stay in build/.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
target="${1:?A Linux Rust target triple is required}"
case "$target" in
  x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl) openssl_target=linux-x86_64 ;;
  aarch64-unknown-linux-gnu|aarch64-unknown-linux-musl) openssl_target=linux-aarch64 ;;
  *) echo "Unsupported pinned OpenSSL target: $target" >&2; exit 2 ;;
esac

# Codex rust-v0.156.0 .github/scripts/install-musl-openssl.sh (not Cargo.lock).
version=3.6.4
sha256=9bffaa1ad1e07b354c21bd3324ec02fa15579f45a7d0494b3e74bc449b7333ef
target_cc="CC_${target//-/_}"
compiler="${!target_cc:-${TARGET_CC:-${CC:-}}}"
if [[ -z "$compiler" ]]; then
  host="$(rustc -vV | sed -n 's/^host: //p')"
  if [[ "$target" == "$host" ]]; then
    compiler=cc
  elif [[ "$target" == *-musl && "${target%%-*}" == "${host%%-*}" ]]; then
    compiler=musl-gcc
  else
    compiler="${target/-unknown/}-gcc"
  fi
fi
for tool in "$compiler" perl make curl sha256sum flock; do
  command -v "$tool" >/dev/null || { echo "Pinned OpenSSL requires $tool." >&2; exit 1; }
done

cache="$repo_root/build/native-openssl"
mkdir -p "$cache"
# Serialize preparation so concurrent Go/Cargo builds cannot reuse partial output.
exec 9>"$cache/.lock"
flock 9
fingerprint="$(
  { sha256sum scripts/prepare-openssl.sh; "$compiler" --version;
    printf '%s\n' "$target" "$compiler" "${CFLAGS-}" "${CPPFLAGS-}" "${AR-}" "${RANLIB-}";
  } | sha256sum | cut -d ' ' -f 1
)"
root="$cache/$target/$fingerprint"
prefix="$root/install/usr/local"
if [[ -f "$prefix/.complete" && -f "$prefix/lib/libssl.a" && -f "$prefix/lib/libcrypto.a" &&
      -f "$prefix/include/openssl/opensslv.h" ]]; then
  printf '%s\n' "$prefix"
  exit 0
fi

archive="$cache/openssl-$version.tar.gz"
if [[ ! -f "$archive" ]]; then
  case "${MINI_SUB2API_TLS_OFFLINE:-${CARGO_NET_OFFLINE:-false}}" in
    true|1)
      echo "OpenSSL source is not cached. Run scripts/prepare-openssl.sh $target online first." >&2
      exit 1 ;;
  esac
  echo "Downloading pinned OpenSSL $version." >&2
  temporary="$(mktemp "$cache/download.XXXXXX")"
  trap 'rm -f "$temporary"' EXIT
  curl --fail --location --silent --show-error --retry 3 --connect-timeout 20 \
    --max-time 600 --proto '=https' --proto-redir '=https' \
    "https://github.com/openssl/openssl/releases/download/openssl-$version/openssl-$version.tar.gz" \
    -o "$temporary"
  printf '%s  %s\n' "$sha256" "$temporary" | sha256sum --check --status || {
    echo "OpenSSL source checksum mismatch." >&2; exit 1;
  }
  mv "$temporary" "$archive"
  trap - EXIT
fi
printf '%s  %s\n' "$sha256" "$archive" | sha256sum --check --status || {
  echo "Cached OpenSSL source checksum mismatch; remove build/native-openssl/openssl-$version.tar.gz." >&2
  exit 1
}

mkdir -p "$root"
# An interrupted build has no completion marker and is reconstructed in its own cache slot.
rm -rf "$root/source" "$root/install"
mkdir -p "$root/source"
tar -xzf "$archive" --strip-components=1 -C "$root/source"
# Keep identifying compiler/build paths out of OpenSSL's embedded compiler string.
# Passing remap flags to Configure itself would copy those paths into that string.
cat >"$root/source/mini-sub2api-cc" <<'EOF'
#!/usr/bin/env bash
exec "$MINI_SUB2API_OPENSSL_CC" \
  "-ffile-prefix-map=$MINI_SUB2API_OPENSSL_ROOT=." \
  "-fdebug-prefix-map=$MINI_SUB2API_OPENSSL_ROOT=." "$@"
EOF
chmod +x "$root/source/mini-sub2api-cc"
jobs="${OPENSSL_BUILD_JOBS:-${CARGO_BUILD_JOBS:-4}}"
echo "Building static OpenSSL $version for $target." >&2
umask 077
if ! (
  cd "$root/source"
  # Match official release options; remap C paths as well as the Rust build paths.
  export MINI_SUB2API_OPENSSL_CC="$compiler" MINI_SUB2API_OPENSSL_ROOT="$repo_root"
  CC=./mini-sub2api-cc perl ./Configure "$openssl_target" \
    --prefix=/usr/local --openssldir=/usr/local/ssl --libdir=lib \
    no-shared no-module no-tests no-comp no-zlib no-zlib-dynamic \
    no-ssl3 no-md2 no-rc5 no-weak-ssl-ciphers no-camellia no-idea no-seed \
    no-engine no-async -DOPENSSL_NO_SECURE_MEMORY &&
  make -j"$jobs" build_libs &&
  make DESTDIR="$root/install" install_dev
) >"$root/build.log" 2>&1; then
  echo "Pinned OpenSSL build failed; inspect build/native-openssl/$target/$fingerprint/build.log locally." >&2
  exit 1
fi
touch "$prefix/.complete"
printf '%s\n' "$prefix"
