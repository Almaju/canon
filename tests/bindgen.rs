//! End-to-end tests for `oneway bindgen`.
//!
//! Each test feeds a `.wit` fixture from `tests/fixtures/wit/` through
//! `oneway::bindgen::generate_from_path` and asserts on the produced
//! Oneway source. The source is then re-parsed via the Oneway lexer +
//! parser to make sure it's syntactically valid.

use std::path::Path;

use oneway::bindgen;
use oneway::lexer::Scanner;
use oneway::parser::Parser;

fn parse_oneway(source: &str) {
    let mut scanner = Scanner::new(source);
    let tokens = scanner
        .scan_tokens()
        .expect("generated source should lex cleanly");
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .expect("generated source should parse cleanly");
}

#[test]
fn monotonic_clock_roundtrip() {
    let path = Path::new("tests/fixtures/wit/monotonic-clock.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f.relative_path, "wasi/src/clocks/monotonic_clock.ow");
    assert!(f.skipped.is_empty(), "no items should be skipped");

    // Types and functions both present, alphabetical.
    assert!(f.content.contains("Duration = Int"));
    assert!(f.content.contains("Instant = Int"));
    // Bindgen emits a single file-level `bindings "<urn>"` directive
    // followed by bare function-type aliases — no per-function `extern
    // Wasm` marker. The loader (`apply_bindings_directive` in
    // `src/loader.rs`) rewrites each alias into a real FunctionDef with
    // the URN attached. The URN also lives in `EmittedFile.urn` and the
    // companion `_install.toml` index for callers that need direct
    // access.
    assert!(f
        .content
        .contains("bindings \"wasi:clocks/monotonic-clock@"));
    assert!(
        !f.content.contains("extern Wasm"),
        "bindgen should not emit per-function `extern Wasm` anymore",
    );
    assert_eq!(f.urn, "wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15");
    assert!(f.content.contains("getResolution = () -> Duration"));
    assert!(f.content.contains("now = () -> Instant"));

    // Alphabetical ordering: Duration < Instant; getResolution < now.
    let d = f.content.find("Duration = ").unwrap();
    let i = f.content.find("Instant = ").unwrap();
    assert!(d < i, "types should be alphabetical");
    let g = f.content.find("getResolution").unwrap();
    let n = f.content.find("\nnow ").unwrap();
    assert!(g < n, "functions should be alphabetical");

    parse_oneway(&f.content);
}

#[test]
fn resources_emit_handle_newtypes() {
    // The fixture declares two resources (`counter`, `gauge`) and two free
    // functions — `consume` (takes `own<gauge>`) and `tick` (no handles).
    // We expect:
    //   * `Counter = Handle` and `Gauge = Handle` in the output.
    //   * Every resource method/constructor/static skipped with a reason
    //     mentioning codegen.
    //   * The free function `consume`, whose signature mentions a handle,
    //     skipped with the same kind of reason.
    //   * The plain free function `tick` still emitted.
    //   * The generated source lexes and parses cleanly (i.e. `Handle` is
    //     accepted by the checker as a builtin).
    let path = Path::new("tests/fixtures/wit/resources.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f.relative_path, "demo/src/resources/handles.ow");

    assert!(
        f.content.contains("Counter = Handle"),
        "missing `Counter = Handle` in output:\n{}",
        f.content
    );
    assert!(
        f.content.contains("Gauge = Handle"),
        "missing `Gauge = Handle` in output:\n{}",
        f.content
    );

    // Plain free function still emitted.
    assert!(
        f.content.contains("tick = () -> Int"),
        "plain free fn `tick` should still be emitted:\n{}",
        f.content
    );

    // Resource methods/constructors/statics are skipped with the new
    // "codegen lowering pending" reason (never the old "v1 skips
    // resources" string).
    let codegen_pending: Vec<&String> = f
        .skipped
        .iter()
        .filter(|s| s.contains("codegen lowering pending"))
        .collect();
    assert!(
        codegen_pending.len() >= 4,
        "expected at least 4 skipped items mentioning codegen (3 counter methods + `consume`), got: {:?}",
        f.skipped
    );
    assert!(
        !f.skipped.iter().any(|s| s.contains("v1 skips resources")),
        "resources should no longer be skipped under the old `v1 skips resources` reason:\n{:?}",
        f.skipped
    );

    // The free function `consume`, which takes `own<gauge>`, must be
    // among the codegen-pending skips (proves the handle-in-signature
    // filter catches free fns too, not just methods).
    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("fn consume") && s.contains("handle in signature")),
        "`consume` should be skipped with a `handle in signature` reason; got: {:?}",
        f.skipped
    );

    parse_oneway(&f.content);
}

#[test]
fn kitchen_sink_roundtrip() {
    let path = Path::new("tests/fixtures/wit/kitchen-sink.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f.relative_path, "demo/src/sink/kitchen_sink.ow");
    assert!(f.skipped.is_empty(), "no items should be skipped");

    // Records emit prefixed field newtypes so the resulting product is
    // composed of distinct types (Oneway's "alphabetical-distinct" rule).
    assert!(f.content.contains("Point = PointX * PointY"));
    assert!(f.content.contains("PointX = Float"));
    assert!(f.content.contains("PointY = Float"));

    // Enums become prefixed zero-data unions.
    assert!(f
        .content
        .contains("Color = ColorBlue + ColorGreen + ColorRed"));
    assert!(f.content.contains("ColorBlue = Unit"));

    // Variants — data-carrying arms become 1-component products, others
    // become Unit.
    assert!(f
        .content
        .contains("Shape = ShapeCircle + ShapeEmpty + ShapeRectangle"));
    assert!(f.content.contains("ShapeCircle = Float"));
    assert!(f.content.contains("ShapeEmpty = Unit"));
    assert!(f.content.contains("ShapeRectangle = Point"));

    // Flags become a product of Bool newtypes.
    assert!(f
        .content
        .contains("Style = StyleBold * StyleItalic * StyleUnderline"));
    assert!(f.content.contains("StyleBold = Bool"));

    // Pure alias.
    assert!(f.content.contains("ColorList = List<Color>"));

    // Tuple of identical types collapses to the `T^N` repeat form
    // (rather than `Float * Float * Float`, which would violate the
    // distinct-types rule on products).
    assert!(
        f.content.contains("Triple = Float^3"),
        "tuple of identical types should emit `Float^3`:\n{}",
        f.content
    );

    // Function signatures.
    assert!(f.content.contains("centre = (Shape) -> Option<Point>"));
    assert!(f
        .content
        .contains("paint = (ColorList * Shape) -> Result<Style, String>"));
    assert!(f.content.contains("reset = () -> Unit"));

    parse_oneway(&f.content);
}
