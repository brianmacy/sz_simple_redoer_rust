//! Simple Senzing redo processor (Rust port of sz_simple_redoer-v4).
//!
//! Mirrors the Python `sz_simple_redoer.py`: a multithreaded redo processor
//! built on the Senzing engine. Concurrency follows the sz-rust-sdk mandate of
//! OS thread pools (NOT async): one producer thread polls `get_redo_record()`
//! and feeds an `mpsc::sync_channel` whose capacity equals the worker count
//! (providing prefetch-style backpressure), and N worker threads each hold
//! their own engine handle and call `process_redo_record()`.
//!
//! Behavior parity with the Python version:
//! * `--info` -> only gates whether the response JSON is PRINTED. The engine
//!   always routes through the WithInfo helper regardless, so `--info` has no
//!   effect on engine-side processing (see `SZ_WITH_INFO_BITS`).
//! * `SzBadInputError` / `SzRetryTimeoutExceededError` -> log and drop the
//!   record (redo records are engine-internal; there is no queue to reject to).
//! * Periodic `get_stats()` every `LONG_RECORD / 2` seconds.
//! * Pause `SENZING_REDO_SLEEP_TIME_IN_SECONDS` when no records are available.
//! * Graceful shutdown on SIGINT and SIGTERM; final stats printed on exit.

use clap::Parser;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use sz_rust_sdk::prelude::*;
use tracing::{error, info, warn};

/// Bit 62, historically the Senzing v4 `SZ_WITH_INFO` position. This value is
/// INERT at the engine level in this SDK: `SzEngineCore::process_redo_record`
/// unconditionally calls `Sz_processRedoRecordWithInfo_helper` (the WithInfo
/// helper) regardless of the flags passed, and the sz-rust-sdk `SzFlags` set
/// exposes no named WITH_INFO constant. Passing this bit therefore changes
/// nothing the engine does; `--info` only controls whether the returned JSON
/// response is PRINTED (see `redo_flags` / the worker print path). The const is
/// retained to keep the `redo_flags`/test surface stable, but it carries no
/// behavioral meaning at the engine boundary.
const SZ_WITH_INFO_BITS: u64 = 1 << 62;

/// Default seconds before a record is considered long-running (Python `LONG_RECORD`).
const DEFAULT_LONG_RECORD: u64 = 300;

/// Default pause seconds when no redo records are available
/// (Python `SENZING_REDO_SLEEP_TIME_IN_SECONDS`).
const DEFAULT_SLEEP_SECS: u64 = 60;

/// Report a throughput line every N processed records (Python `STATS_INTERVAL`).
const STATS_INTERVAL: usize = 1000;

/// Instance/module name passed to the Senzing environment.
const INSTANCE_NAME: &str = "sz_simple_redoer";

/// Bounded grace window for worker shutdown (FIX 3). After shutdown is
/// requested we wait at most this long for every worker to finish its current
/// `process_redo_record` before giving up the join. Senzing engine calls are
/// uninterruptible, so a worker wedged inside one cannot be cancelled; bounding
/// the join keeps shutdown within a container's SIGTERM grace instead of
/// hanging indefinitely. The tradeoff: if a worker is still stuck past this
/// window we SKIP `destroy_global_instance()` and let the OS reclaim native
/// state on exit (a one-time leak-on-exit) rather than risk calling into native
/// state a stuck worker is still using.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

// Global counters shared across producer and worker threads.
static PROCESSED: AtomicUsize = AtomicUsize::new(0);
static ERRORS: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(true);

/// Set when a worker hits a fatal condition: either it cannot obtain an engine
/// handle (unrecoverable misconfiguration), or `process_redo_record` returns an
/// error that is NOT BadInput/RetryTimeoutExceeded (e.g. Database,
/// DatabaseConnectionLost, Unrecoverable, NotInitialized, License — FIX 1). The
/// producer/run path treats this as fatal so the process tears down and exits
/// non-zero rather than limping along (losing redo work) and reporting success.
static WORKER_FATAL: AtomicBool = AtomicBool::new(false);

/// Tracks the count of in-flight records reported as long-running already, so a
/// monotonic id can be assigned to each dispatched record for the in-flight map.
static NEXT_RECORD_ID: AtomicUsize = AtomicUsize::new(0);

