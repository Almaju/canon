//! Codegen micro-benchmark harness.
//!
//! `src/codegen/wasm/mod.rs` is the hot path for compiling any
//! non-trivial program, yet nothing in `cargo test` would catch a
//! performance regression there — the fixture suites assert *what* the
//! codegen emits, never *how fast*. This harness closes that gap: it
//! times `codegen::generate()` over the checked-in example programs so a
//! slowdown shows up as a wall-clock number before it ships.
//!
//! ## Why a hand-rolled timer instead of `criterion`
//!
//! `criterion` is the usual Rust benchmarking crate, but it lives well
//! outside this project's dependency orbit (the Bytecode-Alliance wasm
//! toolchain plus the embedded runtime — see `CLAUDE.md` § Code style).
//! Pulling it in for a smoke-grade benchmark isn't worth the cost. A
//! plain [`Instant::now`]-based loop that reports min / median / mean
//! over a handful of iterations is enough to spot a regression, and it
//! adds no dependencies.
//!
//! ## What is timed
//!
//! Only [`codegen::generate`] — the load + check pipeline runs once, up
//! front, and is *not* included in the sample. That isolates the
//! codegen hot path the way the motivating issue asks for. The emitted
//! component is a `Vec<u8>`; its length is reported alongside the
//! timings for context (a larger module is expected to take longer).
//!
//! ## How it plugs into cargo
//!
//! The driver lives in `tests/bench.rs` as a single `#[ignore]`d test,
//! so a bare `cargo test` compiles it but never runs it — the benchmark
//! does not gate the suite. Run it on demand:
//!
//! ```sh
//! cargo test --test bench -- --ignored --nocapture
//! # or, equivalently:
//! just bench
//! ```
//!
//! Iteration counts are tunable via the environment:
//!
//! | Var                   | Meaning                     | Default |
//! |-----------------------|-----------------------------|---------|
//! | `CANON_BENCH_ITERS`   | timed samples per example   | 50      |
//! | `CANON_BENCH_WARMUP`  | untimed warmup runs         | 5       |

use canon::{checker, codegen, loader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default number of timed samples collected per example.
const DEFAULT_ITERS: usize = 50;
/// Default number of untimed warmup runs before sampling begins.
const DEFAULT_WARMUP: usize = 5;

/// The timing summary for one successfully-benchmarked example.
pub struct Timing {
    /// Fastest observed `generate()` call — the least noisy estimate of
    /// the true cost, so it's the number to watch for regressions.
    pub min: Duration,
    /// Middle sample after sorting; robust to the occasional outlier.
    pub median: Duration,
    /// Arithmetic mean across all samples.
    pub mean: Duration,
    /// Size, in bytes, of the emitted WebAssembly component. Reported
    /// for context — codegen cost scales roughly with output size.
    pub bytes: usize,
}

/// What happened when we tried to benchmark one example.
pub enum Outcome {
    /// The example compiled and was timed.
    Timed(Timing),
    /// The example was skipped, with a human-readable reason (a load
    /// error, an unimplemented codegen path, or a `generate()` panic).
    /// This mirrors `just examples`, which skips examples that don't
    /// compile yet rather than failing the whole run.
    Skipped(String),
}

/// One example's name paired with its benchmark outcome.
pub struct BenchResult {
    pub name: String,
    pub outcome: Outcome,
}

/// Reads a `usize` tuning knob from the environment, falling back to
/// `default` when unset or unparseable (a `0` is treated as "unset" so
/// `CANON_BENCH_WARMUP=0` still disables warmup — handled by the caller).
fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

/// Number of timed samples per example (`CANON_BENCH_ITERS`, min 1).
fn iters() -> usize {
    env_usize("CANON_BENCH_ITERS", DEFAULT_ITERS).max(1)
}

/// Number of untimed warmup runs (`CANON_BENCH_WARMUP`, may be 0).
fn warmup() -> usize {
    env_usize("CANON_BENCH_WARMUP", DEFAULT_WARMUP)
}

/// The repository root, derived from `CARGO_MANIFEST_DIR` so the harness
/// finds `examples/` regardless of the process's working directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Discovers the benchmarkable example entry points: every
/// `examples/<name>/src/main.can`. Each such directory is a Canon
/// package (`just examples` builds them as workspace members); the
/// package's entry file is always `src/main.can`. Returns
/// `(name, entry_path)` pairs sorted by name for stable reporting.
///
/// The `examples/` workspace root (`examples/canon.toml`, no `src/`) and
/// any loose non-package example are naturally excluded — they have no
/// `src/main.can`.
pub fn discover_examples() -> Vec<(String, PathBuf)> {
    let examples_dir = repo_root().join("examples");
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("could not read `{}`: {}", examples_dir.display(), e))
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|dir| {
            let entry = dir.join("src").join("main.can");
            if entry.is_file() {
                let name = dir.file_name()?.to_string_lossy().into_owned();
                Some((name, entry))
            } else {
                None
            }
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Benchmarks `codegen::generate()` on a single example entry file.
///
/// Loading and checking happen once and are excluded from the timing.
/// If either step fails — a load error, or any checker error — the
/// example is [`Outcome::Skipped`] with a reason, matching the
/// skip-don't-fail policy of `just examples`. `generate()` is wrapped in
/// [`catch_unwind`](std::panic::catch_unwind) so a panic on one example
/// (e.g. a `validate()` assertion tripping on an unimplemented path)
/// degrades to a skip rather than aborting the whole report.
pub fn bench_one(entry: &Path) -> Outcome {
    let loaded = match loader::load_module(entry) {
        Ok(loaded) => loaded,
        Err(err) => return Outcome::Skipped(format!("load error: {err}")),
    };

    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        return Outcome::Skipped(format!(
            "{} checker error(s) — does not compile yet",
            errors.len()
        ));
    }

    let module = loaded.module;
    let samples = iters();

    // Warmup + timing under one `catch_unwind`: a panicking `generate()`
    // becomes a skip instead of an aborted run. The closure returns the
    // per-iteration samples plus the output size from the final run.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for _ in 0..warmup() {
            let _ = codegen::generate(&module);
        }
        let mut durations: Vec<Duration> = Vec::with_capacity(samples);
        let mut bytes = 0usize;
        for _ in 0..samples {
            let start = Instant::now();
            let component = codegen::generate(&module);
            durations.push(start.elapsed());
            bytes = component.len();
        }
        (durations, bytes)
    }));

    let (mut durations, bytes) = match result {
        Ok(pair) => pair,
        Err(_) => return Outcome::Skipped("generate() panicked".to_string()),
    };

    durations.sort_unstable();
    let min = durations[0];
    let median = durations[durations.len() / 2];
    let total: Duration = durations.iter().sum();
    let mean = total / durations.len() as u32;

    Outcome::Timed(Timing {
        min,
        median,
        mean,
        bytes,
    })
}

