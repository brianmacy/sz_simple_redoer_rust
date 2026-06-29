//! Integration tests for the Senzing redo processor.
//!
//! These tests exercise the REAL Senzing SDK (no mocks, per the sz-rust-sdk
//! project rules). They require a working Senzing runtime and a configured
//! data store, so they are gated on `SENZING_ENGINE_CONFIGURATION_JSON`:
//!
//! ```bash
//! export SENZING_LIB_PATH=/opt/senzing/er/lib
//! export SENZING_ENGINE_CONFIGURATION_JSON='{
//!   "PIPELINE": {
//!     "CONFIGPATH":  "/etc/opt/senzing",
//!     "RESOURCEPATH":"/opt/senzing/er/resources",
//!     "SUPPORTPATH": "/opt/senzing/data"
//!   },
//!   "SQL": { "CONNECTION": "sqlite3://na:na@/tmp/sz_redoer_it.db" }
//! }'
//! cargo test --test integration_test -- --nocapture
//! ```
//!
//! When the variable is not set the tests no-op with a printed skip notice so
//! the suite never silently passes against a mock.

use sz_rust_sdk::prelude::*;

/// Returns the engine settings from the environment, or `None` to skip.
fn settings_or_skip(test_name: &str) -> Option<String> {
    match std::env::var("SENZING_ENGINE_CONFIGURATION_JSON") {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => {
            eprintln!(
                "SKIP {test_name}: SENZING_ENGINE_CONFIGURATION_JSON is not set. \
                 This test requires the real Senzing SDK + a configured data store."
            );
            None
        }
    }
}

/// The engine must initialize against the real backend and report redo count.
///
/// `count_redo_records()` succeeding proves the engine opened the configured
/// database backend (a missing backend plugin or bad config fails here).
#[test]
fn engine_initializes_and_counts_redo_records() {
    let Some(settings) = settings_or_skip("engine_initializes_and_counts_redo_records") else {
        return;
    };

    let env = SzEnvironmentCore::get_instance("sz_simple_redoer_it", &settings, false)
        .expect("environment must initialize with the real SDK");
    let engine = env.get_engine().expect("engine handle must be obtainable");

    let count = engine
        .count_redo_records()
        .expect("count_redo_records must succeed against the configured backend");
    assert!(count >= 0, "redo count must be non-negative, got {count}");

    // NOTE: do NOT call destroy_global_instance() here. The Senzing engine is a
    // process-global singleton shared across every test in this binary. Tearing
    // it down mid-suite leaves the singleton's is_initialized flag true while the
    // native engine is gone, so the next test's get_instance() returns a DEAD
    // engine and its engine calls fail. Process exit reclaims the singleton.
}

/// `get_redo_record()` must return a value (possibly empty) without erroring,
/// proving the redo retrieval path works end to end.
#[test]
fn get_redo_record_returns_without_error() {
    let Some(settings) = settings_or_skip("get_redo_record_returns_without_error") else {
        return;
    };

    let env = SzEnvironmentCore::get_instance("sz_simple_redoer_it", &settings, false)
        .expect("environment must initialize with the real SDK");
    let engine = env.get_engine().expect("engine handle must be obtainable");

    // An empty repository yields an empty string; a non-empty one yields JSON.
    // Either way the call must not error.
    let _record = engine
        .get_redo_record()
        .expect("get_redo_record must succeed against the configured backend");

    // See note above: the global singleton must outlive individual tests.
}
