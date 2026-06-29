# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial Rust port of `sz_simple_redoer-v4` (Python multithreaded redo processor)
- Single binary `sz_simple_redoer` with CLI via `clap` (derive + env features)
- Multithreaded redo processing using `sz-rust-sdk` (rev-pinned at `b595ca4`)
- Graceful shutdown on `SIGINT`/`SIGTERM` via `ctrlc`
- Structured logging via `tracing` + `tracing-subscriber` with `RUST_LOG` env support
- `--info` flag to request Senzing withinfo payload on redo processing
- `--threads` flag (default: number of CPU cores)
- 6 unit tests covering `logging_id` helpers and redo-flags logic
- 2 integration tests gated on `SENZING_ENGINE_CONFIGURATION_JSON` (run in CI
  against Postgres; self-skip loudly when engine config is absent locally)
- Distroless multi-stage Dockerfile with `WITH_POSTGRES` / `WITH_MSSQL` build args
- GitHub Actions CI workflow (`ci.yml`): lint, build, integration, coverage, docker
- GitHub Actions security workflow (`security.yml`): daily `cargo audit` + `cargo deny`
- `deny.toml`: license, advisory, ban, and git-source allow-list
- `.github/dependabot.yml`: weekly updates for cargo, github-actions, and docker
- `Cargo.lock` tracked (binary application — reproducible builds)
