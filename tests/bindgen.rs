//! End-to-end tests for `canon bindgen`.
//!
//! Each test feeds a `.wit` fixture from `tests/fixtures/wit/` through
//! `canon::bindgen::generate_from_path` and asserts on the produced
//! Canon source. The source is then re-parsed via the Canon lexer +
//! parser to make sure it's syntactically valid.

use std::path::Path;

use canon::bindgen;
use canon::lexer::Scanner;
use canon::parser::Parser;

fn parse_canon(source: &str) {
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
    assert_eq!(
        f.relative_path, "wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can",
        "output lands in the vendored-package layout: the directory carries the pin"
    );
    assert!(f.skipped.is_empty(), "no items should be skipped");

    // Types and functions both present, alphabetical.
    assert!(f.content.contains("Duration = Int"));
    assert!(f.content.contains("Instant = Int"));
    // Bindgen emits bare function-type aliases and nothing else — no
    // header, no per-function marker. A binding file is recognized by
    // shape, and the loader derives each declaration's URN from the
    // vendored path. The URN also lives in
    // `EmittedFile.urn` for callers that need direct access.
    assert!(
        !f.content.contains("bindings \""),
        "bindgen should not emit the bindings header anymore:\n{}",
        f.content
    );
    assert!(
        !f.content.contains("extern Wasm"),
        "bindgen should not emit per-function `extern Wasm` anymore",
    );
    assert_eq!(f.urn, "wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15");
    // Each function is a string-anchored anonymous constructor keyed by a
    // minted result newtype (`now -> instant` mints `Now = Instant`),
    // whose body names the WIT fragment verbatim.
    assert!(f.content.contains("Now = Instant"));
    assert!(f.content.contains("Unit => Now {\n    \"now\"\n}"));
    assert!(f.content.contains("GetResolution = Duration"));
    assert!(f
        .content
        .contains("Unit => GetResolution {\n    \"get-resolution\"\n}"));

    // Alphabetical ordering by constructed type: GetResolution < Now.
    let g = f.content.find("Unit => GetResolution").unwrap();
    let n = f.content.find("Unit => Now").unwrap();
    assert!(g < n, "constructors should be alphabetical");

    parse_canon(&f.content);
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
    assert_eq!(f.relative_path, "demo/resources@1.0.0/handles.can");

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

    // Plain free function still emitted, as a string-anchored constructor
    // over its minted result newtype.
    assert!(
        f.content.contains("Tick = Int") && f.content.contains("Unit => Tick {\n    \"tick\"\n}"),
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

    parse_canon(&f.content);
}

#[test]
fn kitchen_sink_roundtrip() {
    let path = Path::new("tests/fixtures/wit/kitchen-sink.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f.relative_path, "demo/sink@1.0.0/kitchen_sink.can");
    assert!(f.skipped.is_empty(), "no items should be skipped");

    // Records emit prefixed field newtypes so the resulting product is
    // composed of distinct types (Canon's "alphabetical-distinct" rule).
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

    // Function signatures. A `result` return takes the new string-anchored
    // constructor form (the `ok` payload is minted, so `?` still sees a
    // `Result`): `paint -> result<style, string>` mints `Paint = Style`.
    assert!(f.content.contains("Paint = Style"));
    assert!(f
        .content
        .contains("(ColorList * Shape) => Result<Paint, String> {\n    \"paint\"\n}"));
    // `option` and no-result functions take the string-anchored form
    // like every other shape: the mint aliases the whole rendered type
    // (`Centre = Option<Float>`) or `Unit` for a pure effect.
    assert!(f.content.contains("Centre = Option<Float>"));
    assert!(f.content.contains("Shape => Centre {\n    \"centre\"\n}"));
    assert!(f.content.contains("Reset = Unit"));
    assert!(f.content.contains("Unit => Reset {\n    \"reset\"\n}"));

    parse_canon(&f.content);
}

#[test]
fn wasi_scalar_payload_returns_are_emitted() {
    // In the `wasi:` namespace codegen reads exact widths from the
    // vendored WIT, so both wide and narrow scalar list/option returns
    // are emitted; compound payloads stay skipped.
    let path = Path::new("tests/fixtures/wit/payloads.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];

    // Narrow list element (`list<u8>`) — the `get-random-bytes` shape.
    assert!(f.content.contains("ReadBytes = List<Int>"));
    assert!(f
        .content
        .contains("Int => ReadBytes {\n    \"read-bytes\"\n}"));
    // Wide list element.
    assert!(f.content.contains("ReadWords = List<Int>"));
    // Scalar option payloads, wide and narrow.
    assert!(f.content.contains("PeekValue = Option<Int>"));
    assert!(f.content.contains("PeekFlag = Option<Int>"));

    // Compound payloads are skipped with a reason naming the gap.
    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("readPairs") && s.contains("compound payload")),
        "list<record> return should be skipped: {:?}",
        f.skipped
    );
    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("peekPair") && s.contains("compound payload")),
        "option<record> return should be skipped: {:?}",
        f.skipped
    );

    // Bare `result` returns are emitted (the mint aliases the absent ok
    // payload as `Unit`, so the constructor still returns a `Result`);
    // bare `result` parameters stay skipped.
    assert!(f.content.contains("SyncAll = Unit"));
    assert!(f
        .content
        .contains("Unit => Result<SyncAll, Unit> {\n    \"sync-all\"\n}"));
    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("abortWith") && s.contains("bare `result` parameter")),
        "bare-result parameter should be skipped: {:?}",
        f.skipped
    );

    parse_canon(&f.content);
}

#[test]
fn narrow_payloads_outside_wasi_are_skipped() {
    // Outside `wasi:` there is no vendored WIT to read narrow widths
    // from — the decode stride would be a guess — so narrow scalar
    // payloads are skipped while wide ones are emitted.
    let path = Path::new("tests/fixtures/wit/narrow-payloads.wit");
    let files = bindgen::generate_from_path(path).expect("bindgen should succeed");
    assert_eq!(files.len(), 1);
    let f = &files[0];

    assert!(f.content.contains("ReadWords = List<Int>"));
    assert!(f.content.contains("PeekValue = Option<Int>"));

    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("readBytes") && s.contains("width unknowable")),
        "narrow list element outside wasi should be skipped: {:?}",
        f.skipped
    );
    assert!(
        f.skipped
            .iter()
            .any(|s| s.contains("peekFlag") && s.contains("width unknowable")),
        "narrow option payload outside wasi should be skipped: {:?}",
        f.skipped
    );

    parse_canon(&f.content);
}
