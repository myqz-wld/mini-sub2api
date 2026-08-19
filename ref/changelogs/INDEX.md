# Changelogs Index

## Scope

Routing index for final changelog records. Final changelogs document user-visible features, behavior changes, API changes, dependency upgrades, and project setup changes. Debug, performance, security, and review-driven fixes go in `ref/reviews/`.

Per-record rows live only in bucket `INDEX.md` files.

## Naming

Use `ref/changelogs/<bucket>/CHANGELOG_X_<topic>.md`. Scan every bucket first and choose the maximum existing number plus one. Use a short stable kebab-case topic.

## File Structure

- Frontmatter with `changelog_id` and `changed_at`
- Summary
- Changes grouped by module or layer
- Validation
- Do Not Split Protection
- Notes or related review

## Buckets

| Bucket | Date Range | Directory |
|---|---|---|
| Recent 3 days | Within the last 3 days, inclusive | `ref/changelogs/recent-3-days/` |
| Recent week | Older than 3 days and within 7 days | `ref/changelogs/recent-week/` |
| Recent month | Older than 7 days and within 30 days | `ref/changelogs/recent-month/` |
| History | Older than 30 days or no parseable date | `ref/changelogs/history/` |

## Rebucket Rules

On every new or edited changelog, scan all records, recompute buckets from `changed_at`, move stale records, and update every affected bucket index.

## Bucket Indexes

- `ref/changelogs/recent-3-days/INDEX.md`
- `ref/changelogs/recent-week/INDEX.md`
- `ref/changelogs/recent-month/INDEX.md`
- `ref/changelogs/history/INDEX.md`

