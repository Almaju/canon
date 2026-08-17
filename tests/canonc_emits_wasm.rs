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

/// Same again, for a declaration taking two parameters.
fn canonc_apply2(name: &str, source: &str, export: &str, a: i32, b: i32) -> i32 {
    let hex = canonc_stdout(name, source);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    instance
        .get_typed_func::<(i32, i32), i32>(&mut store, export)
        .unwrap_or_else(|_| panic!("emitted module should export `{export}`"))
        .call(&mut store, (a, b))
        .expect("call the export")
}

/// Same again, for a declaration returning a string: the export hands
/// back the `(ptr, len)` pair and the bytes are read out of the module's
/// own exported memory.
fn canonc_string(name: &str, source: &str, export: &str) -> String {
    let hex = canonc_stdout(name, source);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    let (ptr, len) = instance
        .get_typed_func::<(), (i32, i32)>(&mut store, export)
        .unwrap_or_else(|_| panic!("emitted module should export `{export}`"))
        .call(&mut store, ())
        .expect("call the export");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("emitted module should export its memory");
    let mut out = vec![0u8; len as usize];
    memory
        .read(&store, ptr as usize, &mut out)
        .expect("read the string out of memory");
    String::from_utf8(out).expect("utf-8 string")
}

/// Same again, for a declaration taking a string: the bytes are written
/// into the module's memory past the data segment and handed in as the
/// `(ptr, len)` pair the parameter expects.
fn canonc_string_arg(name: &str, source: &str, export: &str, arg: &str) -> String {
    let hex = canonc_stdout(name, source);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("emitted module should export its memory");
    // past anything the bump allocator will hand out in these tests
    let at = 32768;
    memory
        .write(&mut store, at, arg.as_bytes())
        .expect("write the argument into memory");
    let f = instance
        .get_func(&mut store, export)
        .unwrap_or_else(|| panic!("emitted module should export `{export}`"));
    let mut results = [wasmtime::Val::I32(0), wasmtime::Val::I32(0)];
    f.call(
        &mut store,
        &[
            wasmtime::Val::I32(at as i32),
            wasmtime::Val::I32(arg.len() as i32),
        ],
        &mut results,
    )
    .expect("call the export");
    let (ptr, len) = match (&results[0], &results[1]) {
        (wasmtime::Val::I32(p), wasmtime::Val::I32(l)) => (*p, *l),
        other => panic!("expected a (ptr, len) pair, got {other:?}"),
    };
    let mut out = vec![0u8; len as usize];
    memory
        .read(&store, ptr as usize, &mut out)
        .expect("read the result out of memory");
    String::from_utf8(out).expect("utf-8 string")
}

/// Same again, for a declaration taking a product: the fields are laid
/// out in the module's memory and the pointer to them is the argument.
fn canonc_product_arg(name: &str, source: &str, export: &str, fields: &[i32]) -> i32 {
    let hex = canonc_stdout(name, source);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("emitted module should export its memory");
    let at = 32768;
    let mut raw = Vec::new();
    for f in fields {
        raw.extend_from_slice(&f.to_le_bytes());
    }
    memory
        .write(&mut store, at, &raw)
        .expect("lay the product out in memory");
    instance
        .get_typed_func::<i32, i32>(&mut store, export)
        .unwrap_or_else(|e| panic!("export `{export}`: {e}"))
        .call(&mut store, at as i32)
        .expect("call the export")
}

/// Same again, for a declaration returning a union: the tagged cell's
/// tag and payload are read back out of the module's memory.
fn canonc_tagged(name: &str, source: &str, export: &str, arg: i32) -> (i32, i32) {
    let hex = canonc_stdout(name, source);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes).expect("emitted wasm should validate");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("emitted wasm should instantiate");
    let ptr = instance
        .get_typed_func::<i32, i32>(&mut store, export)
        .unwrap_or_else(|e| panic!("export `{export}`: {e}"))
        .call(&mut store, arg)
        .expect("call the export");
    let memory = instance
        .get_memory(&mut store, "memory")
        .expect("emitted module should export its memory");
    let mut cell = [0u8; 8];
    memory
        .read(&store, ptr as usize, &mut cell)
        .expect("read the tagged cell");
    (
        i32::from_le_bytes(cell[0..4].try_into().expect("tag")),
        i32::from_le_bytes(cell[4..8].try_into().expect("payload")),
    )
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
        "expected a literal"
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
            "expected `->` or `)`",
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

