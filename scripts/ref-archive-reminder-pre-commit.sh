#!/usr/bin/env bash
# Advisory pre-commit reminder for unarchived .ref files.
# Install from the repository root:
#   bash scripts/ref-archive-reminder-pre-commit.sh --install

set -euo pipefail

hook_block_start="# project-engineering-foundation ref archive reminder start"
hook_block_end="# project-engineering-foundation ref archive reminder end"

repo_root() {
  git rev-parse --show-toplevel 2>/dev/null || pwd
}

install_hook() {
  local root git_dir hook_dir hook_path tmp

  root="$(repo_root)"
  if ! git_dir="$(git -C "$root" rev-parse --git-dir 2>/dev/null)"; then
    echo "ref archive reminder: run --install from inside a Git worktree" >&2
    exit 1
  fi

  case "$git_dir" in
    /*) hook_dir="$git_dir/hooks" ;;
    *) hook_dir="$root/$git_dir/hooks" ;;
  esac

  hook_path="$hook_dir/pre-commit"
  mkdir -p "$hook_dir"

  if [ ! -f "$hook_path" ]; then
    printf '#!/usr/bin/env bash\n\n' > "$hook_path"
  elif ! head -n 1 "$hook_path" | grep -q '^#!'; then
    tmp="${hook_path}.tmp.$$"
    {
      printf '#!/usr/bin/env bash\n\n'
      cat "$hook_path"
    } > "$tmp"
    mv "$tmp" "$hook_path"
  fi

  tmp="${hook_path}.tmp.$$"
  awk \
    -v start="$hook_block_start" \
    -v end="$hook_block_end" '
      $0 == start { skip = 1; next }
      $0 == end { skip = 0; next }
      !skip { print }
    ' "$hook_path" > "$tmp"

  {
    cat "$tmp"
    cat <<'HOOK'

# project-engineering-foundation ref archive reminder start
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
reminder="$repo_root/scripts/ref-archive-reminder-pre-commit.sh"
if [ -x "$reminder" ]; then
  "$reminder" || true
elif [ -f "$reminder" ]; then
  bash "$reminder" || true
fi
# project-engineering-foundation ref archive reminder end
HOOK
  } > "${tmp}.new"

  mv "${tmp}.new" "$hook_path"
  rm -f "$tmp"
  chmod +x "$hook_path"
  echo "installed or refreshed ref archive reminder in $hook_path"
}

check_ref_workspace() {
  local root ref_dir count shown_count extra_count

  root="$(repo_root)"
  ref_dir="$root/.ref"
  [ -d "$ref_dir" ] || return 0

  count="$(
    find "$ref_dir" -type f \
      ! -name '.DS_Store' \
      ! -name '.gitkeep' \
      ! -name 'INDEX.md' \
      -print 2>/dev/null \
      | wc -l \
      | tr -d '[:space:]'
  )"
  [ "$count" -gt 0 ] || return 0

  echo >&2
  echo "LLM ref archive check: .ref/ contains unarchived files." >&2

  find "$ref_dir" -type f \
    ! -name '.DS_Store' \
    ! -name '.gitkeep' \
    ! -name 'INDEX.md' \
    -print 2>/dev/null \
    | sort \
    | awk 'NR <= 20' \
    | while IFS= read -r path; do
        printf '  - %s\n' "${path#"$root"/}" >&2
      done

  shown_count=20
  if [ "$count" -gt "$shown_count" ]; then
    extra_count=$((count - shown_count))
    echo "  ... and $extra_count more" >&2
  fi

  echo "Before committing, classify each listed file:" >&2
  echo "  - archive durable context under ref/ and link it from the relevant final record when applicable;" >&2
  echo "  - keep it in .ref/ only if it is intentionally non-final workspace material;" >&2
  echo "  - clean it up if it is scratch that should not persist." >&2
  echo "This hook is advisory and exits 0. LLMs must still act on this check or explicitly justify leaving files in .ref/." >&2
}

case "${1:-}" in
  --install)
    install_hook
    ;;
  "")
    check_ref_workspace
    ;;
  *)
    echo "Usage: $0 [--install]" >&2
    exit 2
    ;;
esac

