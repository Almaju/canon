//! Property-based fuzz test for the whole compiler: **accepted =
//! implemented**.
//!
//! The checker is the gate: a program it accepts must compile (the
//! Direction in CLAUDE.md, and `docs/src/reference/codegen-gaps.md`).
//! This test mutates the checked-in runtime programs — which all check
//! and run — into near-miss variants, runs each through the loader and
//! the checker, and hands every variant the checker accepts to codegen.
//! The property: nothing panics, and codegen's own validation of the
//! component it emits passes. Most variants are rejected, which is fine;
//! the ones that slip through are the ones that matter.
//!
//! Mutations stay close to valid Canon so the checker's acceptance is
//! actually exercised: a type name swapped for another from the same
//! file, a literal swapped for another, a line dropped or duplicated,
//! two lines of one body swapped.
//!
//! Like `fuzz_parser.rs` it is dependency-free (a deterministic xorshift
//! PRNG) and runs under plain `cargo test`.
//!
//! * `CANON_FUZZ_SEED=<n>`  — override the PRNG seed (decimal or `0x…`).
//! * `CANON_FUZZ_ITERS=<n>` — number of generated programs (default 300).
//!
//! A failure prints the offending program, ready to become a fixture.

use canon::{checker, codegen, loader};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The property under test. `Ok` whether the checker accepts or rejects;
/// a panic anywhere — loader, checker, codegen, or codegen's validation
/// of what it emitted — unwinds out of here.
fn compile(source: &str) {
    let Ok(loaded) = loader::load_text(Path::new("fuzz.can"), source) else {
        return;
    };
    if !checker::check_loaded(&loaded).is_empty() {
        return;
    }
    let module = checker::prune_to_reachable(&loaded.module, loaded.entry_items_start);
    let _ = codegen::generate(&module);
}

static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

fn run_case(source: &str) -> Result<(), String> {
    *LAST_PANIC.lock().unwrap() = None;
    match panic::catch_unwind(panic::AssertUnwindSafe(|| compile(source))) {
        Ok(()) => Ok(()),
        Err(_) => Err(LAST_PANIC
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "<no panic message captured>".to_string())),
    }
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| match v.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        })
        .unwrap_or(default)
}

/// The single-file runtime programs: each checks and runs, so every
/// mutation starts one step from an accepted program.
fn corpus() -> Vec<String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/runtime");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/runtime exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "can"))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect()
}

/// Maximal runs of characters satisfying `keep`, as byte ranges.
fn spans(
    source: &str,
    start: impl Fn(char) -> bool,
    keep: impl Fn(char) -> bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if start(c) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if !keep(d) {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
            out.push((i, end));
        }
    }
    out
}

fn replace_one(rng: &mut Rng, source: &str, candidates: &[(usize, usize)]) -> Option<String> {
    if candidates.len() < 2 {
        return None;
    }
    let (s, e) = candidates[rng.below(candidates.len())];
    let (rs, re) = candidates[rng.below(candidates.len())];
    let replacement = &source[rs..re];
    if replacement == &source[s..e] {
        return None;
    }
    Some(format!("{}{}{}", &source[..s], replacement, &source[e..]))
}

fn mutate(rng: &mut Rng, source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    match rng.below(5) {
        // A type name swapped for another from the same program.
        0 => replace_one(
            rng,
            source,
            &spans(
                source,
                |c| c.is_ascii_uppercase(),
                |c| c.is_ascii_alphanumeric(),
            ),
        ),
        // An integer literal swapped for another.
        1 => replace_one(
            rng,
            source,
            &spans(source, |c| c.is_ascii_digit(), |c| c.is_ascii_digit()),
        ),
        // A line dropped.
        2 if lines.len() > 2 => {
            let i = rng.below(lines.len());
            let mut out = lines.clone();
            out.remove(i);
            Some(out.join("\n") + "\n")
        }
        // A line duplicated.
        3 if !lines.is_empty() => {
            let i = rng.below(lines.len());
            let mut out = lines.clone();
            out.insert(i, lines[i]);
            Some(out.join("\n") + "\n")
        }
        // Two lines at the same indentation swapped.
        4 if lines.len() > 2 => {
            let i = rng.below(lines.len());
            let indent = |l: &str| l.len() - l.trim_start().len();
            let j = (0..lines.len())
                .filter(|&j| j != i && indent(lines[j]) == indent(lines[i]))
                .nth(rng.below(lines.len()))?;
            let mut out = lines.clone();
            out.swap(i, j);
            Some(out.join("\n") + "\n")
        }
        _ => None,
    }
}

#[test]
fn accepted_programs_compile() {
    let seed = env_u64("CANON_FUZZ_SEED", 0x5EED_CA11);
    let iters = env_u64("CANON_FUZZ_ITERS", 300);
    let corpus = corpus();
    assert!(!corpus.is_empty(), "no runtime corpus");

    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        *LAST_PANIC.lock().unwrap() = Some(info.to_string());
    }));

    let mut rng = Rng::new(seed);
    let mut failure = None;
    let mut generated = 0;
    while generated < iters {
        let base = &corpus[rng.below(corpus.len())];
        let mut program = base.clone();
        for _ in 0..=rng.below(2) {
            if let Some(next) = mutate(&mut rng, &program) {
                program = next;
            }
        }
        if program == *base {
            continue;
        }
        generated += 1;
        if let Err(message) = run_case(&program) {
            failure = Some((program, message));
            break;
        }
    }
    panic::set_hook(previous_hook);

    if let Some((program, message)) = failure {
        panic!(
            "an accepted program failed to compile (seed {seed:#x}):\n{message}\n\n--- program ---\n{program}"
        );
    }
}
