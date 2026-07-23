# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Bumped `sz-rust-sdk` dependency from the stale `rev = "b595ca4"` to release
  `tag = "v4.3.1"` (76 commits ahead). Pulls in the structured error API
  (`ErrorContext` / `ErrorCategory` + generated `getLastExceptionCode` code map)
  and the ownership-based environment teardown.
- Redo-error classification now uses the structured `ErrorCategory` hierarchy
  (new `is_reject_error` helper) instead of matching struct-shaped `SzError`
  variants. Retry-timeout (`SENZ0010` -> `RetryTimeoutExceeded`) is correctly
  treated as **retryable → drop-and-continue, not fatal**; the stale SDK
  bucketed it coarsely so it fell through to the fatal arm. Classification is
  category-based — no error-message string matching.
- Environment teardown switched from the removed static
  `SzEnvironmentCore::destroy_global_instance()` to the ownership-based
  `Arc<SzEnvironmentCore>::destroy(self)`, still gated on `workers_clean`.

### Added

- Initial Rust port of `sz_simple_redoer-v4` (Python multithreaded redo processor)
- Single binary `sz_simple_redoer` with CLI via `clap` (derive + env features)
- Multithreaded redo processing using `sz-rust-sdk` (release `tag = "v4.3.1"`)
- Graceful shutdown on `SIGINT`/`SIGTERM` via `ctrlc`
- Structured logging via `tracing` + `tracing-subscriber` with `RUST_LOG` env support
- `--info` flag to request Senzing withinfo payload on redo processing
- `--threads` flag (default: number of CPU cores)
- 9 unit tests covering `logging_id` helpers, redo-flags logic, and
  `is_reject_error` error classification (bad-input / retry-timeout vs fatal)
- 2 integration tests gated on `SENZING_ENGINE_CONFIGURATION_JSON` (run in CI
  against Postgres; self-skip loudly when engine config is absent locally)
- Distroless multi-stage Dockerfile with `WITH_POSTGRES` / `WITH_MSSQL` build args
- GitHub Actions CI workflow (`ci.yml`): lint, build, integration, coverage, docker
- GitHub Actions security workflow (`security.yml`): daily `cargo audit` + `cargo deny`
- `deny.toml`: license, advisory, ban, and git-source allow-list
- `.github/dependabot.yml`: weekly updates for cargo, github-actions, and docker
- `Cargo.lock` tracked (binary application — reproducible builds)