#[test]
fn canonc_compiles_a_dispatch() {
    // `-> ( * False { … } * True { … } )` is wasm's `if (result i32)`.
    // The arms come in alphabetical order, the way Canon writes them,
    // and are emitted the other way round: `if` takes the branch for a
    // non-zero scrutinee, which is `True`.
    let sign = "Int => Sign { Int -> Lt(0) -> ( * False { 1 } * True { 0 -> Difference(1) } ) }\n";
    assert_eq!(canonc_apply("sign.can", sign, "sign", Some(-9)), -1);
    assert_eq!(canonc_apply("sign.can", sign, "sign", Some(9)), 1);

    // An arm body is a body: operand, chain, nested dispatch and all.
    let clamp = "Int => Clamp { Int -> Gt(10) -> ( * False { Int -> Lt(0) -> ( * False { Int } * True { 0 } ) } * True { 10 } ) }\n";
    assert_eq!(canonc_apply("clamp.can", clamp, "clamp", Some(-4)), 0);
    assert_eq!(canonc_apply("clamp.can", clamp, "clamp", Some(4)), 4);
    assert_eq!(canonc_apply("clamp.can", clamp, "clamp", Some(40)), 10);

    // The dispatch is a chain step, so the chain keeps going after it.
    let bumped = "Int => Bumped { Int -> Gt(0) -> ( * False { 0 } * True { Int } ) -> Sum(100) }\n";
    assert_eq!(
        canonc_apply("dispatchbump.can", bumped, "bumped", Some(-3)),
        100
    );
    assert_eq!(
        canonc_apply("dispatchbump.can", bumped, "bumped", Some(3)),
        103
    );
}

#[test]
fn canonc_compiles_recursion() {
    // Branching plus calls is a base case plus a recursive step, which
    // is the first program `canonc` can compile that it could not have
    // unrolled: the module it emits computes rather than evaluates.
    let fact = "Int => Factorial { Int -> Le(1) -> ( * False { Int -> Difference(1) -> Factorial -> Product(Int) } * True { 1 } ) }\n";
    assert_eq!(canonc_apply("fact.can", fact, "factorial", Some(0)), 1);
    assert_eq!(canonc_apply("fact.can", fact, "factorial", Some(5)), 120);
    assert_eq!(
        canonc_apply("fact.can", fact, "factorial", Some(10)),
        3628800
    );

    // Mutual recursion needs the forward half of the name table.
    let parity = "Int => Even { Int -> Eq(0) -> ( * False { Int -> Difference(1) -> Odd } * True { 1 } ) }\n\nInt => Odd { Int -> Eq(0) -> ( * False { Int -> Difference(1) -> Even } * True { 0 } ) }\n";
    assert_eq!(canonc_apply("parity.can", parity, "even", Some(10)), 1);
    assert_eq!(canonc_apply("parity.can", parity, "even", Some(7)), 0);
    assert_eq!(canonc_apply("parity.can", parity, "odd", Some(7)), 1);
}