/// One in-flight redo record: when the worker picked it up and its raw JSON.
type InFlight = HashMap<usize, (Instant, String)>;

/// A record handed to a worker: a monotonic id plus the raw JSON payload.
type Job = (usize, String);

/// Command-line / environment configuration.
///
/// Precedence is CLI args > environment > defaults (clap's `env` feature).
/// Environment variable names are reused verbatim from the Python version so
/// the container interface is unchanged.
#[derive(Parser, Debug)]
#[command(
    name = "sz_simple_redoer",
    about = "Simple multithreaded Senzing redo processor (Rust port)"
)]
struct Args {
    /// Produce withinfo messages (print the JSON returned by the engine).
    #[arg(short = 'i', long = "info", default_value_t = false)]
    info: bool,

    /// Output Senzing debug trace (verbose engine logging).
    #[arg(short = 't', long = "debugTrace", default_value_t = false)]
    debug_trace: bool,

    /// Worker thread count. 0 (or unset) auto-detects via available CPUs.
    #[arg(long, env = "SENZING_THREADS_PER_PROCESS", default_value_t = 0)]
    threads_per_process: usize,

    /// Seconds to pause when no redo records are available.
    #[arg(
        long,
        env = "SENZING_REDO_SLEEP_TIME_IN_SECONDS",
        default_value_t = DEFAULT_SLEEP_SECS
    )]
    redo_sleep_secs: u64,

    /// Seconds before a record is considered long-running; stats are emitted
    /// every `long_record / 2` seconds.
    #[arg(long, env = "LONG_RECORD", default_value_t = DEFAULT_LONG_RECORD)]
    long_record: u64,
}

/// The flags to pass to `process_redo_record` for this run.
///
/// NOTE: the returned flags are inert at the engine level (see
/// `SZ_WITH_INFO_BITS`); the engine always uses the WithInfo helper. We still
/// thread `info` through so the value mirrors the Python call site, but the
/// observable difference of `--info` comes solely from the print decision in
/// the worker, not from these flags.
fn redo_flags(info: bool) -> Option<SzFlags> {
    if info {
        Some(SzFlags::from_bits_retain(SZ_WITH_INFO_BITS))
    } else {
        None
    }
}

/// Build a human-readable log id for a redo record, matching the Python
/// `logging_id()`: prefer `DATA_SOURCE : RECORD_ID`, fall back to UMF_PROC
/// repair messages, then to a constant.
fn logging_id(record: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(record) else {
        return "UNKNOWN RECORD".to_string();
    };

    let dsrc = value.get("DATA_SOURCE").and_then(|v| v.as_str());
    let rec_id = value.get("RECORD_ID").and_then(|v| v.as_str());
    if let (Some(dsrc), Some(rec_id)) = (dsrc, rec_id) {
        return format!("{dsrc} : {rec_id}");
    }

    // Repair messages carry UMF_PROC.PARAMS[0].PARAM.VALUE in the Python port.
    if let Some(umf_proc) = value.get("UMF_PROC") {
        if let Some(param_value) = umf_proc
            .get("PARAMS")
            .and_then(|p| p.get(0))
            .and_then(|p| p.get("PARAM"))
            .and_then(|p| p.get("VALUE"))
            .and_then(|v| v.as_str())
        {
            return format!("{param_value} : REPAIR_ENTITY");
        }
        return "UMF_PROC : REPAIR_ENTITY".to_string();
    }

    "UNKNOWN RECORD".to_string()
}

