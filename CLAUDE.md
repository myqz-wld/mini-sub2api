# CLAUDE.md

> Shared repository workflow for paired AI-coding entries.
> Put runtime or tool differences in `AGENTS.md` to avoid drift.

## Repository Baseline

- OS / package manager: macOS or Linux; Go and Cargo through project-local `mise`.
- Runtime versions: Go 1.26.4 and Rust 1.96.0, pinned in `mise.toml`.
- Build output: `build/` only; Cargo is configured through `.cargo/config.toml`.
- Network constraint: implementation and automated tests must never contact real OpenAI, ChatGPT, or Codex endpoints. Use loopback mock auth and inference servers.
- Product shape: Go coordinator plus a supervised Rust Codex core; public API is limited to `POST /v1/responses`.

## Base Directory Structure

- `CLAUDE.md`: shared repository workflow and validation rules.
- `AGENTS.md`: entry-specific differences only.
- `UI_COPY_LANGUAGE.md`: SSOT for user-facing CLI copy language.
- `README.md`: setup, usage, validation, security, and project structure.
- `src/coordinator/`: Go coordinator, CLI, persistence, and public HTTP service.
- `src/core/codex/`: Rust Codex core adapter and credential vault.
- `src/protocol/v1/`: versioned coordinator/core protocol and fixtures.
- `scripts/`: project automation and copied foundation helpers.
- `build/`: all generated binaries, Cargo target files, and build metadata.
- `ref/changelogs/INDEX.md`: final changelog routing index.
- `ref/reviews/INDEX.md`: final review routing index.
- `ref/plans/INDEX.md`: final plan routing index.
- `.ref/`: ignored non-final plans, reviews, raw outputs, and scratch material.

## Required After Changes

Before starting, run `find ref/changelogs ref/plans ref/reviews -maxdepth 2 -type f -name '*.md' 2>/dev/null || true`. Before creating or moving a final typed record, read the relevant root and bucket indexes, scan all same-type buckets, and use the next numeric id.

1. Update `README.md` when user-visible behavior, structure, startup, ports, dependencies, security requirements, or validation changes.
2. For each meaningful feature, behavior, API, dependency, or setup change, create a changelog record, rebucket records, and update affected indexes. Debug, performance, security, and review-driven fixes use review records.
3. Keep non-final material in `.ref/`. Archive final plans under `ref/plans/`, update indexes, and remove or explicitly classify workspace copies at handoff.
4. Store durable investigation or architecture material under `ref/` and link it from the relevant final record.
5. Keep the advisory hook installed with `bash scripts/ref-archive-reminder-pre-commit.sh --install`.

Project-specific triggers:

- After changing `src/protocol/v1/`, validate the same fixtures from both Go and Rust before changing either consumer further.
- After changing credential, OAuth, header-redaction, listener, TLS, key-hashing, retention, or revocation logic, add or update loopback-only security tests and write a review record.
- After changing the SQLite schema, add a transactional forward migration and test upgrade from the previous schema version.
- Never add a test or script that can fall back from a loopback mock URL to a real OpenAI/Codex URL.
- Never log or snapshot downstream keys, OAuth/API-key secrets, authorization codes, refresh tokens, request bodies, or response bodies.

## UI/CLI Copy Language

Write active project documentation and maintainer/agent-facing instructions in English by default. Before adding or changing user-facing CLI copy, read `UI_COPY_LANGUAGE.md` and follow its active mode.

## Review Expiry And Minimum Re-Review Scope

The next review's minimum scope is:

```text
unreviewed files union expired reviewed files union scope_unknown files
```

A reviewed file expires since its latest usable review baseline when any condition is true:

- Net change is at least `min(200 lines, 30% of current LOC)`.
- At least 3 distinct commits touched the file.
- At least 90 days have passed and the file changed at least once.
- REVIEW frontmatter sets `expired: true`.

Before review, run `bash scripts/file-level-review-expiry.sh`.

## File-Size Guardrail (500 LOC)

Before submitting, attempt to split source files over 500 LOC. Generated code, lockfiles, snapshots, migrations, and fixtures are exempt. Extract pure functions/types/constants first, then same-directory modules behind stable APIs. Record any justified exception and its revisit trigger in the relevant final record.

## Validation Flow

```bash
mise exec -- gofmt -w src/coordinator
mise exec -- cargo fmt --all -- --check
mise exec -- go test ./src/coordinator/...
mise exec -- cargo test --workspace
bash scripts/test.sh
bash scripts/build.sh
```

All auth/inference integration tests must use loopback mock endpoints.

## Deployment / Packaging

- Plain HTTP may bind only IPv4/IPv6 loopback. Every non-loopback listener requires native TLS with both certificate and key.
- A reverse proxy is optional and may forward only to a deployment-local loopback listener.
- Package `mini-sub2api`, `mini-sub2api-core-codex`, and `build-info.json` together.

Packaging must generate and ship `build-info.json` with package name, semantic version, full and short git commit, branch when available, dirty flag, and build timestamp. Installed artifacts must expose human-readable `--version` output and a machine-checkable `--check-installed` command. The freshness check compares installed metadata with the current source checkout commit, may compare local `origin/main`, never fetches remotes, and distinguishes missing metadata from commit mismatch.