#[test]
fn canonc_reports_what_a_dispatch_wanted() {
    // The arms are fixed — `False` then `True`, in the order Canon
    // requires — so each token of the shape is checked and named.
    for (name, source, want) in [
        (
            "nostar.can",
            "Unit => Answer { 1 -> Eq(1) -> ( False { 2 } * True { 3 } ) }\n",
            "expected `*`",
        ),
        (
            "noname.can",
            "Unit => Answer { 1 -> Eq(1) -> ( * Nope { 2 } * True { 3 } ) }\n",
            "expected `False`",
        ),
        (
            "noorder.can",
            "Unit => Answer { 1 -> Eq(1) -> ( * False { 2 } * Nope { 3 } ) }\n",
            "expected `True`",
        ),
        (
            "noclose.can",
            "Unit => Answer { 1 -> Eq(1) -> ( * False { 2 } * True { 3 } }\n",
            "expected `)`",
        ),
    ] {
        assert_eq!(canonc_stdout(name, source), want, "for source {source:?}");
    }
}

#[test]
fn canonc_reads_a_head_that_follows_type_declarations() {
    // Canonical Canon puts every type declaration before the first
    // constructor, so a real file's tokens open with `Total = Int`, not
    // with the declaration head. The parameter is the name the head puts
    // before `=>`, which needs `=>` to be its own token — sharing `=`'s
    // made the alias's own left side look like the parameter.
    assert_eq!(
        canonc_apply(
            "aliased.can",
            "Total = Int\n\nCount => Bumped { Count -> Sum(1) }\n",
            "bumped",
            Some(9)
        ),
        10
    );
    // Products and unions in front of it are skipped the same way.
    assert_eq!(
        canonc_apply(
            "aliased2.can",
            "Pair = Left * Right\n\nSign = Down + Up\n\nInt => Twice { Int -> Product(2) }\n",
            "twice",
            Some(6)
        ),
        12
    );
}

#[test]
fn canonc_compiles_more_than_one_parameter() {
    // The head's inputs are the run of names before `=>`, so a
    // declaration takes as many `i32`s as it names — and a body reads
    // each one back by the local index its position gives it.
    let area = "Base * Height => Area { Base -> Product(Height) -> Quotient(2) }\n";
    assert_eq!(canonc_apply2("area.can", area, "area", 6, 4), 12);

    // The second call argument is an expression in its own right, not
    // just a literal or a name.
    let gcd = "Left * Right => Gcd { Right -> Eq(0) -> ( * False { Right -> Gcd(Left -> Remainder(Right)) } * True { Left } ) }\n";
    assert_eq!(canonc_apply2("gcd.can", gcd, "gcd", 1071, 462), 21);
}

#[test]
fn canonc_nests_call_arguments() {
    // An argument is parsed by the same step that parses a body, closed
    // by `)` instead of `}`, so it nests to any depth.
    let deep = "Int => Deep { 1 -> Sum(2 -> Product(3 -> Sum(4))) }\n";
    assert_eq!(canonc_apply("deep.can", deep, "deep", Some(0)), 15);
    // And the diagnostic names the terminator the argument wanted.
    assert_eq!(
        canonc_stdout("unclosed.can", "Unit => Answer { 1 -> Sum(2 -> Sum(3) }\n"),
        "expected `->` or `)`"
    );
}

#[test]
fn canonc_calls_a_declaration_that_takes_nothing() {
    // A `Unit` declaration is a constant, and naming one in a body calls
    // it — as the body's head or as an operand. Until now it could be
    // declared and never used. The name table tells the two arities
    // apart, so the same name still can't be *piped* into: there is
    // nothing for the receiver to fill.
    let head = "Unit => Seed { 7 }\n\nUnit => Total { Seed -> Sum(1) }\n";
    assert_eq!(canonc_export("seedhead.can", head, "total"), 8);

    let operand = "Unit => Seed { 7 }\n\nInt => Shifted { Int -> Sum(Seed) }\n";
    assert_eq!(canonc_apply("seedarg.can", operand, "shifted", Some(2)), 9);

    assert_eq!(
        canonc_stdout(
            "seedpipe.can",
            "Unit => Seed { 7 }\n\nInt => Nope { Int -> Seed }\n"
        ),
        "Seed is not a declaration or an operation"
    );
}

