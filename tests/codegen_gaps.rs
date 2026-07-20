//! Tests for the checker's *codegen-gap errors* — the hard rejections it
//! emits when a program reaches a feature the code generator doesn't
//! implement yet, keeping the accepted language and the implemented
//! language the same set (see the module comment in `src/checker/mod.rs`
//! and `docs/src/reference/codegen-gaps.md`).
//!
//! These live here rather than in the `.can` fixture layers because a
//! couple of the cases need a synthetic `extern` binding the loader
//! wouldn't produce from plain source, and the reachability boundary
//! (`entry_items_start`) needs direct control.

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
fn mark_extern(module: &mut Module, fn_name: &str, path: &str) {
    for item in &mut module.items {
        if let Item::Function(f) = item {
            if f.name.name == fn_name {
                f.extern_wasm = Some(ExternWasm {
                    path: path.to_string(),
                    is_async: false,
                });
            }
        }
    }
}

fn gap_errors(module: &Module, entry_items_start: usize) -> Vec<String> {
    checker::codegen_gap_errors(module, entry_items_start, None)
        .iter()
        .map(|e| e.message().to_string())
        .collect()
}

/// The dispatch-ready HTTP entry of a parsed fixture, plus the gap errors
/// computed with it — the http-world variant of `gap_errors`.
fn http_gap_errors(module: &Module) -> Vec<String> {
    let entry = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(f) if f.name.name == "handler" => Some(f.clone()),
            _ => None,
        })
        .expect("handler should parse");
    checker::codegen_gap_errors(module, 0, Some(&entry))
        .iter()
        .map(|e| e.message().to_string())
        .collect()
}

#[test]
fn rejects_reachable_compound_list_binding() {
    // `readPairs` is a binding returning `List<Pair>` where `Pair` is a
    // product — a compound payload — and `main` reaches it, so codegen
    // would fail.
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
    mark_extern(&mut module, "readPairs", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one gap error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("`List<Pair>`") && errors[0].contains("codegen-gaps.md"),
        "error should name the payload and point to the doc page: {}",
        errors[0]
    );
}

#[test]
fn scalar_list_binding_is_fine() {
    // `List<Int>` returns decode (per-width read-back) — no error. The
    // alias hop (`Duration = Int`) exercises the payload check's alias
    // chase.
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
    mark_extern(&mut module, "readBytes", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert!(
        errors.is_empty(),
        "a scalar list return should not error, got: {:?}",
        errors
    );
}

#[test]
fn rejects_reachable_compound_option_binding() {
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
    mark_extern(&mut module, "readPair", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one option gap error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("`Option<Pair>`"),
        "error should name the option payload: {}",
        errors[0]
    );
}

#[test]
fn compound_payload_behind_return_alias_is_rejected() {
    // Bindings minted by `canon install` return a named result newtype
    // (`ReadPairs = List<Pair>`), not a literal `List<…>` — the payload
    // check must chase the alias to see the compound element.
    let source = r#"
Pair = Left * Right

Left = Int

Right = Int

ReadPairs = List<Pair>

readPairs = () => ReadPairs {
    List()
}

main = () => Unit {
    readPairs()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readPairs", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert_eq!(
        errors.len(),
        1,
        "expected one gap error through the alias, got: {:?}",
        errors
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
    mark_extern(&mut module, "readMaybe", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert!(
        errors.is_empty(),
        "a scalar option return should not error, got: {:?}",
        errors
    );
}

#[test]
fn no_error_for_unreachable_binding() {
    // Same binding, but it's an *imported* item (index < entry_items_start)
    // that `main` never references — the file-granular loader pulls such
    // siblings in wholesale, and they must not error: a build compiles
    // only the reachable set.
    let source = r#"
Pair = Left * Right

Left = Int

Right = Int

readPairs = () => List<Pair> {
    List()
}

main = () => Unit {
    "hi".print()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readPairs", "wasi:example/x#read");

    // Treat everything before `main` as imported; only `main` is entry.
    let entry_start = module.items.len() - 1;
    let errors = gap_errors(&module, entry_start);
    assert!(
        errors.is_empty(),
        "an unreachable binding must not error, got: {:?}",
        errors
    );
}

#[test]
fn string_list_binding_is_fine() {
    // `List<String>` returns work — no error.
    let source = r#"
readLines = () => List<String> {
    List()
}

main = () => Unit {
    readLines()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "readLines", "wasi:example/x#read");

    let errors = gap_errors(&module, 0);
    assert!(
        errors.is_empty(),
        "a `List<String>` return should not error, got: {:?}",
        errors
    );
}

#[test]
fn rejects_reachable_stream_signature() {
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
    let errors = gap_errors(&module, 0);
    assert_eq!(
        errors.len(),
        1,
        "expected one Stream gap error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("Stream<T>"),
        "error should name the Stream gap: {}",
        errors[0]
    );
}

#[test]
fn http_world_rejects_loaded_non_http_externs() {
    // The `wasi:http/service` world links every *loaded* extern into the
    // fixed import block — `WasmGen::new_http` exits on anything beyond
    // `wasi:http/types`, reachable or not, so the checker rejects at the
    // same granularity. `now` is loaded but never called; it must still
    // be named.
    let source = r#"
now = () => Int {
    0
}

handler = (Request) => Response {
    Response()
}
"#;
    let mut module = parse(source);
    mark_extern(&mut module, "now", "wasi:clocks/system-clock@0.3.0#now");

    let errors = http_gap_errors(&module);
    assert_eq!(
        errors.len(),
        1,
        "expected one http-world import error, got: {:?}",
        errors
    );
    assert!(
        errors[0].contains("wasi:clocks/system-clock@0.3.0#now")
            && errors[0].contains("wasi:http/types"),
        "error should name the offending import: {}",
        errors[0]
    );
}

#[test]
fn http_world_allows_http_types() {
    let source = r#"
path = () => String {
    ""
}

handler = (Request) => Response {
    Response()
}
"#;
    let mut module = parse(source);
    mark_extern(
        &mut module,
        "path",
        "wasi:http/types@0.3.0#[method]request.get-path-with-query",
    );

    let errors = http_gap_errors(&module);
    assert!(
        errors.is_empty(),
        "http-world-satisfiable imports must not error, got: {:?}",
        errors
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

/// The WIT shapes `canon install` refuses to bind never enter the accepted
/// language, so the checker has nothing to reject — but the skip reasons
/// are part of the same contract. Pin that each one is documented on the
/// gaps page alongside the checker-rejected features.
#[test]
fn every_install_skip_is_documented() {
    let doc = std::fs::read_to_string("docs/src/reference/codegen-gaps.md")
        .expect("codegen-gaps.md should exist");
    for skip in [
        "resource method",
        "handle in signature",
        "bare `result` parameter",
        "sub-u64 integer inside a compound shape",
    ] {
        assert!(
            doc.contains(skip),
            "`canon install` skip reason `{skip}` is missing from \
             docs/src/reference/codegen-gaps.md"
        );
    }
}
