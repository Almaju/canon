//! Lowering of `JsonLit` / `HtmlLit` AST nodes into ordinary `Expr`s.
//!
//! Both JSON and HTML literals desugar to left-associative `String.concat`
//! chains over `StringLit` (Static parts) and `.ToJson()` / `.ToHtml()`
//! method calls (Interp parts). The resulting `Expr` is a normal expression
//! the codegen can compile via its existing machinery — no literal-specific
//! instructions to lower below this point.
use crate::ast::{Expr, HtmlLitPart, Ident, JsonLitPart};

/// Lower a `JsonLit { parts }` into the equivalent left-associative
/// `String.concat` chain over `StringLit` (Static parts) and `.ToJson()`
/// method calls (Interp parts).
///
/// Example: `{"k": foo}` (parts = [Static(`{"k":`), Interp(foo), Static(`}`)])
///
///   → `"{\"k\":".concat(foo.ToJson()).concat("}")`
pub(super) fn json_lit_to_concat_chain(parts: &[JsonLitPart], span: crate::error::Span) -> Expr {
    let part_exprs: Vec<Expr> = parts
        .iter()
        .map(|p| match p {
            JsonLitPart::Static(s) => Expr::StringLit {
                value: s.clone(),
                span,
            },
            JsonLitPart::Interp(e) => Expr::MethodCall {
                receiver: e.clone(),
                method: Ident {
                    name: "ToJson".to_string(),
                    span,
                },
                args: vec![],
                piped: false,
                span,
            },
        })
        .collect();

    let mut iter = part_exprs.into_iter();
    // Parser invariant: parts is never empty (always starts with the
    // opening `{` or `[` as a Static).
    let mut acc = iter.next().expect("JsonLit parts must be non-empty");
    for next in iter {
        acc = Expr::MethodCall {
            receiver: Box::new(acc),
            method: Ident {
                name: "concat".to_string(),
                span,
            },
            args: vec![next],
            piped: false,
            span,
        };
    }
    acc
}

/// Lower an `HtmlLit { parts }` into the equivalent left-associative
/// `String.concat` chain over `StringLit` (Static parts) and
/// `.ToHtml()` method calls (Interp parts) — the exact HTML analogue of
/// `json_lit_to_concat_chain` above. `ToHtml` dispatches on the
/// interpolated value's type: `String` and `Int` escape through the
/// stdlib's `text()`, `Html` passes through unchanged.
///
/// Example: `<li>{name}</li>` (parts = [Static(`<li>`), Interp(name),
/// Static(`</li>`)])
///
///   → `"<li>".concat(name.ToHtml()).concat("</li>")`
pub(super) fn html_lit_to_concat_chain(parts: &[HtmlLitPart], span: crate::error::Span) -> Expr {
    let part_exprs: Vec<Expr> = parts
        .iter()
        .map(|p| match p {
            HtmlLitPart::Static(s) => Expr::StringLit {
                value: s.clone(),
                span,
            },
            HtmlLitPart::Interp(e) => Expr::MethodCall {
                receiver: e.clone(),
                method: Ident {
                    name: "ToHtml".to_string(),
                    span,
                },
                args: vec![],
                piped: false,
                span,
            },
        })
        .collect();

    let mut iter = part_exprs.into_iter();
    // Parser invariant: parts is never empty (the literal's opening
    // tag is always a Static).
    let mut acc = iter.next().expect("HtmlLit parts must be non-empty");
    for next in iter {
        acc = Expr::MethodCall {
            receiver: Box::new(acc),
            method: Ident {
                name: "concat".to_string(),
                span,
            },
            args: vec![next],
            piped: false,
            span,
        };
    }
    acc
}