/// Initialize tracing from `SENZING_LOG_LEVEL` (Python `log_level_map`).
///
/// Falls back to `info` for unset/unrecognized values, matching the Python
/// default. An explicit `RUST_LOG` still wins if set.
fn init_tracing() {
    let level = std::env::var("SENZING_LOG_LEVEL")
        .unwrap_or_else(|_| "info".to_string())
        .to_lowercase();

    // Map the Python log-level vocabulary onto tracing levels.
    let directive = match level.as_str() {
        "debug" | "notset" => "debug",
        "warning" | "warn" => "warn",
        "error" => "error",
        "fatal" | "critical" => "error",
        _ => "info",
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(directive));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = Args::parse();

    // Required configuration; fail loudly (nonzero exit) when missing, like the
    // Python validate_config().
    let settings = match std::env::var("SENZING_ENGINE_CONFIGURATION_JSON") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            error!(
                "Missing required environment variable SENZING_ENGINE_CONFIGURATION_JSON. \
                 This tells Senzing how to connect to your data store. \
                 See https://docs.senzing.com for configuration examples."
            );
            std::process::exit(1);
        }
    };

    // Basic JSON validation (Python parsed it with orjson before proceeding).
    if serde_json::from_str::<serde_json::Value>(&settings).is_err() {
        error!("Invalid JSON in SENZING_ENGINE_CONFIGURATION_JSON");
        std::process::exit(1);
    }

    let n_workers = if args.threads_per_process == 0 {
        num_cpus::get()
    } else {
        args.threads_per_process
    };
    info!("Thread pool configured: {n_workers} workers");

    // Graceful shutdown on SIGINT (Ctrl+C) and SIGTERM (container stop). The
    // ctrlc crate's "termination" feature makes set_handler fire for SIGTERM and
    // SIGHUP as well as SIGINT, so `docker stop` is handled gracefully.
    if let Err(e) = ctrlc::set_handler(|| {
        warn!("Graceful shutdown requested");
        RUNNING.store(false, Ordering::Relaxed);
    }) {
        warn!("Could not install signal handler: {e}");
    }

    // Initialize the Senzing environment (process-wide singleton).
    let environment = SzEnvironmentCore::get_instance(INSTANCE_NAME, &settings, args.debug_trace)?;
    info!("Senzing environment initialized");

    let (workers_clean, result) = run(environment.clone(), &args, n_workers);

    // Teardown (FIX 3): only destroy the global Senzing environment if EVERY
    // worker finished within the shutdown grace window. The sz-rust-sdk has no
    // SenzingGuard type; destroy_global_instance() is the documented teardown
    // path, but it calls Sz_destroy() into native state. If a worker is still
    // stuck inside an uninterruptible process_redo_record (workers_clean ==
    // false), tearing down here would risk a use-after-free in that still-live
    // call. In that case we deliberately SKIP teardown and let process exit
    // reclaim the resources (a one-time leak-on-exit — acceptable, and far
    // safer than calling into freed native state).
    if workers_clean {
        if let Err(e) = SzEnvironmentCore::destroy_global_instance() {
            warn!("Error during Senzing environment teardown: {e}");
        }
    } else {
        warn!(
            "Skipping Senzing environment teardown: a worker is still running an \
             uninterruptible engine call; letting process exit reclaim resources"
        );
    }

    result
}

