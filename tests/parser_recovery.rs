//! Tests for the parser's error-recovery strategy. A single bad token
//! no longer aborts the whole parse: `Parser::parse_recover`
//! resynchronizes at the next item / block / arm boundary and keeps
//! going, so one file can report many syntax errors at once (the win
//! that lets the LSP and `canon check` show more than one error per
//! file). These can't be expressed as a `.can` fixture because they
//! assert on the *set* of errors, not a single golden `.stderr`.

use canon::ast::{Expr, Item};
use canon::lexer::Scanner;
use canon::parser::Parser;

fn recover(src: &str) -> (canon::ast::Module, Vec<canon::error::CanonError>) {
    let tokens = Scanner::new(src).scan_tokens().expect("lexer failed");
    Parser::new(tokens).parse_recover()
}

/// Two independent broken top-level definitions each surface their own
/// error, and the well-formed definition between them still parses.
#[test]
fn recovers_across_top_level_items() {
    let src = "Foo = *\n\nBar = String\n\nBaz = +\n";
    let (module, errors) = recover(src);
    assert_eq!(errors.len(), 2, "expected one error per broken definition");
    // The good definition in the middle survived recovery.
    let names: Vec<&str> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::TypeDef(td) => Some(td.name.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        names.contains(&"Bar"),
        "the well-formed definition should still be parsed, got {names:?}"
    );
}

/// A broken dispatch arm reports one error, and the sibling arms on
/// either side of it are still recovered.
#[test]
fn recovers_across_dispatch_arms() {
    let src = "Unit => Program {\n    \
               True() -> (\n        \
               * False => Unit { \"a\" -> Print }\n        \
               * 1 2 3 => Unit { \"b\" -> Print }\n        \
               * True => Unit { \"c\" -> Print }\n    )\n}\n";
    let (module, errors) = recover(src);
    assert_eq!(errors.len(), 1, "expected exactly one arm error");

    let arm_count = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some(f),
            _ => None,
        })
        .flat_map(|f| &f.body.exprs)
        .find_map(|e| match e {
            Expr::Match { arms, .. } => Some(arms.len()),
            _ => None,
        });
    assert_eq!(
        arm_count,
        Some(2),
        "the two well-formed arms should survive recovery"
    );
}

/// Recovery is inert on a clean parse: no spurious errors, and the
/// module comes back intact.
#[test]
fn clean_source_yields_no_errors() {
    let (module, errors) = recover("Bool = False + True\n");
    assert!(errors.is_empty(), "clean source produced {errors:?}");
    assert_eq!(module.items.len(), 1);
}

/// The single-error `parse` wrapper stays backwards compatible: it
/// returns the *first* error encountered, exactly as before recovery
/// existed.
#[test]
fn parse_returns_first_error() {
    let src = "Foo = *\n\nBaz = +\n";
    let tokens = Scanner::new(src).scan_tokens().expect("lexer failed");
    let err = Parser::new(tokens).parse().expect_err("should be an error");
    assert_eq!(err.span().line, 1, "first error should be the earliest one");
}
