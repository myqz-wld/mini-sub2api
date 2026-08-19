# Reviews Index

## Scope

Routing index for final debug, code-review, performance, security, and review-driven records. Feature records go in `ref/changelogs/`.

Per-record rows live only in bucket `INDEX.md` files.

## Naming

Use `ref/reviews/<bucket>/REVIEW_X_<topic>.md`. Scan every bucket first and choose the maximum existing number plus one. Use a short stable kebab-case topic.

## File Structure

- Frontmatter with `review_id`, `reviewed_at`, `baseline_commit`, and expiry fields
- Scope and `review-scope` block
- Findings and evidence
- Fixes landed
- Residual risk and follow-ups

## Buckets

| Bucket | Date Range | Directory |
|---|---|---|
| Recent 3 days | Within the last 3 days, inclusive | `ref/reviews/recent-3-days/` |
| Recent week | Older than 3 days and within 7 days | `ref/reviews/recent-week/` |
| Recent month | Older than 7 days and within 30 days | `ref/reviews/recent-month/` |
| History | Older than 30 days or no parseable date | `ref/reviews/history/` |

## Rebucket Rules

On every new or edited review, scan all records, recompute buckets from `reviewed_at`, move stale records, and update every affected bucket index.

## Bucket Indexes

- `ref/reviews/recent-3-days/INDEX.md`
- `ref/reviews/recent-week/INDEX.md`
- `ref/reviews/recent-month/INDEX.md`
- `ref/reviews/history/INDEX.md`

