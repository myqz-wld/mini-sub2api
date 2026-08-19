# Plans Index

## Scope

Routing index for final plan documents. Keep non-final plans in `.ref/plans/`; only approved and completed plans enter `ref/plans/`.

Per-record rows live only in bucket `INDEX.md` files.

## Naming

Use `ref/plans/<bucket>/PLAN_X_<topic>.md`. Scan every bucket first and choose the maximum existing number plus one. Use a short stable kebab-case topic.

## File Structure

- Goal
- Context and constraints
- Decision ledger and selected design
- Task breakdown and validation
- `Completed At` or `completed_at`
- Final status and handoff

## Buckets

| Bucket | Date Range | Directory |
|---|---|---|
| Recent 3 days | Within the last 3 days, inclusive | `ref/plans/recent-3-days/` |
| Recent week | Older than 3 days and within 7 days | `ref/plans/recent-week/` |
| Recent month | Older than 7 days and within 30 days | `ref/plans/recent-month/` |
| History | Older than 30 days or no parseable date | `ref/plans/history/` |

## Rebucket Rules

On every new or edited plan, scan all records, recompute buckets from completion date, move stale records, and update every affected bucket index.

## Bucket Indexes

- `ref/plans/recent-3-days/INDEX.md`
- `ref/plans/recent-week/INDEX.md`
- `ref/plans/recent-month/INDEX.md`
- `ref/plans/history/INDEX.md`