#[test]
fn canonc_compiles_a_string_literal() {
    // A string is a `(ptr, len)` pair into the module's own linear
    // memory — the reference compiler's representation — so a
    // declaration returning one has two results, and the bytes ride in a
    // data segment rather than being built at runtime.
    assert_eq!(
        canonc_string(
            "greeting.can",
            "Unit => Greeting { \"hello\" }\n",
            "greeting"
        ),
        "hello"
    );

    // The table is interned and shared across declarations: one copy of
    // the bytes, and the second declaration points at the same offset.
    let shared =
        "Unit => Greeting { \"hi\" }\n\nUnit => Other { \"hi\" }\n\nUnit => Third { \"bye\" }\n";
    assert_eq!(canonc_string("shared.can", shared, "greeting"), "hi");
    assert_eq!(canonc_string("shared.can", shared, "other"), "hi");
    assert_eq!(canonc_string("shared.can", shared, "third"), "bye");
    // One copy of the bytes: "bye" and "hi" back to back, five in all,
    // rather than seven with "hi" written twice.
    let hex = canonc_stdout("shared.can", shared);
    assert!(hex.ends_with("056279656869"), "data segment was {hex}");

    // Nothing else takes a string yet, and saying so beats emitting a
    // module that reads the bytes as a number.
    assert_eq!(
        canonc_stdout("strop.can", "Unit => Answer { \"hi\" -> Sum(1) }\n"),
        "Sum is not a declaration or an operation"
    );
    assert_eq!(
        canonc_stdout("strarg.can", "Unit => Answer { 1 -> Sum(\"hi\") }\n"),
        "this operation takes a number"
    );
}

#[test]
fn canonc_sizes_a_section_past_one_byte() {
    // Section sizes and entry counts are LEB128, not a single byte: a
    // body longer than 127 bytes used to emit a size with the
    // continuation bit set and no byte to follow it.
    let long = format!(
        "Unit => Answer {{ 0 {} }}\n",
        (0..60).map(|_| "-> Sum(1)").collect::<Vec<_>>().join(" ")
    );
    assert_eq!(canonc_answer("long.can", &long), 60);
}

#[test]
fn canonc_compiles_a_string_parameter() {
    // `canonc` reads the file's type declarations now, resolving each
    // name through its aliases to `Int` or `String`. A `String`-rooted
    // parameter takes two `i32`s rather than one, so the signature and
    // every later parameter's local index follow from the types rather
    // than from counting names.
    let echo = "Echo = String\n\nText = String\n\nText => Echo { Text }\n";
    assert_eq!(
        canonc_string_arg("echo.can", echo, "echo", "round trip"),
        "round trip"
    );

    // An alias chain resolves the whole way down.
    let deep =
        "Echo = String\n\nName = Word\n\nWord = Text\n\nText = String\n\nName => Echo { Name }\n";
    assert_eq!(
        canonc_string_arg("deepecho.can", deep, "echo", "aliased"),
        "aliased"
    );
}

#[test]
fn canonc_indexes_locals_past_a_string_parameter() {
    // A string takes two local slots, so a scalar declared after one
    // reads back at index 2, not 1. Counting parameters rather than
    // slots put every later local one short.
    let bumped = "Bumped = Int\n\nCount = Int\n\nText = String\n\nText * Count => Bumped { Count -> Sum(1) }\n";
    let hex = canonc_stdout("bumped2.can", bumped);
    assert!(hex.contains("2002410"), "expected `local.get 2` in {hex}");

    // And a string declared after a scalar starts at 1.
    let taken =
        "Count = Int\n\nTaken = String\n\nText = String\n\nCount * Text => Taken { Text }\n";
    let hex = canonc_stdout("taken.can", taken);
    assert!(
        hex.contains("20012002"),
        "expected `local.get 1/2` in {hex}"
    );
}

