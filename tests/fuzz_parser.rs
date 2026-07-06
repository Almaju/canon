//! Property-based fuzz test for the lexer and parser.
//!
//! The lexer (`src/lexer/scanner.rs`) and parser (`src/parser/parser.rs`)
//! are pure functions of their input bytes: `Scanner::scan_tokens` and
//! `Parser::parse` each return a `Result`. The contract this test pins is
//! **"structured error or clean parse, never a panic"** — no malformed
//! input should ever crash the compiler front-end with an index-out-of-
//! bounds, a slice on a non-char-boundary, an arithmetic overflow, or an
//! `unwrap` on `None`.
//!
//! Rather than pull in `cargo-fuzz` / `arbitrary` (the crate's dependency
//! budget is the Bytecode Alliance wasm toolchain only — see CLAUDE.md),
//! this is a self-contained property test that runs under the ordinary
//! `cargo test` harness, so CI exercises it with no extra tooling. It uses
//! a deterministic xorshift PRNG so any failure reproduces exactly, and it
//! seeds its corpus from the checked-in `.can` programs (`examples/`,
//! `tests/checker/`, `tests/runtime/`, `tests/canon/`) before mutating and
//! splicing them into malformed variants.
//!
//! ## Reproducing / expanding a run
//!
//! * `CANON_FUZZ_SEED=<n>`  — override the PRNG seed (decimal or `0x…`).
//! * `CANON_FUZZ_ITERS=<n>` — number of generated inputs (default 25000).
//!
//! When the fuzzer finds a panic, the failure message prints the offending
//! input both as a lossy string and as hex, so it can be dropped into a
//! focused regression test.

use canon::lexer::Scanner;
use canon::parser::Parser;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The property under test: running the front-end on `source` must return
/// control here. The outcome (`Ok` tokens / `Err`, `Ok` module / `Err`) is
/// deliberately discarded — the assertion is simply "it did not panic".
fn scan_and_parse(source: &str) {
    // A lexer error means there are no tokens to parse, which mirrors the
    // real pipeline (loader / formatter / main all bail on a lex error).
    if let Ok(tokens) = Scanner::new(source).scan_tokens() {
        let _ = Parser::new(tokens).parse();
    }
}

// ---------------------------------------------------------------------------
// Panic capture
// ---------------------------------------------------------------------------

/// Records the message from the most recent caught panic. A custom panic
/// hook writes here so a discovered bug surfaces as one clean, reproducible
/// failure instead of a flood of backtraces on stderr.
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Runs one case under `catch_unwind`. Returns `Ok` if the front-end
/// returned normally, or `Err(panic_message)` if it unwound.
fn run_case(source: &str) -> Result<(), String> {
    *LAST_PANIC.lock().unwrap() = None;
    match panic::catch_unwind(panic::AssertUnwindSafe(|| scan_and_parse(source))) {
        Ok(()) => Ok(()),
        Err(_) => Err(LAST_PANIC
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "<no panic message captured>".to_string())),
    }
}

