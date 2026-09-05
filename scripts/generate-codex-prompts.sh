#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
exec mise exec -- go run ./src/coordinator/cmd/generate-codex-prompts "$@"