#[test]
fn canonc_compiles_a_string_chain() {
    // The value in hand carries its kind through the chain now, so a
    // string can be an operand and the operations that apply to it are
    // the string ones. `Length` drops the pointer and keeps the count.
    let size = "Size = Int\n\nText = String\n\nText => Size { Text -> Length }\n";
    let hex = canonc_stdout("size.can", size);
    // one i32 scratch local, then the two parameter slots read back,
    // stashed, the pointer dropped and the count returned
    assert!(
        hex.contains("01057f200020012102 1a2002".replace(' ', "").as_str()),
        "expected the length sequence in {hex}"
    );

    // A string literal is an ordinary operand: the chain keeps going.
    assert_eq!(
        canonc_answer(
            "strlen.can",
            "Answer = Int\n\nUnit => Answer { \"hello\" -> Length -> Sum(1) }\n"
        ),
        6
    );

    // And the kind decides which operations are in reach, in both
    // directions — this used to emit an `i32.add` over a pointer.
    assert_eq!(
        canonc_stdout(
            "numop.can",
            "Answer = Int\n\nUnit => Answer { \"hi\" -> Sum(1) }\n"
        ),
        "Sum is not a declaration or an operation"
    );
    assert_eq!(
        canonc_stdout(
            "strdispatch.can",
            "Answer = Int\n\nUnit => Answer { \"hi\" -> ( * False { 0 } * True { 1 } ) }\n"
        ),
        "a dispatch needs a number to branch on"
    );
}

#[test]
fn canonc_concatenates_strings() {
    // `Joined` is the first operation that has to allocate. The bump
    // pointer is a mutable global starting past the data segment, and
    // it moves inline — `global.get; global.get; n; i32.add;
    // global.set` — so no helper function exists to shift every other
    // function's index.
    assert_eq!(
        canonc_string(
            "join.can",
            "Greeting = String\n\nUnit => Greeting { \"hello, \" -> Joined(\"world\") }\n",
            "greeting"
        ),
        "hello, world"
    );

    // It chains, so the second concatenation allocates over the first's
    // result rather than reusing the literals' offsets.
    assert_eq!(
        canonc_string(
            "join3.can",
            "Greeting = String\n\nUnit => Greeting { \"a\" -> Joined(\"b\") -> Joined(\"c\") }\n",
            "greeting"
        ),
        "abc"
    );

    // And it works on what a parameter brought in.
    assert_eq!(
        canonc_string_arg(
            "shout.can",
            "Shout = String\n\nText = String\n\nText => Shout { Text -> Joined(\"!\") }\n",
            "shout",
            "hey"
        ),
        "hey!"
    );

    // The argument's kind has to match what the operation produces.
    assert_eq!(
        canonc_stdout(
            "joinnum.can",
            "Greeting = String\n\nUnit => Greeting { \"a\" -> Joined(1) }\n"
        ),
        "this operation takes a string"
    );
}

#[test]
fn canonc_indexes_a_string_byte() {
    // `ByteAt` is 1-based, the way Canon indexes everywhere, so the
    // emitted address is `ptr + n - 1`. No allocation — the pointer and
    // the index are all it needs.
    assert_eq!(
        canonc_answer(
            "byte.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> ByteAt(2) }\n"
        ),
        98
    );
    assert_eq!(
        canonc_answer(
            "byte1.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> ByteAt(1) }\n"
        ),
        97
    );
    // It yields a number, so the numeric operations pick up after it.
    assert_eq!(
        canonc_answer(
            "bytesum.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> ByteAt(3) -> Difference(96) }\n"
        ),
        3
    );
}

