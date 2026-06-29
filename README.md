# sz_simple_redoer (Rust)

A multithreaded Senzing redo processor written in Rust. This is the Rust sibling
of the Python [`sz_simple_redoer-v4`](../sz_simple_redoer-v4): same behavior,
same environment-variable interface, but compiled to a single native binary so
the container can be **distroless** (no Python interpreter, no pip, no apt, no
shell) and so glue-layer mistakes surface at compile time.

## Overview

A simple, scalable parallel redo processor built on the
[sz-rust-sdk](https://github.com/brianmacy/sz-rust-sdk). It is meant as a
starting point for developers writing their own redo processors.

Concurrency follows the sz-rust-sdk mandate of **OS thread pools, not async**:

- One **producer** thread polls `get_redo_record()`.
- Records flow through an `mpsc::sync_channel` whose capacity equals the worker
  count. A full channel blocks the producer, providing prefetch-style
  backpressure (the equivalent of the Python pool-full throttle).
- N **worker** threads each hold their **own** engine handle (via
  `env.get_engine()`) and call `process_redo_record()`. Engines are thread-safe
  at the C library level, one handle per OS thread.

## API demonstrated

### Core
- `get_redo_record` — retrieve a redo record produced by the engine, if any.
- `process_redo_record` — process a JSON redo record.

### Supporting
- `SzEnvironmentCore::get_instance` — initialize the Senzing environment.
- `get_stats` — retrieve internal engine diagnostics.

For more on the Senzing SDK see <https://docs.senzing.com>.

## Configuration

Environment-variable names are identical to the Python version. Precedence is
**CLI args > environment > defaults**.

### Required
```
SENZING_ENGINE_CONFIGURATION_JSON   # JSON config telling Senzing how to connect
```
If this is unset or empty the process logs an error and exits non-zero.

### Optional
```
SENZING_LOG_LEVEL                   # notset|debug|info|warning|error|fatal|critical (default: info)
SENZING_THREADS_PER_PROCESS         # worker count; 0/unset auto-detects CPUs (default: 0 = auto)
SENZING_REDO_SLEEP_TIME_IN_SECONDS  # pause when no records available (default: 60)
LONG_RECORD                         # long-running threshold; stats emitted every LONG_RECORD/2s (default: 300)
```

### Command-line options
```
-i, --info         Produce withinfo messages (print the JSON the engine returns)
-t, --debugTrace   Enable verbose Senzing engine logging
```

`SENZING_LOG_LEVEL` sets the tracing level; an explicit `RUST_LOG` overrides it.

## Build and run locally

Requires the Senzing runtime installed at `/opt/senzing/er` (the SDK's `build.rs`
links `dylib=Sz`, searching `/opt/senzing/er/lib`; override with
`SENZING_LIB_PATH`).

```bash
export SENZING_LIB_PATH=/opt/senzing/er/lib
export LD_LIBRARY_PATH=/opt/senzing/er/lib
export SENZING_ENGINE_CONFIGURATION_JSON='{
  "PIPELINE": {
    "CONFIGPATH":  "/etc/opt/senzing",
    "RESOURCEPATH":"/opt/senzing/er/resources",
    "SUPPORTPATH": "/opt/senzing/data"
  },
  "SQL": { "CONNECTION": "sqlite3://na:na@/tmp/sz_redoer.db" }
}'

cargo build --release
./target/release/sz_simple_redoer          # basic
./target/release/sz_simple_redoer -i       # print withinfo output
./target/release/sz_simple_redoer -i -t    # withinfo + debug trace
```

## Docker (distroless)

The runtime image is `gcr.io/distroless/cc-debian13:nonroot` (matched to the
glibc of `senzing/senzingsdk-runtime:4.3.2`, which is Debian 13 / glibc 2.41 —
cc-debian12 fails with a `GLIBC_2.38 not found` error; see `DOCKER_NOTES.md`).
Backends are selected at build time. At least one backend is required; the build
errors out if both are disabled.

```bash
# Both backends (default)
docker build -t brian/sz_simple_redoer_rust .

# PostgreSQL only
docker build --build-arg WITH_MSSQL=0 -t brian/sz_simple_redoer_rust:pg .

# SQL Server only
docker build --build-arg WITH_POSTGRES=0 -t brian/sz_simple_redoer_rust:mssql .

docker run --rm -e SENZING_ENGINE_CONFIGURATION_JSON brian/sz_simple_redoer_rust
```

### What the runtime image ships

The `libSz.so` payload (~430 MB) must ship regardless of language — distroless
does not shrink it. What distroless removes is the Python interpreter, pip, apt,
and the shell, reducing attack surface.

The Dockerfile copies a **measured** manifest of native libraries from the
builder stage (which itself pulls `/opt/senzing` from
`senzing/senzingsdk-runtime:4.4.0`):

- **Common:** the entire `/opt/senzing/er/lib` tree (libSz + ~50 `libg2*`
  resolution plugins + the `libg2*ECreator` feature-expression libs + db
  plugins + szvec/szzstd), `/opt/senzing/er/resources`, `szBuildVersion.json`,
  `/opt/senzing/data` (SUPPORTPATH transliteration/data models) and
  `/etc/opt/senzing` (CONFIGPATH templates).
- **PostgreSQL:** `libpostgresqlplugin.so` plus the measured `libpq` dependency
  closure (krb5/GSSAPI/LDAP/SASL/z/zstd), staged into `/opt/senzing/er/lib`.
- **SQL Server:** `libmssqlplugin.so`, the unixODBC libraries, the dlopen'd
  Microsoft ODBC driver, and the `/etc/odbc*.ini` registration files.

### Verification notes (distroless contents and ECreator libs)

See `DOCKER_NOTES.md` for the build-time verification of:

- whether `gcr.io/distroless/cc-debian12` already provides `libstdc++.so.6` and
  `libgcc_s.so.1` (needed by the MSSQL ODBC driver), and
- whether the Senzing feature-expression / ECreator libs (`libg2*ECreator.so`,
  from `senzingsdk-setup`) are required at redo runtime.

## Tests

There are **no mock tests** (per the sz-rust-sdk rules). The integration tests
in `tests/integration_test.rs` use the **real** SDK and are gated on
`SENZING_ENGINE_CONFIGURATION_JSON`; when it is unset they print a skip notice
rather than passing silently.

```bash
export SENZING_LIB_PATH=/opt/senzing/er/lib
export SENZING_ENGINE_CONFIGURATION_JSON='...'   # as above
cargo test --bins                 # unit tests (logging_id / flag mapping; no SDK needed)
cargo test --test integration_test -- --nocapture   # real-SDK integration tests
```

## Notes
- Exits gracefully on Ctrl+C or SIGTERM after in-flight records finish.
- `SzBadInputError` / `SzRetryTimeoutExceededError` are logged and the record is
  dropped — redo records are engine-internal, so there is no queue to reject to.
- The `sz-rust-sdk` dependency is an unofficial, git-only crate pinned to a
  specific commit (`Cargo.lock` is committed) and treated as a pinned
  supply-chain dependency.
