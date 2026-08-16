//! The self-hosted compiler's output is real wasm, and it runs.
//!
//! `canonc` reads *Canon* source from a file named on the command line,
//! tokenizes it, parses the declaration body as a literal followed by a
//! chain of `-> Op(literal)` steps, and emits a WebAssembly core module
//! whose exported `answer` evaluates it. One declaration form of one
//! language, but the input is Canon and the output is wasm. The emission is hex rather than bytes because Canon
//! cannot write binary yet — `write` takes a UTF-8 `string` (see the
//! codegen gaps) — and hex sidesteps that without waiting on it.
//!
//! This is the end-to-end claim in one test: Canon-authored compiler
//! output, decoded, handed to wasmtime, executed, and the result checked
//! against the input it was derived from.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Run `canonc` over `source` and return what it wrote to stdout —
/// hex for a module it accepted, a diagnostic for one it rejected.
fn canonc_stdout(name: &str, source: &str) -> String {
    // Written where the test can point at it — `canonc` reads its input
    // path from the program arguments, so this exercises the real entry
    // rather than a baked-in string.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("canonc-input");
    fs::create_dir_all(&path).expect("create tmpdir");
    path.push(name);
    fs::write(&path, source).expect("write source");

    let out = Command::new(env!("CARGO_BIN_EXE_canon"))
        .args(["run", "canonc", path.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run canonc");
    assert!(
        out.status.success(),
        "canon run canonc failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8 stdout")
        .trim()
        .to_string()
}

/// Compile `source` with `canonc`, run the wasm it emits, and return
/// what its `answer` export evaluates to.
fn canonc_answer(name: &str, source: &str) -> i32 {
    canonc_export(name, source, "answer")
}

/// Same, for a module whose exported function is named something else —
/// `canonc` lowercases the declared type, so `Unit => Total { … }`
/// exports `total`.
fn canonc_export(name: &str, source: &str, export: &str) -> i32 {
    canonc_apply(name, source, export, None)
}

/// Same again, for a declaration that takes a parameter: `arg` is
/// passed to the exported function.
fn canonc_apply(name: &str, source: &str, export: &str, arg: Option<i32>) -> i32 {
    let hex = canonc_stdout(name, source);
    let hex = hex.as_str();

    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    assert_eq!(&bytes[..4], b"\0asm", "emitted bytes are a wasm module");

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    match arg {
        None => instance
            .get_typed_func::<(), i32>(&mut store, export)
            .unwrap_or_else(|_| panic!("emitted module should export `{export}`"))
            .call(&mut store, ())
            .expect("call the export"),
        Some(a) => instance
            .get_typed_func::<i32, i32>(&mut store, export)
            .unwrap_or_else(|_| panic!("emitted module should export `{export}`"))
            .call(&mut store, a)
            .expect("call the export"),
    }
}

#[test]
fn canonc_output_is_wasm_that_runs() {
    // `canonc` read `Unit => Answer { 7 }` off disk, found the literal
    // in the declaration body, and emitted `i32.const 7`.
    assert_eq!(canonc_answer("seven.can", "Unit => Answer { 7 }\n"), 7);
}

#[test]
fn canonc_tokenizes_rather_than_scanning_for_digits() {
    // A digit inside an identifier is part of that word, not a literal.
    // Reading the source byte by byte would fold the `7` of `Answer7`
    // into the answer and return 73; the scanner runs each word to its
    // end and classifies it by its first byte, so only `3` is a number.
    // The export follows the declaration too, so this one is `answer7`.
    assert_eq!(
        canonc_export("word.can", "Unit => Answer7 { 3 }\n", "answer7"),
        3
    );
}

#[test]
fn canonc_takes_the_literal_from_the_declaration_body() {
    // The parser wants the literal that opens a brace body, so a
    // number loose in the declaration head is not the answer. The
    // tokenizer alone would have taken the first `Number` it saw and
    // returned 7.
    assert_eq!(canonc_answer("body.can", "Unit => Answer 7 { 3 }\n"), 3);
}

#[test]
fn canonc_reports_a_body_with_no_literal() {
    // No literal to emit, so `canonc` says so instead of emitting a
    // module for a number it never read. Before the parser this was a
    // silent `i32.const 0`.
    assert_eq!(
        canonc_stdout("empty.can", "Unit => Answer { }\n"),
        "declaration body is empty"
    );
}

#[test]
fn canonc_encodes_literals_past_one_leb_byte() {
    // `i32.const` takes a signed LEB128 operand, so 64 is `c0 00`, not
    // the `40` a single byte would give — that decodes as -64. Past
    // 127 a one-byte operand isn't even well formed, and the module
    // failed to validate. The section and body sizes move with the
    // operand's width, so they are computed rather than baked in.
    assert_eq!(canonc_answer("n64.can", "Unit => Answer { 64 }\n"), 64);
    assert_eq!(canonc_answer("n200.can", "Unit => Answer { 200 }\n"), 200);
    assert_eq!(
        canonc_answer("n100000.can", "Unit => Answer { 100000 }\n"),
        100000
    );
}

#[test]
fn canonc_compiles_an_arithmetic_chain() {
    // The body is no longer one instruction: each `-> Op(n)` step
    // appends an `i32.const` and the operator, and the body and section
    // sizes follow the instruction stream's length.
    assert_eq!(
        canonc_answer("sum.can", "Unit => Answer { 1 -> Sum(2) }\n"),
        3
    );
    assert_eq!(
        canonc_answer("diff.can", "Unit => Answer { 10 -> Difference(4) }\n"),
        6
    );
    assert_eq!(
        canonc_answer("prod.can", "Unit => Answer { 6 -> Product(7) }\n"),
        42
    );
    assert_eq!(
        canonc_answer("quot.can", "Unit => Answer { 20 -> Quotient(6) }\n"),
        3
    );
    assert_eq!(
        canonc_answer("rem.can", "Unit => Answer { 20 -> Remainder(6) }\n"),
        2
    );
    // Chains keep folding left, the way the pipe reads.
    assert_eq!(
        canonc_answer(
            "chain.can",
            "Unit => Answer { 1 -> Sum(2) -> Product(10) }\n"
        ),
        30
    );
}

#[test]
fn canonc_reports_what_the_grammar_wanted() {
    // Each parse step names the token it expected, so a malformed body
    // is a diagnostic rather than a module built from whatever the
    // scanner happened to find.
    for (name, source, want) in [
        (
            "op.can",
            "Unit => Answer { 1 -> Frobnicate(2) }\n",
            "Frobnicate is not a declaration or an operation",
        ),
        (
            "lparen.can",
            "Unit => Answer { 1 -> Sum 2 }\n",
            "expected `(`",
        ),
        (
            "rparen.can",
            "Unit => Answer { 1 -> Sum(2 }\n",
            "expected `)`",
        ),
        ("bare.can", "Unit => Answer { -> }\n", "expected a literal"),
    ] {
        assert_eq!(canonc_stdout(name, source), want, "for source {source:?}");
    }
}

#[test]
fn canonc_exports_the_declared_name() {
    // The declaration's name reaches the export section rather than
    // being thrown away — every module used to export `answer` no
    // matter what it was called. The name is lowercased, so the type
    // `Total` exports `total`, and the export section's size follows
    // the name's length.
    assert_eq!(
        canonc_export("total.can", "Unit => Total { 7 }\n", "total"),
        7
    );
    assert_eq!(
        canonc_export("sum2.can", "Unit => Grand { 1 -> Sum(2) }\n", "grand"),
        3
    );
}

#[test]
fn canonc_compiles_a_parameter() {
    // A declaration whose input isn't `Unit` takes one i32: the type
    // section grows a parameter, and naming it in the body reads it
    // back with `local.get 0`.
    assert_eq!(
        canonc_apply(
            "double.can",
            "Int => Double { Int -> Product(2) }\n",
            "double",
            Some(21)
        ),
        42
    );
    // The parameter is named by its declared type, whatever that is.
    assert_eq!(
        canonc_apply(
            "bumped.can",
            "Count => Bumped { Count -> Sum(1) }\n",
            "bumped",
            Some(9)
        ),
        10
    );
}

#[test]
fn canonc_rejects_a_name_that_is_not_the_parameter() {
    // `Unit` declares no parameter, so there is nothing for a name in
    // the body to refer to.
    assert_eq!(
        canonc_stdout("noparam.can", "Unit => Answer { Int -> Sum(1) }\n"),
        "expected a literal"
    );
}

#[test]
fn canonc_compiles_the_parameter_as_an_operand() {
    // The parameter reads back on either side of an operation, not
    // just as the body's head.
    assert_eq!(
        canonc_apply(
            "halved.can",
            "Int => Halved { 100 -> Difference(Int) }\n",
            "halved",
            Some(60)
        ),
        40
    );
    // Both sides at once, so `local.get 0` is emitted twice.
    assert_eq!(
        canonc_apply(
            "squared.can",
            "Int => Squared { Int -> Product(Int) }\n",
            "squared",
            Some(7)
        ),
        49
    );
    // Still nothing to refer to when the declaration takes `Unit`.
    assert_eq!(
        canonc_stdout("noparamop.can", "Unit => Answer { 2 -> Product(Int) }\n"),
        "expected a literal operand"
    );
}

#[test]
fn canonc_compiles_more_than_one_declaration() {
    // Every declaration becomes its own exported function: the type,
    // function, export and code sections all carry one entry each, with
    // counts and sizes derived from what was compiled.
    let source = "Unit => Answer { 7 }\n\nInt => Double { Int -> Product(2) }\n";
    assert_eq!(canonc_export("multi.can", source, "answer"), 7);
    assert_eq!(canonc_apply("multi.can", source, "double", Some(21)), 42);
}

#[test]
fn canonc_compiles_comparisons() {
    // The comparisons are ordinary binary operations that happen to
    // yield 0 or 1, so they need no signature of their own.
    assert_eq!(
        canonc_answer("lt.can", "Unit => Answer { 3 -> Lt(5) }\n"),
        1
    );
    assert_eq!(
        canonc_answer("nlt.can", "Unit => Answer { 5 -> Lt(3) }\n"),
        0
    );
    assert_eq!(
        canonc_answer("eq.can", "Unit => Answer { 4 -> Eq(4) }\n"),
        1
    );
    assert_eq!(
        canonc_answer("ne.can", "Unit => Answer { 4 -> Ne(4) }\n"),
        0
    );
    assert_eq!(
        canonc_answer("ge.can", "Unit => Answer { 4 -> Ge(4) }\n"),
        1
    );
    assert_eq!(
        canonc_apply(
            "gtp.can",
            "Int => Positive { Int -> Gt(0) }\n",
            "positive",
            Some(-3)
        ),
        0
    );
}

#[test]
fn canonc_compiles_a_call_to_another_declaration() {
    // A name in the chain that is not an operation is a call to a
    // declaration in the same file, resolved through a table of every
    // declared name built before any body is parsed — so a call can
    // point forward as well as back.
    let source = "Int => Double { Int -> Product(2) }\n\nInt => Quad { Int -> Double -> Double }\n";
    assert_eq!(canonc_apply("quad.can", source, "quad", Some(5)), 20);

    let forward =
        "Int => Quad { Int -> Double -> Double }\n\nInt => Double { Int -> Product(2) }\n";
    assert_eq!(canonc_apply("fwd.can", forward, "quad", Some(5)), 20);

    // A call is an operand too, so it composes with the arithmetic.
    let mixed = "Int => Double { Int -> Product(2) }\n\nInt => Odd { Int -> Double -> Sum(1) }\n";
    assert_eq!(canonc_apply("odd.can", mixed, "odd", Some(3)), 7);
}

#[test]
fn canonc_rejects_a_call_to_a_declaration_taking_no_parameter() {
    // A `Unit` declaration has nothing to pipe into, so its name is not
    // a call target — it goes into the table as a hole that no name
    // matches, which keeps every other declaration's index in place.
    assert_eq!(
        canonc_stdout(
            "unitcall.can",
            "Unit => Seed { 7 }\n\nInt => Grown { Int -> Seed }\n"
        ),
        "Seed is not a declaration or an operation"
    );
}