/// Benchmarks every discovered example and returns the results in
/// discovery (alphabetical) order.
pub fn bench_all() -> Vec<BenchResult> {
    discover_examples()
        .into_iter()
        .map(|(name, entry)| BenchResult {
            name,
            outcome: bench_one(&entry),
        })
        .collect()
}

/// Formats a [`Duration`] into a compact fixed-unit string, picking µs
/// or ms so the number stays readable (e.g. `842.3µs`, `1.37ms`).
fn fmt_dur(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 1_000_000 {
        format!("{:.1}µs", ns as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    }
}

/// Renders the benchmark results as an aligned, human-readable table
/// suitable for printing under `--nocapture`. Skipped examples are
/// listed separately with their reasons so a gap in coverage is visible
/// rather than silent.
pub fn format_report(results: &[BenchResult]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "codegen::generate() benchmark  ({} samples/example, {} warmup)\n\n",
        iters(),
        warmup()
    ));

    let timed: Vec<(&String, &Timing)> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Timed(t) => Some((&r.name, t)),
            Outcome::Skipped(_) => None,
        })
        .collect();

    if timed.is_empty() {
        out.push_str("  (no examples were benchmarked)\n");
    } else {
        out.push_str(&format!(
            "  {:<18}  {:>10}  {:>10}  {:>10}  {:>10}\n",
            "example", "min", "median", "mean", "bytes"
        ));
        out.push_str(&format!("  {}\n", "-".repeat(18 + 4 * 12)));
        for (name, t) in &timed {
            out.push_str(&format!(
                "  {:<18}  {:>10}  {:>10}  {:>10}  {:>10}\n",
                name,
                fmt_dur(t.min),
                fmt_dur(t.median),
                fmt_dur(t.mean),
                t.bytes,
            ));
        }
    }

    let skipped: Vec<(&String, &String)> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Skipped(reason) => Some((&r.name, reason)),
            Outcome::Timed(_) => None,
        })
        .collect();

    if !skipped.is_empty() {
        out.push_str("\n  skipped:\n");
        for (name, reason) in &skipped {
            out.push_str(&format!("    {name:<18}  {reason}\n"));
        }
    }

    out
}

/// Runs the full benchmark and returns the formatted report string.
pub fn run() -> String {
    format_report(&bench_all())
}