#[test]
fn canonc_compares_strings() {
    // `Eq` on strings needs a loop, so it is the one helper function —
    // prepended at index 0 so every declaration shifts up one rather
    // than appended at an index the parse can't know yet.
    for (name, source, want) in [
        (
            "eqsame.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> Eq(\"abc\") }\n",
            1,
        ),
        (
            "eqdiff.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> Eq(\"abd\") }\n",
            0,
        ),
        (
            "eqshort.can",
            "Answer = Int\n\nUnit => Answer { \"abc\" -> Eq(\"ab\") }\n",
            0,
        ),
        (
            "eqempty.can",
            "Answer = Int\n\nUnit => Answer { \"\" -> Eq(\"\") }\n",
            1,
        ),
    ] {
        assert_eq!(canonc_answer(name, source), want, "for {source:?}");
    }

    // It yields a number, so a dispatch can branch on it.
    assert_eq!(
        canonc_answer(
            "eqbranch.can",
            "Answer = Int\n\nUnit => Answer { \"yes\" -> Eq(\"yes\") -> ( * False { 10 } * True { 20 } ) }\n"
        ),
        20
    );

    // And it compares what a parameter brought in.
    assert_eq!(
        canonc_string_arg(
            "eqparam.can",
            "Shout = String\n\nText = String\n\nText => Shout { Text -> Eq(\"ok\") -> ( * False { \"no\" } * True { \"yes\" } ) }\n",
            "shout",
            "ok"
        ),
        "yes"
    );
}

#[test]
fn canonc_slices_a_string() {
    // `Substring` takes a pair, which is the first argument list with a
    // `*` in it. Slicing allocates nothing: strings are immutable, so
    // the result is a pointer and length into the same bytes.
    assert_eq!(
        canonc_string(
            "sub.can",
            "From = Int\n\nPart = String\n\nTo = Int\n\nUnit => Part { \"hello world\" -> Substring(1 -> From * 5 -> To) }\n",
            "part"
        ),
        "hello"
    );
    assert_eq!(
        canonc_string(
            "sub2.can",
            "From = Int\n\nPart = String\n\nTo = Int\n\nUnit => Part { \"hello world\" -> Substring(7 -> From * 11 -> To) }\n",
            "part"
        ),
        "world"
    );
    // Both bounds are numbers.
    assert_eq!(
        canonc_stdout(
            "subbad.can",
            "From = Int\n\nPart = String\n\nUnit => Part { \"abc\" -> Substring(1 -> From * \"x\") }\n"
        ),
        "a slice bound must be a number"
    );
}

#[test]
fn canonc_relabels_through_a_newtype() {
    // `-> From` and `-> Acc` are relabels, not calls: a pipe into a
    // declared type emits nothing and only changes what the value is
    // called. Canon leans on them constantly, and `canonc` used to
    // reject every one as an unknown operation.
    assert_eq!(
        canonc_answer(
            "relabel.can",
            "Acc = Int\n\nAnswer = Int\n\nUnit => Answer { 1 -> Sum(2) -> Acc -> Product(10) }\n"
        ),
        30
    );
    // A relabel can change the kind, and the operations follow it.
    assert_eq!(
        canonc_answer(
            "relabelkind.can",
            "Answer = Int\n\nWord = Text\n\nText = String\n\nUnit => Answer { \"abc\" -> Word -> Length }\n"
        ),
        3
    );
    // An unknown name is still an unknown name.
    assert_eq!(
        canonc_stdout(
            "unknown.can",
            "Answer = Int\n\nUnit => Answer { 1 -> Zork }\n"
        ),
        "Zork is not a declaration or an operation"
    );
}

#[test]
fn canonc_renders_a_number() {
    // `-> String` on a number is the second helper: allocate twelve
    // bytes, fill them backwards, and hand back the pointer and length
    // of what was written.
    for (name, source, want) in [
        (
            "itoa0.can",
            "Shown = String\n\nUnit => Shown { 0 -> String }\n",
            "0",
        ),
        (
            "itoa7.can",
            "Shown = String\n\nUnit => Shown { 7 -> String }\n",
            "7",
        ),
        (
            "itoa42.can",
            "Shown = String\n\nUnit => Shown { 42 -> String }\n",
            "42",
        ),
        (
            "itoabig.can",
            "Shown = String\n\nUnit => Shown { 100000 -> String }\n",
            "100000",
        ),
        (
            "itoaneg.can",
            "Shown = String\n\nUnit => Shown { 0 -> Difference(45) -> String }\n",
            "-45",
        ),
    ] {
        assert_eq!(canonc_string(name, source, "shown"), want, "for {source:?}");
    }

    // It yields a string, so the string operations pick up after it.
    assert_eq!(
        canonc_string(
            "itoajoin.can",
            "Shown = String\n\nUnit => Shown { 12 -> String -> Joined(\" apples\") }\n",
            "shown"
        ),
        "12 apples"
    );
    assert_eq!(
        canonc_answer(
            "itoalen.can",
            "Answer = Int\n\nUnit => Answer { 1234 -> String -> Length }\n"
        ),
        4
    );
}

