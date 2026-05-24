//! End-to-end tests for `oneway gen-bindings`.
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
    assert!(f
        .content
        .contains("extern Wasm(\"wasi:clocks/monotonic-clock@"));
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

    // Function signatures.
    assert!(f.content.contains("centre = (Shape) -> Option<Point>"));
    assert!(f
        .content
        .contains("paint = (ColorList * Shape) -> Result<Style, String>"));
    assert!(f.content.contains("reset = () -> Unit"));

    parse_oneway(&f.content);
}
