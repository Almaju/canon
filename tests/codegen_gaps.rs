//! Tests for the checker's *codegen-gap warnings* — the non-fatal
//! heads-up it emits when a program type-checks but reaches a feature
//! the code generator doesn't implement yet (see the module comment in
//! `src/checker/mod.rs` and `docs/src/reference/codegen-gaps.md`).
//!
//! These live here rather than in the `.can` fixture layers because the
//! warnings are a separate return channel from the fatal `CheckError`
//! stream: the fixture harness only observes errors, and a couple of the
//! cases need a synthetic `extern` binding the loader wouldn't produce
//! from plain source.

use canon::ast::{ExternWasm, Item, Module};
use canon::checker::{self, CODEGEN_GAPS};
use canon::lexer::Scanner;
use canon::parser::Parser;

fn parse(source: &str) -> Module {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    canon::ast::resolve_new_syntax(&mut module);
    module
}

/// Mark the named function as an `extern` binding, so gap detection treats
/// it as a binding declaration. Real binding files are recognised by path
/// and shape; a hand-authored fixture can't sit under a versioned package
/// dir, so we synthesize the flag directly.
fn mark_extern(module: &mut Module, fn_name: &str) {
    for item in &mut module.items {
        if let Item::Function(f) = item {
            if f.name.name == fn_name {
                f.extern_wasm = Some(ExternWasm {
                    path: "wasi:example/x#read".to_string(),
                    is_async: false,
                });
            }
        }
    }
}

#[test]
fn warns_on_reachable_compound_list_binding() {
    // `readPairs` is a binding returning `List<Pair>` where `Pair` is a
    // product — a compound element type — and `main` reaches it, so
    // codegen would fail.
    let source = r#"
Pair = Left * Right

Left = Int

Right = Int

readPairs = () => List<Pair> {
    List()
}

main = () => Unit {
    readPairs()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readPairs");

    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one gap warning, got: {:?}",
        warnings
    );
    let msg = &warnings[0].message;
    assert!(
        msg.contains("list<T>") && msg.contains("codegen-gaps.md"),
        "warning should name the gap and point to the doc page: {msg}"
    );
}

#[test]
fn scalar_list_binding_is_fine() {
    // `List<Int>` returns decode now (per-width read-back) — no warning.
    // The alias hop (`Duration = Int`) exercises the payload check's
    // alias chase.
    let source = r#"
Duration = Int

readBytes = () => List<Duration> {
    List()
}

main = () => Unit {
    readBytes()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readBytes");

    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert!(
        warnings.is_empty(),
        "a scalar list return should not warn, got: {:?}",
        warnings
    );
}

#[test]
fn warns_on_reachable_compound_option_binding() {
    let source = r#"
Pair = Left * Right

Left = Int

Right = Int

readPair = () => Option<Pair> {
    List()
}

main = () => Unit {
    readPair()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readPair");

    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one option gap warning, got: {:?}",
        warnings
    );
    assert!(
        warnings[0].message.contains("option<T>"),
        "warning should name the option gap: {}",
        warnings[0].message
    );
}

#[test]
fn scalar_option_binding_is_fine() {
    let source = r#"
readMaybe = () => Option<Int> {
    List()
}

main = () => Unit {
    readMaybe()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readMaybe");

    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert!(
        warnings.is_empty(),
        "a scalar option return should not warn, got: {:?}",
        warnings
    );
}

#[test]
fn no_warning_for_unreachable_binding() {
    // Same binding, but it's an *imported* item (index < entry_items_start)
    // that `main` never references — the file-granular loader pulls such
    // siblings in wholesale, and they must not warn.
    let source = r#"
readBytes = () => List<Int> {
    List()
}

main = () => Unit {
    "hi".print()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readBytes");

    // Treat `readBytes` (items[0]) as imported; only `main` is entry.
    let warnings = checker::codegen_gap_warnings(&module, 1);
    assert!(
        warnings.is_empty(),
        "an unreachable binding must not warn, got: {:?}",
        warnings
    );
}

#[test]
fn string_list_binding_is_fine() {
    // `List<String>` returns already work — no warning.
    let source = r#"
readLines = () => List<String> {
    List()
}

main = () => Unit {
    readLines()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readLines");

    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert!(
        warnings.is_empty(),
        "a `List<String>` return should not warn, got: {:?}",
        warnings
    );
}

#[test]
fn warns_on_reachable_stream_signature() {
    // A plain (non-extern) helper whose signature mentions `Stream<T>`:
    // codegen drops such imports, so reaching it fails to link.
    let source = r#"
readChunks = () => Stream<Int> {
    List()
}

main = () => Unit {
    readChunks()
}
"#;
    let module = parse(source);
    let warnings = checker::codegen_gap_warnings(&module, 0);
    assert_eq!(
        warnings.len(),
        1,
        "expected one Stream gap warning, got: {:?}",
        warnings
    );
    assert!(
        warnings[0].message.contains("Stream<T>"),
        "warning should name the Stream gap: {}",
        warnings[0].message
    );
}

/// Single-source-of-truth pin: every gap in `CODEGEN_GAPS` must be
/// documented, by title, in `docs/src/reference/codegen-gaps.md`. If the
/// two drift, this fails and forces them back into lockstep.
#[test]
fn every_gap_is_documented() {
    let doc = std::fs::read_to_string("docs/src/reference/codegen-gaps.md")
        .expect("codegen-gaps.md should exist");
    for gap in CODEGEN_GAPS {
        assert!(
            doc.contains(gap.title),
            "gap `{}` is missing from docs/src/reference/codegen-gaps.md",
            gap.title
        );
    }
}