/// Run the producer + worker pool until shutdown, then print final stats.
///
/// On a fatal error (producer engine/DB failure, or a worker that cannot obtain
/// an engine handle, or a worker that hit an unrecoverable processing error)
/// this still performs orderly teardown — signal shutdown, bounded-join the
/// workers, print final stats — and only THEN returns the error so `main()`
/// exits non-zero (FIX 1 / FIX 2a). A clean shutdown returns `Ok(())`.
///
/// Returns `(workers_clean, result)` where `workers_clean` is `true` iff every
/// worker finished within the shutdown grace window (FIX 3). When it is `false`,
/// `main()` must skip native teardown because a worker may still be inside an
/// uninterruptible engine call.
fn run(
    environment: Arc<SzEnvironmentCore>,
    args: &Args,
    n_workers: usize,
) -> (bool, anyhow::Result<()>) {
    let start_time = Instant::now();

    // Channel capacity == worker count gives prefetch-style backpressure: the
    // producer's send blocks (here: spins re-checking RUNNING) on a full
    // channel, matching the Python pool-full throttle / prefetch limit.
    let (tx, rx) = mpsc::sync_channel::<Job>(n_workers);
    let rx = Arc::new(Mutex::new(rx));

    // Shared map of in-flight records (id -> (pickup time, raw JSON)) used for
    // long-running-record monitoring (FIX 3). Workers insert on pickup and
    // remove on completion; the producer scans it periodically.
    let in_flight: Arc<Mutex<InFlight>> = Arc::new(Mutex::new(HashMap::new()));

    let info = args.info;
    let mut handles = Vec::with_capacity(n_workers);
    for thread_id in 0..n_workers {
        let rx = Arc::clone(&rx);
        let env = environment.clone();
        let in_flight = Arc::clone(&in_flight);
        let handle = thread::Builder::new()
            .name(format!("redo-worker-{thread_id}"))
            .spawn(move || match env.get_engine() {
                Ok(engine) => worker_loop(
                    thread_id, rx, engine, info, in_flight, start_time, n_workers,
                ),
                Err(e) => {
                    // A worker that cannot obtain an engine is an unrecoverable
                    // misconfiguration, not a per-record hiccup. Flag it fatal
                    // and request global shutdown so the producer stops sending
                    // and the whole process exits non-zero (FIX 2a).
                    error!("Worker {thread_id} failed to get engine: {e}");
                    ERRORS.fetch_add(1, Ordering::Relaxed);
                    WORKER_FATAL.store(true, Ordering::Relaxed);
                    RUNNING.store(false, Ordering::Relaxed);
                }
            })
            .expect("failed to spawn worker thread");
        handles.push(handle);
    }

    // Producer runs on the current thread with its own engine handle. Obtaining
    // that handle is itself a fatal startup error, so on failure we still tear
    // down the (already spawned) workers before returning the error.
    let producer_engine = match environment.get_engine() {
        Ok(engine) => engine,
        Err(e) => {
            RUNNING.store(false, Ordering::Relaxed);
            let workers_clean = join_workers_bounded(handles);
            return (workers_clean, Err(anyhow::Error::from(e)));
        }
    };

    // The producer reports the fatal engine error (if any) it stopped on.
    let producer_result = producer_loop(
        producer_engine.as_ref(),
        tx,
        args,
        Arc::clone(&in_flight),
        n_workers,
    );

    // Producer exited (shutdown requested or fatal error). Signal workers and
    // wait — within a bounded grace window (FIX 3) — for in-flight records to
    // finish. `workers_clean` is false if any worker is still stuck past grace.
    RUNNING.store(false, Ordering::Relaxed);
    let workers_clean = join_workers_bounded(handles);

    // Final statistics on exit (Python prints throughput stats + completion).
    let processed = PROCESSED.load(Ordering::Relaxed);
    let errors = ERRORS.load(Ordering::Relaxed);
    print_throughput_stats(processed, start_time, &in_flight, n_workers);
    info!("Completed processing {processed} redo records ({errors} errors)");

    if let Ok(stats) = producer_engine.get_stats() {
        info!("Final engine stats: {stats}");
    }

    // Propagate fatal failures AFTER teardown so the process exits non-zero
    // (FIX 1 / FIX 2a). A clean shutdown (SIGINT/SIGTERM) returns Ok.
    if let Err(e) = producer_result {
        return (workers_clean, Err(anyhow::Error::from(e)));
    }
    if WORKER_FATAL.load(Ordering::Relaxed) {
        return (
            workers_clean,
            Err(anyhow::anyhow!(
                "A worker thread failed fatally (could not obtain a Senzing engine handle, \
                 or hit an unrecoverable error processing a redo record)"
            )),
        );
    }

    (workers_clean, Ok(()))
}

