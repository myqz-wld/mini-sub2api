#!/usr/bin/env bash
# File-level review expiry check. Run this as the first step before the next review.
# Use with the repository CLAUDE.md review-expiry rules.
# Run from the repository root.

set -euo pipefail

baseline_for_review() {
  local review_file baseline

  review_file="$1"
  baseline="$(
    awk '
      NR == 1 && $0 == "---" { in_frontmatter = 1; next }
      in_frontmatter && $0 == "---" { exit }
      in_frontmatter && $0 ~ /^baseline_commit:[[:space:]]*/ {
        sub(/^baseline_commit:[[:space:]]*/, "", $0)
        sub(/[[:space:]]+#.*$/, "", $0)
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", $0)
        if ($0 != "" && $0 !~ /^</) print $0
        exit
      }
    ' "$review_file"
  )"

  if [ -n "$baseline" ]; then
    printf '%s\n' "$baseline"
  fi
}

echo '## 1. Full file -> REVIEW x baseline commit mapping (same file in multiple reviews = multiple rows; use the largest X as latest)'
printf '%s\t%s\t%s\n' 'file' 'review' 'baseline_commit'
while IFS= read -r f; do
  BASE="$(baseline_for_review "$f")"
  if [ -z "$BASE" ]; then
    BASE="<missing-baseline>"
  fi
  awk '/^```review-scope$/{s=1; next} /^```$/{s=0} s' "$f" \
    | while IFS= read -r p; do
        [ -n "$p" ] && printf '%s\t%s\t%s\n' "$p" "$f" "$BASE"
      done
done < <(find ref/reviews -type f -path '*/REVIEW_*.md' -print 2>/dev/null | sort)

echo
echo 'Records with <missing-baseline> cannot exempt files; classify their paths as scope_unknown.'

echo
echo '## 2. Single-file expiry check example (churn / commits / age)'
echo '## Usage: set file=... to the target file and BASE=... to the baseline commit from the mapping above'
cat <<'EOF'
file=src/main/foo.ts
BASE=<coverage baseline commit from the mapping above>

churn="$(
  git diff -w --numstat "$BASE" -- "$file" \
    | awk 'NF==3 {add+=$1; del+=$2} END {print add+del}'
)"
echo "churn=$churn"

current_loc="$(wc -l < "$file" | tr -d '[:space:]')"
threshold="$(
  awk -v loc="$current_loc" 'BEGIN {
    raw = loc * 0.30
    t = int(raw)
    if (t < raw) t += 1
    if (t > 200) t = 200
    print t
  }'
)"
echo "current_loc=$current_loc threshold=$threshold"

git log -w --format=%H "$BASE..HEAD" -- "$file" | sort -u | wc -l
git log -1 --format=%cs "$BASE"
EOF

echo
cat <<'EOF'
## 3. Expiry rules (any match expires coverage)

- Net churn >= min(200 lines, 30% of current file LOC)
- Distinct commit count >= 3
- Coverage age >= 90 days and the file changed at least once during that period
- frontmatter expired: true (manual override)

## 4. Required minimum scope for this review

unreviewed files + expired reviewed files + scope_unknown files
Reviews without parseable scope cannot exempt files.

## 5. Threshold changes (200 / 3 / 90)

Changing thresholds is a project policy change; use the repository's configured review flow.
The expiry check itself is mechanical and does not need adversarial review.
EOF