#[test]
fn canonc_compiles_a_format_string() {
    // A backtick literal is text chunks and `{…}` holes, folded into a
    // `Joined` chain. The scanner needs a mode for it: inside the
    // backticks, text is text until a `{`, and inside a hole the
    // ordinary rules apply again until the matching `}`.
    assert_eq!(
        canonc_string(
            "fmt.can",
            "Shown = String\n\nUnit => Shown { `hello, world` }\n",
            "shown"
        ),
        "hello, world"
    );

    // A hole holding a string goes in as it stands; one holding a
    // number renders first.
    assert_eq!(
        canonc_string(
            "fmthole.can",
            "Shown = String\n\nUnit => Shown { `there are {2 -> Sum(3)} left` }\n",
            "shown"
        ),
        "there are 5 left"
    );
    assert_eq!(
        canonc_string_arg(
            "fmtstr.can",
            "Shown = String\n\nText = String\n\nText => Shown { `<{Text}>` }\n",
            "shown",
            "body"
        ),
        "<body>"
    );

    // Holes at either end, and several of them.
    assert_eq!(
        canonc_string(
            "fmtmany.can",
            "Shown = String\n\nUnit => Shown { `{1 -> String}-{2 -> String}-{3 -> String}` }\n",
            "shown"
        ),
        "1-2-3"
    );
    assert_eq!(
        canonc_string(
            "fmtempty.can",
            "Shown = String\n\nUnit => Shown { `` }\n",
            "shown"
        ),
        ""
    );

    // And the result is a string, so the chain keeps going.
    assert_eq!(
        canonc_answer(
            "fmtlen.can",
            "Answer = Int\n\nUnit => Answer { `ab{1 -> String}` -> Length }\n"
        ),
        3
    );
}

#[test]
fn canonc_reads_a_product_field() {
    // A product is a pointer to its fields laid out in declaration
    // order — four bytes for a scalar, eight for a string's pointer and
    // length. `.` reads one back at its offset.
    let decls = "Left = Int\n\nPair = Left * Right\n\nRight = Int\n\n";
    assert_eq!(
        canonc_product_arg(
            "pleft.can",
            &format!("{decls}Pair => Left {{ Pair.Left }}\n"),
            "left",
            &[11, 22]
        ),
        11
    );
    assert_eq!(
        canonc_product_arg(
            "pright.can",
            &format!("{decls}Pair => Right {{ Pair.Right }}\n"),
            "right",
            &[11, 22]
        ),
        22
    );

    // The field's own type is what the value carries away, so the
    // operations that follow are that type's.
    assert_eq!(
        canonc_product_arg(
            "pchain.can",
            &format!("{decls}Pair => Left {{ Pair.Left -> Sum(Pair.Right) }}\n"),
            "left",
            &[11, 22]
        ),
        33
    );

    // A name that isn't a field, and a `.` on something with no fields.
    assert_eq!(
        canonc_stdout(
            "pnofield.can",
            &format!("{decls}Pair => Left {{ Pair.Nope }}\n")
        ),
        "Pair has no field Nope"
    );
    assert_eq!(
        canonc_stdout(
            "pnoprod.can",
            "Answer = Int\n\nCount = Int\n\nCount => Answer { Count.Nope }\n"
        ),
        "Count has no fields to read"
    );
}

