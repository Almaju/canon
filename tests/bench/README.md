# tests/bench — codegen benchmark suite

A smoke-grade wall-clock benchmark over `codegen::generate()`, the hot
path in `src/codegen/wasm/mod.rs`. It exists to surface a performance
regression in codegen as a wall-clock number before it ships — the
correctness fixture suites (`tests/checker/`, `tests/runtime/`,
`tests/canon/`) assert *what* is emitted, never *how fast*.

## Running

```sh
just bench
# or, directly:
cargo test --release --test bench -- --ignored --nocapture
```

The benchmark is a single `#[ignore]`d test (`tests/bench.rs`), so a
plain `cargo test` compiles it but never runs it — it does **not** gate
the suite. `--nocapture` is required to see the report; `--release` is
recommended so the numbers reflect an optimized build.

## What it measures

For every `examples/<name>/src/main.can` it:

1. Loads and type-checks the program **once** (not timed). Examples that
   fail to load or check are skipped with a reason, mirroring
   `just examples`.
2. Times `codegen::generate(&module)` over N samples (after warmup) and
   reports **min / median / mean** plus the emitted component size.

Only `generate()` is timed — the load + check pipeline is excluded so
the sample isolates the codegen hot path.

## Tuning

| Env var              | Meaning                   | Default |
|----------------------|---------------------------|---------|
| `CANON_BENCH_ITERS`  | timed samples per example | 50      |
| `CANON_BENCH_WARMUP` | untimed warmup runs       | 5       |

## Why no `criterion`

`criterion` is the standard Rust benchmarking crate but sits outside this
project's dependency orbit (the Bytecode-Alliance wasm toolchain plus the
embedded runtime — see `CLAUDE.md` § Code style). A plain
`Instant::now()` loop is enough to catch a regression without the
dependency cost.

## Layout

| File          | Role                                                     |
|---------------|----------------------------------------------------------|
| `../bench.rs` | cargo test binary — the `#[ignore]`d driver              |
| `harness.rs`  | discovery, timing, and report formatting                 |
