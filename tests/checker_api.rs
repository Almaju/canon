//! Tests that probe the *checker's API surface* directly — things
//! that can't be expressed as a single `.can` fixture because they
//! involve calling internal entry points with synthetic arguments.
//!
//! Positive and negative tests of plain source code belong in
//! `tests/fixtures/checker/{ok,fail}/` and are exercised by
//! `tests/checker_fixtures.rs`. Reach for this file only when the
//! test needs to feed the checker something the loader wouldn't
//! produce on its own.

use canon::ast::resolve_new_syntax;
use canon::checker;
use canon::checker::check_with_entry;
use canon::lexer::Scanner;
use canon::parser::Parser;

fn parse(source: &str) -> canon::ast::Module {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse().expect("parser failed");
    resolve_new_syntax(&mut module);
    module
}

/// Regression test for issue 4: the sort-order check must only apply
/// to items declared in the *entry file*. Items pulled in via `use`
/// follow their own file's ordering and shouldn't constrain the
/// entry file's local ordering.
///
/// `entry_items_start` is the boundary index passed to
/// `check_with_entry` — items at indices `[0..start)` are treated as
/// "imported" and `[start..)` are checked for ordering. Here we
/// build a module where the first two items pretend to be imported
/// (a typedef and a method on it) and verify that a local method on
/// the *same* receiver type declared later isn't compared against
/// the imported one for alphabetical order.
#[test]
fn method_ordering_only_within_entry_file() {
    let source = r#"
HttpRequest = String

path = (HttpRequest) -> String {
    HttpRequest
}

chatHandler = (HttpRequest) -> String {
    HttpRequest
}

main = () -> Unit {
    "ok".print()
}
"#;
    let module = parse(source);

    // entry_items_start = 2: items[0..2] (HttpRequest typedef + path method)
    // are treated as "imported"; only items[2..] participate in ordering.
    let errors = check_with_entry(&module, 2);
    let ordering_errs: Vec<_> = errors
        .iter()
        .filter(|e| {
            let msg = format!("{:?}", e);
            msg.contains("alphabetical order") && msg.contains("chatHandler")
        })
        .collect();
    assert!(
        ordering_errs.is_empty(),
        "spurious ordering error for a local method that only precedes an imported method: {:?}",
        ordering_errs
    );
}

/// Sanity check: `check` (zero offset) and `check_with_entry(_, 0)`
/// are observationally identical on well-formed input.
#[test]
fn check_and_check_with_entry_zero_agree() {
    let source = r#"
main = () -> Unit {
    "hi".print()
}
"#;
    let module = parse(source);
    let a = checker::check(&module);
    let b = check_with_entry(&module, 0);
    assert_eq!(format!("{:?}", a), format!("{:?}", b));
}