/// Join all worker handles within a bounded grace window (FIX 3).
///
/// Polls `JoinHandle::is_finished` until either every worker has finished or
/// `SHUTDOWN_GRACE` elapses. Workers that finished are joined (so panics are
/// counted as errors). Returns `true` iff ALL workers finished within the
/// window. A `false` return means at least one worker is still stuck inside an
/// uninterruptible `process_redo_record`; the caller must NOT tear down native
/// state in that case (see `main()`), since the stuck worker may still touch it.
fn join_workers_bounded(handles: Vec<thread::JoinHandle<()>>) -> bool {
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    let mut pending = handles;

    loop {
        // Join everything that has already finished; keep the rest.
        let mut still_running = Vec::new();
        for handle in pending {
            if handle.is_finished() {
                if handle.join().is_err() {
                    error!("A worker thread panicked");
                    ERRORS.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                still_running.push(handle);
            }
        }
        pending = still_running;

        if pending.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            // Grace expired: detach the remaining (stuck) workers WITHOUT
            // joining. Dropping the handles detaches the OS threads; any thread
            // still inside an uninterruptible engine call keeps running and is
            // reaped when the process exits. This is the deliberate tradeoff —
            // bound shutdown time rather than block on join() forever.
            warn!(
                "{} worker(s) still running after {:?} grace; detaching and \
                 skipping native teardown to avoid use-after-free",
                pending.len(),
                SHUTDOWN_GRACE
            );
            drop(pending);
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Print the throughput line (Python `print_simple_stats`): messages processed,
/// rate/sec, active/max threads, and runtime.
fn print_throughput_stats(
    processed: usize,
    start_time: Instant,
    in_flight: &Arc<Mutex<InFlight>>,
    max_workers: usize,
) {
    let elapsed = start_time.elapsed().as_secs_f64();
    let rate = if elapsed > 0.0 {
        processed as f64 / elapsed
    } else {
        0.0
    };
    let active = in_flight.lock().map(|m| m.len()).unwrap_or(0);
    info!(
        "Stats: {processed} processed, {rate:.1}/sec, \
         {active}/{max_workers} threads active, runtime: {elapsed:.0}s"
    );
}

/// Producer: poll `get_redo_record()` and hand records to the worker pool.
///
/// Returns `Err` only on a fatal engine/DB failure from `get_redo_record()`
/// (FIX 1) — the caller propagates it so the process exits non-zero. A clean
/// shutdown (RUNNING flipped to false) or a disconnected channel returns
/// `Ok(())`.
fn producer_loop(
    engine: &dyn SzEngine,
    tx: mpsc::SyncSender<Job>,
    args: &Args,
    in_flight: Arc<Mutex<InFlight>>,
    max_workers: usize,
) -> SzResult<()> {
    let sleep_dur = Duration::from_secs(args.redo_sleep_secs);
    // Long-record monitor cadence mirrors the Python `LONG_RECORD / 2` check.
    let mut last_monitor = Instant::now();

    while RUNNING.load(Ordering::Relaxed) {
        let record = match engine.get_redo_record() {
            Ok(record) => record,
            Err(e) => {
                // Fatal engine/DB failure: count it and propagate so the
                // process exits non-zero after orderly teardown (FIX 1). The
                // Python original re-raises here.
                error!("Error retrieving redo record: {e}");
                ERRORS.fetch_add(1, Ordering::Relaxed);
                return Err(e);
            }
        };

        if record.trim().is_empty() {
            info!(
                "No redo records available. Pausing for {} seconds.",
                args.redo_sleep_secs
            );
            interruptible_sleep(sleep_dur);
            run_monitor(
                engine,
                &in_flight,
                args.long_record,
                max_workers,
                &mut last_monitor,
            );
            continue;
        }

        // Send is interruptible (FIX 2b): the std SyncSender has no
        // send_timeout, so spin on try_send re-checking RUNNING each step. This
        // preserves the channel's prefetch backpressure (capacity == workers)
        // while letting SIGINT/SIGTERM unblock a producer wedged on a full
        // channel (e.g. all workers died).
        let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        if send_interruptible(&tx, (id, record)).is_err() {
            // Either shutdown was requested mid-send, or the channel
            // disconnected (all workers gone). Stop producing; whether this is
            // fatal is decided by the WORKER_FATAL flag in run().
            warn!("Producer stopping: shutdown requested or worker channel disconnected");
            break;
        }

        run_monitor(
            engine,
            &in_flight,
            args.long_record,
            max_workers,
            &mut last_monitor,
        );
    }

    // Dropping tx signals workers that no more records are coming.
    drop(tx);
    Ok(())
}

/// Send a job, re-checking `RUNNING` so a shutdown signal unblocks a producer
/// wedged on a full channel (FIX 2b). Returns `Err` if shutdown was requested
/// before the send completed, or if the channel disconnected.
fn send_interruptible(tx: &mpsc::SyncSender<Job>, mut job: Job) -> Result<(), ()> {
    let step = Duration::from_millis(100);
    loop {
        if !RUNNING.load(Ordering::Relaxed) {
            return Err(());
        }
        match tx.try_send(job) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Full(returned)) => {
                // Channel saturated (prefetch limit reached): wait briefly and
                // retry, re-checking RUNNING on the next loop iteration.
                job = returned;
                thread::sleep(step);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return Err(()),
        }
    }
}

/// Long-running-record monitor (FIX 3): every `LONG_RECORD / 2` seconds print
/// engine stats, log each in-flight record running longer than `LONG_RECORD`,
/// and warn if every worker is stuck on a long record. Also emits the periodic
/// throughput line. Mirrors the Python check at lines ~219-240 / ~212-216.
fn run_monitor(
    engine: &dyn SzEngine,
    in_flight: &Arc<Mutex<InFlight>>,
    long_record: u64,
    max_workers: usize,
    last_monitor: &mut Instant,
) {
    if last_monitor.elapsed() < Duration::from_secs((long_record / 2).max(1)) {
        return;
    }
    *last_monitor = Instant::now();

    match engine.get_stats() {
        Ok(stats) => info!("Engine stats: {stats}"),
        Err(e) => warn!("Could not retrieve engine stats: {e}"),
    }

    // Scan in-flight records for long runners. The Python uses LONG_RECORD * 2
    // as the "stuck" duration threshold; mirror it exactly.
    let stuck_threshold = Duration::from_secs(long_record.saturating_mul(2));
    let long_threshold = Duration::from_secs(long_record);
    let mut num_stuck = 0usize;
    if let Ok(map) = in_flight.lock() {
        for (_id, (started, record)) in map.iter() {
            let duration = started.elapsed();
            if duration > long_threshold {
                let mins = duration.as_secs_f64() / 60.0;
                info!("Long record ({mins:.1} min): {}", logging_id(record));
            }
            if duration > stuck_threshold {
                num_stuck += 1;
            }
        }
    }
    if num_stuck >= max_workers && max_workers > 0 {
        warn!("All {max_workers} threads are stuck on long running records");
    }
}

/// Worker: receive redo records and process them with the engine.
///
/// `SzError::BadInput` and `SzError::RetryTimeoutExceeded` are logged and the
/// record is dropped (no queue to reject to); all other errors are counted.
fn worker_loop(
    thread_id: usize,
    rx: Arc<Mutex<mpsc::Receiver<Job>>>,
    engine: Box<dyn SzEngine>,
    info: bool,
    in_flight: Arc<Mutex<InFlight>>,
    start_time: Instant,
    max_workers: usize,
) {
    let flags = redo_flags(info);
    // Backoff when the channel is momentarily empty. Kept short so shutdown is
    // responsive (we re-check RUNNING each iteration) while not busy-spinning.
    let idle_backoff = Duration::from_millis(50);

    loop {
        // FIX 2: do NOT hold the receiver Mutex across a blocking recv. The
        // previous code held the lock for the entire `recv_timeout(250ms)`, so
        // only one worker could wait on the channel at a time and the other N-1
        // were parked on `rx.lock()` — effective receive concurrency of 1,
        // defeating the thread pool. Here we lock only for a non-blocking
        // `try_recv`, release immediately, and back off briefly when empty. The
        // sync_channel's prefetch backpressure (capacity == n_workers) is
        // unchanged; this only changes how workers wait to dequeue.
        let try_result = {
            let receiver = rx.lock().unwrap();
            receiver.try_recv()
        };

        let (id, record) = match try_result {
            Ok(job) => job,
            Err(mpsc::TryRecvError::Empty) => {
                if RUNNING.load(Ordering::Relaxed) {
                    // Nothing right now; sleep briefly (lock released) and retry.
                    thread::sleep(idle_backoff);
                    continue;
                }
                // Shutdown requested: drain any record that may have been queued
                // between the try_recv above and the RUNNING check, then exit.
                let receiver = rx.lock().unwrap();
                match receiver.try_recv() {
                    Ok(job) => job,
                    Err(_) => break,
                }
            }
            // Producer dropped the sender: no more records will ever arrive.
            Err(mpsc::TryRecvError::Disconnected) => break,
        };

        // Register the record as in-flight so the producer's monitor can detect
        // long-running / stuck records (FIX 3). Removed once processing ends.
        if let Ok(mut map) = in_flight.lock() {
            map.insert(id, (Instant::now(), record.clone()));
        }

        match engine.process_redo_record(&record, flags) {
            Ok(result) => {
                let count = PROCESSED.fetch_add(1, Ordering::Relaxed) + 1;
                if info && !result.is_empty() {
                    // The engine always returns the WithInfo response (the helper
                    // is called unconditionally); `--info` only decides whether we
                    // PRINT it. This is where a real deployment would push the
                    // response to a withinfo queue.
                    println!("{result}");
                }
                // Periodic throughput line every STATS_INTERVAL records,
                // matching the Python `messages % STATS_INTERVAL == 0` check.
                if count.is_multiple_of(STATS_INTERVAL) {
                    print_throughput_stats(count, start_time, &in_flight, max_workers);
                }
            }
            Err(SzError::BadInput { .. }) | Err(SzError::RetryTimeoutExceeded { .. }) => {
                // Redo records are engine-internal; there is no queue to reject
                // to, so log and drop (matches the Python behavior).
                warn!(
                    "FAILED due to bad data or timeout [worker {thread_id}]: {}",
                    logging_id(&record)
                );
            }
            Err(e) => {
                // FIX 1: any other error (Database, DatabaseConnectionLost,
                // Unrecoverable, NotInitialized, License, ...) is FATAL, not a
                // per-record hiccup. The Python original lets such an exception
                // escape to the outer handler -> exit(1). If we only logged and
                // continued, a DB drop mid-run would fail every record while the
                // process kept polling and still exited 0 — silently losing redo
                // work. Flag it fatal and request global shutdown so teardown
                // runs and `run()` returns non-zero (same mechanism as the
                // get_engine failure path).
                error!(
                    "FATAL error processing redo record [worker {thread_id}]: {e} [{}]",
                    logging_id(&record)
                );
                ERRORS.fetch_add(1, Ordering::Relaxed);
                WORKER_FATAL.store(true, Ordering::Relaxed);
                RUNNING.store(false, Ordering::Relaxed);
                // Drop this record from the in-flight map before exiting the
                // loop so the final stats don't show a phantom in-flight record.
                if let Ok(mut map) = in_flight.lock() {
                    map.remove(&id);
                }
                break;
            }
        }

        // Record is no longer in flight regardless of outcome.
        if let Ok(mut map) = in_flight.lock() {
            map.remove(&id);
        }
    }
}

/// Sleep up to `dur`, waking early (in 250ms steps) if shutdown is requested.
fn interruptible_sleep(dur: Duration) {
    let step = Duration::from_millis(250);
    let mut remaining = dur;
    while remaining > Duration::ZERO && RUNNING.load(Ordering::Relaxed) {
        let nap = remaining.min(step);
        thread::sleep(nap);
        remaining = remaining.saturating_sub(nap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_id_uses_data_source_and_record_id() {
        let rec = r#"{"DATA_SOURCE":"TEST","RECORD_ID":"42"}"#;
        assert_eq!(logging_id(rec), "TEST : 42");
    }

    #[test]
    fn logging_id_handles_umf_proc_repair() {
        let rec = r#"{"UMF_PROC":{"PARAMS":[{"PARAM":{"VALUE":"99"}}]}}"#;
        assert_eq!(logging_id(rec), "99 : REPAIR_ENTITY");
    }

    #[test]
    fn logging_id_umf_proc_without_value_falls_back() {
        let rec = r#"{"UMF_PROC":{"PARAMS":[]}}"#;
        assert_eq!(logging_id(rec), "UMF_PROC : REPAIR_ENTITY");
    }

    #[test]
    fn logging_id_unknown_on_unparseable() {
        assert_eq!(logging_id("not json"), "UNKNOWN RECORD");
    }

    #[test]
    fn logging_id_unknown_when_no_ids() {
        assert_eq!(logging_id(r#"{"FOO":"bar"}"#), "UNKNOWN RECORD");
    }

    #[test]
    fn redo_flags_set_when_info() {
        assert_eq!(
            redo_flags(true),
            Some(SzFlags::from_bits_retain(SZ_WITH_INFO_BITS))
        );
        assert_eq!(redo_flags(false), None);
    }
}
