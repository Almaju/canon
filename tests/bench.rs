//! Codegen benchmark driver.
//!
//! This is a `#[ignore]`d test so a plain `cargo test` compiles it but
//! never runs it — the benchmark must not gate the suite (it's a
//! smoke-grade wall-clock check, not a correctness assertion). Run it on
//! demand with:
//!
//! ```sh
//! cargo test --test bench -- --ignored --nocapture
//! # or:
//! just bench
//! ```
//!
//! The harness itself lives in `tests/bench/harness.rs`; see that
//! module for what is measured and how to tune the iteration counts.

#[path = "bench/harness.rs"]
mod harness;

/// Times `codegen::generate()` over every `examples/*/src/main.can` and
/// prints an aligned min / median / mean report. `#[ignore]` keeps it
/// out of the default `cargo test` run.
#[test]
#[ignore = "codegen benchmark; run with `--ignored --nocapture` (or `just bench`)"]
fn codegen_benchmark() {
    let report = harness::run();
    // `--nocapture` surfaces this; without it the report is swallowed,
    // which is fine for a `cargo test` compile-only pass.
    println!("\n{report}");
}