// ---------------------------------------------------------------------------
// Deterministic PRNG (xorshift64*) — no `rand` dependency
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Any non-zero state works; force the low bit so a zero seed is
        // still valid.
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n` (returns 0 when `n == 0`).
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }

    fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

// ---------------------------------------------------------------------------
// Input generation
// ---------------------------------------------------------------------------

/// Bytes and short fragments biased toward Canon's grammar, so mutated
/// inputs land near-valid often enough to reach deep parser states rather
/// than bouncing off the first token.
const INTERESTING: &[&str] = &[
    "=>", "->", "*", "+", "(", ")", "{", "}", "<", ">", "[", "]", "|", ".", ",", ":", "?", "/",
    "=", "\"", "\n", "\t", " ", "#", ";", "\\", "<div>", "</div>", "<br>", "{x}", "\"hi\"", "0",
    "1", "42", "3.14", "Foo", "Bar", "main", "Unit", "Program", "List(", "String", "Int", "Bool",
    "True", "False", "é", "→", "λ", "🦀", "\u{0}", "\u{80}", "\u{ffff}",
];

/// A raw-byte string of random length. Exercises the invalid-UTF-8 and
/// non-char-boundary paths after the lossy conversion at the call site.
fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    let len = rng.below(256);
    (0..len).map(|_| rng.byte()).collect()
}

/// A string assembled from the `INTERESTING` fragments — denser in
/// grammar-significant tokens than uniform random bytes.
fn random_from_fragments(rng: &mut Rng) -> Vec<u8> {
    let pieces = rng.below(48);
    let mut out = Vec::new();
    for _ in 0..pieces {
        out.extend_from_slice(INTERESTING[rng.below(INTERESTING.len())].as_bytes());
    }
    out
}

/// A mutated / spliced variant of one or two corpus programs.
fn mutate(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = corpus[rng.below(corpus.len())].clone();

    // Optionally splice in a slice of a second program.
    if rng.bool() && !corpus.is_empty() {
        let other = &corpus[rng.below(corpus.len())];
        if !other.is_empty() {
            let a = rng.below(other.len());
            let b = a + rng.below(other.len() - a + 1);
            let at = rng.below(buf.len() + 1);
            let slice: Vec<u8> = other[a..b].to_vec();
            buf.splice(at..at, slice);
        }
    }

    // Apply a handful of point edits.
    let edits = 1 + rng.below(16);
    for _ in 0..edits {
        if buf.is_empty() {
            buf.push(rng.byte());
            continue;
        }
        match rng.below(7) {
            // Flip a byte.
            0 => {
                let i = rng.below(buf.len());
                buf[i] ^= 1 << rng.below(8);
            }
            // Replace a byte with a fully random one.
            1 => {
                let i = rng.below(buf.len());
                buf[i] = rng.byte();
            }
            // Delete a byte.
            2 => {
                let i = rng.below(buf.len());
                buf.remove(i);
            }
            // Insert a random byte.
            3 => {
                let i = rng.below(buf.len() + 1);
                buf.insert(i, rng.byte());
            }
            // Insert an interesting fragment.
            4 => {
                let i = rng.below(buf.len() + 1);
                let frag = INTERESTING[rng.below(INTERESTING.len())].as_bytes();
                buf.splice(i..i, frag.iter().copied());
            }
            // Truncate.
            5 => {
                let i = rng.below(buf.len());
                buf.truncate(i);
            }
            // Duplicate a region (stresses depth counters / nesting).
            6 => {
                let a = rng.below(buf.len());
                let b = a + rng.below(buf.len() - a + 1);
                let dup: Vec<u8> = buf[a..b].to_vec();
                let at = rng.below(buf.len() + 1);
                buf.splice(at..at, dup);
            }
            _ => unreachable!(),
        }
    }
    buf
}

fn generate(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    match rng.below(6) {
        0 => random_bytes(rng),
        1 => random_from_fragments(rng),
        // Bias toward corpus mutation — it reaches the deepest states.
        _ => mutate(rng, corpus),
    }
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

fn collect_can(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip generated binding trees — they aren't hand-authored
            // and add nothing over the source programs.
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "bindgen" || name == "deps" || name == "build" {
                continue;
            }
            collect_can(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("can") {
            out.push(path);
        }
    }
}

fn load_corpus() -> Vec<Vec<u8>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for dir in ["examples", "tests/checker", "tests/runtime", "tests/canon"] {
        collect_can(&root.join(dir), &mut paths);
    }
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| std::fs::read(p).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn report(what: &str, input: &[u8], msg: &str) -> String {
    format!(
        "lexer/parser panicked on {what}\n  panic: {msg}\n  input ({} bytes), lossy: {:?}\n  input hex: {}",
        input.len(),
        String::from_utf8_lossy(input),
        hex(input),
    )
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

fn env_u64(key: &str, default: u64) -> u64 {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            let parsed = v
                .strip_prefix("0x")
                .or_else(|| v.strip_prefix("0X"))
                .map(|hex| u64::from_str_radix(hex, 16))
                .unwrap_or_else(|| v.parse());
            parsed.unwrap_or_else(|_| panic!("invalid {key}: {v:?}"))
        }
        Err(_) => default,
    }
}

/// Runs the whole fuzz campaign. Returns `Err(report)` on the first
/// discovered panic. Never panics itself, so the caller can restore the
/// panic hook before propagating.
fn fuzz(seed: u64, iters: usize, corpus: &[Vec<u8>]) -> Result<(), String> {
    // Baseline: every checked-in program must survive verbatim. (These
    // are already exercised elsewhere, but a regression here is the
    // cheapest possible signal.)
    for (i, bytes) in corpus.iter().enumerate() {
        if let Err(msg) = run_case(&String::from_utf8_lossy(bytes)) {
            return Err(report(&format!("corpus seed #{i}"), bytes, &msg));
        }
    }

    // Generated inputs.
    let mut rng = Rng::new(seed);
    for iter in 0..iters {
        let input = generate(&mut rng, corpus);
        if let Err(msg) = run_case(&String::from_utf8_lossy(&input)) {
            return Err(report(
                &format!("iteration {iter} (CANON_FUZZ_SEED={seed:#x})"),
                &input,
                &msg,
            ));
        }
    }
    Ok(())
}

#[test]
fn lexer_and_parser_never_panic() {
    let seed = env_u64("CANON_FUZZ_SEED", 0x9E37_79B9_7F4A_7C15);
    let iters = env_u64("CANON_FUZZ_ITERS", 25_000) as usize;

    let corpus = load_corpus();
    assert!(
        !corpus.is_empty(),
        "fuzz corpus is empty — expected seed `.can` files under examples/ and tests/"
    );

    // Install a quiet panic hook that records the message instead of
    // printing a backtrace, so a bug fails cleanly and reproducibly.
    // Restore the previous hook no matter how `fuzz` returns.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        *LAST_PANIC.lock().unwrap() = Some(info.to_string());
    }));
    let outcome = panic::catch_unwind(panic::AssertUnwindSafe(|| fuzz(seed, iters, &corpus)));
    panic::set_hook(prev);

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(report)) => panic!("{report}"),
        Err(_) => panic!("fuzz harness itself panicked (not the lexer/parser)"),
    }
}