#[test]
fn canonc_builds_a_product() {
    // Constructing allocates the fields' total width from the bump
    // pointer and stores each value at the offset of the field whose
    // *type* it carries — so the receiver and the argument find their
    // slots by name, not by the order they were written.
    let decls =
        "Boxed = Int\n\nLeft = Int\n\nPair = Left * Right\n\nRight = Int\n\nTotal = Int\n\n\
                 Pair => Boxed { Pair.Left -> Sum(Pair.Right) }\n\n";
    assert_eq!(
        canonc_apply(
            "build.can",
            &format!("{decls}Right => Total {{ Right -> Pair(3 -> Left) -> Boxed }}\n"),
            "total",
            Some(4)
        ),
        7
    );
    // Written the other way round, the same fields are filled.
    assert_eq!(
        canonc_apply(
            "build2.can",
            &format!("{decls}Left => Total {{ Left -> Pair(9 -> Right) -> Boxed }}\n"),
            "total",
            Some(2)
        ),
        11
    );

    // A declaration of the same name is a call, not a construction —
    // that precedence is what makes `Pair => Boxed` above reachable.
    assert_eq!(
        canonc_stdout(
            "buildnofield.can",
            &format!("{decls}Right => Total {{ Right -> Pair(3 -> Boxed) -> Boxed }}\n")
        ),
        "Pair has no field of type Boxed"
    );
}

#[test]
fn canonc_tags_a_union_variant() {
    // A union is a pointer to a tag and a payload. Piping a value into
    // a union it belongs to allocates the cell, writes the variant's
    // position as the tag and the value after it. The cell is as wide
    // as the widest variant needs, so every variant fits the same slot.
    let decls = "Count = Int\n\nLabel = String\n\nSlot = Count + Label\n\nWrapped = Slot\n\n";
    assert_eq!(
        canonc_tagged(
            "tag0.can",
            &format!("{decls}Count => Wrapped {{ Count -> Slot }}\n"),
            "wrapped",
            42
        ),
        (0, 42)
    );

    // A value whose type isn't one of the union's variants.
    assert_eq!(
        canonc_stdout(
            "tagbad.can",
            &format!("Other = Int\n\n{decls}Count => Wrapped {{ Count -> Other -> Slot }}\n")
        ),
        "Other is not a variant of Slot"
    );
}

#[test]
fn canonc_dispatches_on_a_union() {
    // Arms name variants now, not `False` / `True`. The scrutinee's
    // pointer is parked once and each arm tests the tag it loads back,
    // so the arms nest into one `if` chain closed at the end.
    let decls = "Count = Int\n\nKindof = Int\n\nLabel = String\n\nSlot = Count + Label\n\n";
    let src =
        format!("{decls}Slot => Kindof {{ Slot -> ( * Count {{ 10 }} * Label {{ 20 }} ) }}\n");
    assert_eq!(canonc_product_arg("ud0.can", &src, "kindof", &[0, 7]), 10);
    assert_eq!(canonc_product_arg("ud1.can", &src, "kindof", &[1, 7]), 20);

    // Three arms nest the same way.
    let three =
        "Bit = Int\n\nOne = Int\n\nThree = Int\n\nTwo = Int\n\nDigit = One + Three + Two\n\n";
    let src3 = format!(
        "{three}Digit => Bit {{ Digit -> ( * One {{ 1 }} * Three {{ 3 }} * Two {{ 2 }} ) }}\n"
    );
    assert_eq!(canonc_product_arg("ud3a.can", &src3, "bit", &[0, 0]), 1);
    assert_eq!(canonc_product_arg("ud3b.can", &src3, "bit", &[1, 0]), 3);
    assert_eq!(canonc_product_arg("ud3c.can", &src3, "bit", &[2, 0]), 2);

    // An arm that names something that isn't a variant.
    assert_eq!(
        canonc_stdout(
            "udbad.can",
            &format!("{decls}Slot => Kindof {{ Slot -> ( * Count {{ 10 }} * Nope {{ 20 }} ) }}\n")
        ),
        "Nope is not a variant of Slot"
    );
}
