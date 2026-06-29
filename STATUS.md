# Status

**Date**: 2026-06-29
**Branch**: main
**Commit state**: Pre-first-push. One local commit exists (`bb43343 Initial commit`)
but it has NOT been pushed to the remote origin yet — `git status` shows all files
as untracked relative to origin (the ORIG_HEAD in `.git/` is from a rebase/reset
and does not represent a pushed remote state).

## What This Is

Rust port of `sz_simple_redoer-v4` (Python), the Senzing redo-record processor.
Single binary: `sz_simple_redoer` (cargo bin in `src/main.rs`).

## What Is Implemented

- Multithreaded redo processor using the `sz-rust-sdk` (rev-pinned at `b595ca4`)
- CLI via `clap` (derive + env): `--threads`, `--info`, log-level via env
- Graceful shutdown on `SIGINT`/`SIGTERM` via `ctrlc`
- `tracing`/`tracing-subscriber` structured logging
- Unit tests (6): `logging_id_*` helpers and redo-flags logic in `src/main.rs`
- Integration tests (2): gated on `SENZING_ENGINE_CONFIGURATION_JSON`; self-skip
  loudly when the variable is absent; run for real in the CI `integration` job
  against Postgres
- Distroless multi-stage Dockerfile (postgres + mssql build args)
- GitHub Actions CI (`ci.yml`): lint, build, integration, coverage, docker jobs
- GitHub Actions security (`security.yml`): daily `cargo audit` + `cargo deny`
- `deny.toml`: license + advisory + source allow-list
- `dependabot.yml`: cargo, github-actions, docker ecosystems (weekly)

## All Checks Pass (local, 2026-06-29)

| Check | Result |
|---|---|
| `cargo fmt -- --check` | ✅ clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | ✅ 0 warnings |
| `cargo test` (unit) | ✅ 6/6 passed |
| `cargo test` (integration) | ✅ 2 skipped-loudly (no engine config, runs in CI) |
| `cargo deny check` | ✅ rc 0 (warnings: unused license allowances — cosmetic) |
| `cargo audit` | ✅ 0 vulnerabilities |

## Known Open Items

1. **GitHub Actions SHA pinning**: All `uses:` entries are tag-pinned only (e.g.
   `actions/checkout@v4`). Must be SHA-pinned with `# vX.Y.Z` tag comments.
   See NEXT_STEPS.md for the full list.
2. **Dependabot cooldown**: `.github/dependabot.yml` lacks `cooldown.default-days`
   (must be ≥ 21). Currently only has `interval: "weekly"` — no explicit cooldown.
3. **README.md**: Prettier formatting check flagged one warning (informational; no
   `.prettierrc` in repo, so not a hard gate).
