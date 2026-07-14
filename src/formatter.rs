//! Canon source code formatter.
//!
//! Parses an `.can` source file and emits it in canonical format.
//! The formatter enforces "one way" to write Canon code — consistent
//! spacing, indentation, and line breaking.

use crate::ast::*;
use crate::error::{CanonError, Result, Span};
use crate::lexer::Scanner;
use crate::parser::Parser;

const MAX_WIDTH: usize = 100;

/// Format a Canon source string, returning the canonically formatted version.
pub fn format(source: &str) -> Result<String> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let module = parser.parse()?;
    let module = canonicalize_module(&module);
    Ok(emit_module(&module))
}

/// Formatting is a compiler phase: a divergence from canonical form is
/// a `FormatError`, the same standing as a lex, parse, or check error.
/// The canonical form is *defined* by this module's emitter, so the
/// phase is the emitter run against the written source — there is no
/// second rulebook to drift from it. Returns the error pointing at the
/// first place `source` diverges from its canonical form, or `None`
/// when the source is already canonical. A source that fails to parse
/// also returns `None`: the pipeline owns the better-located parse
/// diagnostic. `path` names the offending file in multi-file loads.
pub fn format_error(source: &str, path: &str) -> Option<CanonError> {
    let canonical = format(source).ok()?;
    if canonical == source {
        return None;
    }
    Some(CanonError::FormatError {
        message: "not canonically formatted: run `canon check --fix`".to_string(),
        path: path.to_string(),
        span: divergence_span(source, &canonical),
    })
}

/// Span of the first divergence between `source` and its `canonical`
/// form: the first differing line, from the first differing character
/// to the end of that line. When one text is a prefix of the other the
/// span sits at `source`'s end (canonical has more lines) or on the
/// first surplus source line.
fn divergence_span(source: &str, canonical: &str) -> Span {
    let mut offset = 0usize;
    let mut line_no = 1u32;
    let mut src_lines = source.split_inclusive('\n');
    let mut canon_lines = canonical.split_inclusive('\n');
    loop {
        match (src_lines.next(), canon_lines.next()) {
            (Some(s), Some(c)) if s == c => {
                offset += s.len();
                line_no += 1;
            }
            (Some(s), Some(c)) => {
                let mut column = 1u32;
                let mut byte = 0usize;
                for ((i, sc), cc) in s.char_indices().zip(c.chars()) {
                    if sc != cc {
                        byte = i;
                        break;
                    }
                    column += 1;
                    byte = i + sc.len_utf8();
                }
                let line_end = offset + s.strip_suffix('\n').unwrap_or(s).len();
                return Span::new(offset + byte, line_end.max(offset + byte), line_no, column);
            }
            (Some(s), None) => {
                let line_end = offset + s.strip_suffix('\n').unwrap_or(s).len();
                return Span::new(offset, line_end, line_no, 1);
            }
            (None, _) => return Span::new(offset, offset, line_no, 1),
        }
    }
}

// ── Canonical call form ───────────────────────────────────────────────────────
//
// One way to spell a call: the first input always rides the pipe, the
// rest ride in the parens as a partial application. `B(A)` is rewritten
// to `A -> B`, `B(A * C)` to `A -> B(C)`, and `A.B(C)` / `A * C -> B` to
// `A -> B(C)`. `A -> B(C)` reads as "apply `B`, which already has `C`,
// to `A`". Zero-input calls stay prefix (`Now()`, `Map()`), and
// `List(…)` keeps its elements (an ordered sequence, not a
// subject-bearing call). The parser accepts every spelling; this pass is
// what makes `canon check --fix` pick the canonical one. The compiler treats a
// piped call to a type constructor as construction (`A -> B(rest)` ≡
// `B(A * rest)`), so the rewrite is semantics-preserving.

fn canonicalize_module(m: &Module) -> Module {
    Module {
        items: m
            .items
            .iter()
            .map(|it| match it {
                Item::Function(f) => Item::Function(FunctionDef {
                    body: canon_block(&f.body),
                    anonymous: f.anonymous || is_redundantly_named(f),
                    ..f.clone()
                }),
                other => other.clone(),
            })
            .collect(),
        span: m.span,
    }
}

/// A named bodied declaration whose name is exactly the type its return
/// constructs (`Url = (String) => Result<Url, InvalidUrl> { … }`) spells
/// the name twice — the signature already carries it. The anonymous
/// arrow is the one declaration form for constructors, so `canon check --fix`
/// drops the redundant name (`String => Result<Url, InvalidUrl> { … }`).
/// A name that differs from the constructed type is a checker error, so
/// the arrow is the only bodied form that survives canonical format.
fn is_redundantly_named(f: &FunctionDef) -> bool {
    !f.anonymous
        && f.receiver.is_none()
        && f.generic_params.is_empty()
        && crate::ast::constructed_type_name(&f.return_ty).as_deref() == Some(f.name.name.as_str())
}

fn canon_block(b: &Block) -> Block {
    Block {
        exprs: b.exprs.iter().map(canon_expr).collect(),
        span: b.span,
    }
}

fn canon_arm(a: &MatchArm) -> MatchArm {
    MatchArm {
        body: canon_block(&a.body),
        ..a.clone()
    }
}

/// Flatten a call's argument list to its input factors: a single
/// `ProductValue` argument (`B(a * b)`) becomes `[a, b]`; anything else
/// is taken as written.
fn flatten_inputs(args: &[Expr]) -> Vec<Expr> {
    match args {
        [Expr::ProductValue { fields, .. }] => fields.clone(),
        _ => args.to_vec(),
    }
}

/// A receiver splits into its subject (the piped value, `.1`) and any
/// trailing factors. A product receiver (`A * C -> B`) contributes its
/// first element as the subject and the rest as parens factors;
/// position is preserved so a same-typed builtin (`Difference`) keeps
/// its operand order.
fn split_receiver(recv: Expr) -> (Expr, Vec<Expr>) {
    match recv {
        Expr::ProductValue { fields, .. } if fields.len() >= 2 => {
            let mut it = fields.into_iter();
            let subject = it.next().unwrap();
            (subject, it.collect())
        }
        other => (other, Vec::new()),
    }
}

/// Build the canonical pipe `subject -> name(rest…)`. With no trailing
/// factors it is the bare `subject -> name`; with several it wraps them
/// in a product.
fn make_pipe(subject: Expr, name: Ident, rest: Vec<Expr>, span: Span) -> Expr {
    let args = match rest.len() {
        0 => Vec::new(),
        1 => rest,
        _ => vec![Expr::ProductValue { fields: rest, span }],
    };
    Expr::MethodCall {
        receiver: Box::new(subject),
        method: name,
        args,
        piped: true,
        span,
    }
}

/// Build the canonical prefix call `Name(a * b * …)` with operand order
/// preserved.
fn prefix_call(name: Ident, inputs: Vec<Expr>, span: Span) -> Expr {
    let args = match inputs.len() {
        0 | 1 => inputs,
        _ => vec![Expr::ProductValue {
            fields: inputs,
            span,
        }],
    };
    Expr::Constructor { name, args, span }
}

/// Scalar literals are born inside the call parens — they never pipe.
/// The pipe carries a value that already exists (a parameter, a prior
/// result); a literal springs into existence at the call site, so it
/// rides in the parens: `Greeting("hi")`, `Sum(1 * 2)`,
/// `Print("hello")`. Structured values (`List(…)`, JSON/HTML literals)
/// and every computed expression flow with `->`.
fn is_scalar_literal(e: &Expr) -> bool {
    matches!(
        e,
        Expr::StringLit { .. }
            | Expr::IntLit { .. }
            | Expr::FloatLit { .. }
            | Expr::FormatLit { .. }
    )
}

/// `String("s")`, `Int(3)`, `Float(1.5)` wrap a literal in
/// the constructor that literal already desugars to — the wrap is pure
/// ceremony, so `canon check --fix` unwraps it to the bare literal. Cross-kind
/// construction (`String(42)` decimal rendering, `Int("42")` parsing)
/// is a real conversion and stays.
fn primitive_literal_wrap(name: &str, input: &Expr) -> bool {
    matches!(
        (name, input),
        ("String", Expr::StringLit { .. })
            | ("Int", Expr::IntLit { .. })
            | ("Float", Expr::FloatLit { .. })
    )
}

/// Fold a `Joined` chain into a format string when literal text anchors
/// it. Returns `None` when no direct segment is a string literal or
/// format string (the chain may be list concatenation — only literal
/// text proves strings) or when a bare numeric literal appears as a
/// segment (ill-typed as concatenation; folding would change the
/// failure mode). An all-static fold collapses to the plain string, the
/// same constant-folding the parser applies to literal holes.
fn fold_joined_chain(chain: &Expr, span: crate::error::Span) -> Option<Expr> {
    let mut parts: Vec<FormatLitPart> = Vec::new();
    let mut saw_text = false;
    if !joined_parts(chain, &mut parts, &mut saw_text) || !saw_text {
        return None;
    }
    // Merge adjacent statics so the parts alternate the way the parser
    // produces them (round-trip stability).
    let mut merged: Vec<FormatLitPart> = Vec::with_capacity(parts.len());
    for p in parts {
        match (&p, merged.last_mut()) {
            (FormatLitPart::Static(s), Some(FormatLitPart::Static(last))) => last.push_str(s),
            _ => merged.push(p),
        }
    }
    if merged.iter().all(|p| matches!(p, FormatLitPart::Static(_))) {
        let value = merged
            .iter()
            .map(|p| match p {
                FormatLitPart::Static(s) => s.as_str(),
                FormatLitPart::Interp(_) => unreachable!(),
            })
            .collect::<String>();
        return Some(Expr::StringLit { value, span });
    }
    Some(Expr::FormatLit {
        parts: merged,
        span,
    })
}

/// Flatten a `Joined` tree (receiver chains and `Joined` arguments —
/// concatenation is associative) into format-string parts. `Static`
/// parts come only from direct string-literal / format-string segments;
/// every other segment becomes an interpolation hole. Returns `false`
/// (abort the fold) on a bare numeric-literal segment.
fn joined_parts(expr: &Expr, parts: &mut Vec<FormatLitPart>, saw_text: &mut bool) -> bool {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } if crate::ast::builtin_pipe_name(&method.name) == "Joined" && args.len() == 1 => {
            joined_parts(receiver, parts, saw_text) && joined_parts(&args[0], parts, saw_text)
        }
        Expr::StringLit { value, .. } => {
            *saw_text = true;
            parts.push(FormatLitPart::Static(value.clone()));
            true
        }
        Expr::FormatLit { parts: inner, .. } => {
            *saw_text = true;
            for p in inner {
                parts.push(match p {
                    FormatLitPart::Static(s) => FormatLitPart::Static(s.clone()),
                    FormatLitPart::Interp(e) => FormatLitPart::Interp(Box::new(canon_expr(e))),
                });
            }
            true
        }
        Expr::IntLit { .. } | Expr::FloatLit { .. } => false,
        other => {
            parts.push(FormatLitPart::Interp(Box::new(canon_expr(other))));
            true
        }
    }
}

fn canon_expr(e: &Expr) -> Expr {
    match e {
        Expr::Ident(_) | Expr::StringLit { .. } | Expr::IntLit { .. } | Expr::FloatLit { .. } => {
            e.clone()
        }

        Expr::FieldAccess {
            receiver,
            field,
            span,
        } => Expr::FieldAccess {
            receiver: Box::new(canon_expr(receiver)),
            field: field.clone(),
            span: *span,
        },

        Expr::Try { inner, span } => Expr::Try {
            inner: Box::new(canon_expr(inner)),
            span: *span,
        },

        Expr::Await { inner, span } => Expr::Await {
            inner: Box::new(canon_expr(inner)),
            span: *span,
        },

        Expr::Match {
            scrutinee,
            arms,
            span,
        } => Expr::Match {
            scrutinee: Box::new(canon_expr(scrutinee)),
            arms: arms.iter().map(canon_arm).collect(),
            span: *span,
        },

        Expr::Lambda {
            params,
            return_ty,
            body,
            span,
        } => Expr::Lambda {
            params: params.clone(),
            return_ty: return_ty.clone(),
            body: canon_block(body),
            span: *span,
        },

        Expr::ProductValue { fields, span } => Expr::ProductValue {
            fields: fields.iter().map(canon_expr).collect(),
            span: *span,
        },

        Expr::JsonLit { parts, span } => Expr::JsonLit {
            parts: parts
                .iter()
                .map(|p| match p {
                    JsonLitPart::Static(s) => JsonLitPart::Static(s.clone()),
                    JsonLitPart::Interp(e) => JsonLitPart::Interp(Box::new(canon_expr(e))),
                })
                .collect(),
            span: *span,
        },

        Expr::HtmlLit { parts, span } => Expr::HtmlLit {
            parts: parts
                .iter()
                .map(|p| match p {
                    HtmlLitPart::Static(s) => HtmlLitPart::Static(s.clone()),
                    HtmlLitPart::Interp(e) => HtmlLitPart::Interp(Box::new(canon_expr(e))),
                })
                .collect(),
            span: *span,
        },

        Expr::FormatLit { parts, span } => Expr::FormatLit {
            parts: parts
                .iter()
                .map(|p| match p {
                    FormatLitPart::Static(s) => FormatLitPart::Static(s.clone()),
                    FormatLitPart::Interp(e) => FormatLitPart::Interp(Box::new(canon_expr(e))),
                })
                .collect(),
            span: *span,
        },

        // ── Prefix constructor: `B(inputs…)` ────────────────────────────
        Expr::Constructor { name, args, span } => {
            // `List(…)` is an ordered sequence literal, not a
            // subject-bearing call — keep it prefix, elements in order.
            if name.name == "List" {
                return Expr::Constructor {
                    name: name.clone(),
                    args: args.iter().map(canon_expr).collect(),
                    span: *span,
                };
            }
            let mut inputs: Vec<Expr> = flatten_inputs(args).iter().map(canon_expr).collect();
            if inputs.is_empty() {
                // Zero-input call stays prefix: `Now()`, `Map()`, `None()`.
                return Expr::Constructor {
                    name: name.clone(),
                    args: Vec::new(),
                    span: *span,
                };
            }
            if inputs.len() == 1 && primitive_literal_wrap(&name.name, &inputs[0]) {
                return inputs.pop().unwrap();
            }
            if inputs.len() == 1
                && is_scalar_literal(&inputs[0])
                && !crate::ast::is_builtin_pipe_vocabulary(&name.name)
            {
                // Single literal input: the literal is born in the
                // parens, so the call stays prefix (`Greeting("hi")`).
                // Builtins (`Sum`, `Print`, …) are receiver-oriented
                // machine operations with no prefix form; they keep the
                // pipe until they migrate to stdlib newtypes. Multi-input
                // calls keep the pipe too: the piped call binds its
                // components commutatively, while a prefix argument list
                // is positional.
                return prefix_call(name.clone(), inputs, *span);
            }
            if inputs.iter().any(is_scalar_literal) {
                // Literal inputs present: order carries operand
                // positions — pipe the first, keep the rest as written.
                let subject = inputs.remove(0);
                return make_pipe(subject, name.clone(), inputs, *span);
            }
            // All-computed inputs bind by type (distinct field types —
            // the product rule), so the order is free: sort for
            // determinism, pipe the first, parens hold the rest.
            inputs.sort_by_key(emit_inline);
            let subject = inputs.remove(0);
            make_pipe(subject, name.clone(), inputs, *span)
        }

        // ── Method / pipe: `recv.B(args…)` / `recv -> B(args…)` ──────────
        Expr::MethodCall {
            receiver,
            method,
            args,
            piped,
            span,
        } => {
            // A `Joined` chain anchored by literal text is a hand-written
            // format string — fold it into the backtick literal, the one
            // spelling of building a string around text (`"<" -> Joined(x)
            // -> Joined(">")` becomes `` `<{x}>` ``). A chain with no
            // direct string-literal segment stays a pipe: `Joined` is also
            // list concatenation, and only literal text proves the chain
            // builds a string.
            if crate::ast::builtin_pipe_name(&method.name) == "Joined" && args.len() == 1 {
                if let Some(folded) = fold_joined_chain(e, *span) {
                    return folded;
                }
            }
            // camelCase methods are FFI binding calls at the boundary
            // (`.now()`, `.set()`); `->` only pipes into PascalCase
            // constructors, so leave these as dot-calls.
            if !method.name.chars().next().is_some_and(char::is_uppercase) {
                return Expr::MethodCall {
                    receiver: Box::new(canon_expr(receiver)),
                    method: method.clone(),
                    args: args.iter().map(canon_expr).collect(),
                    piped: *piped,
                    span: *span,
                };
            }
            let (subject, mut rest) = split_receiver(canon_expr(receiver));
            for input in flatten_inputs(args) {
                rest.push(canon_expr(&input));
            }
            if is_scalar_literal(&subject)
                && rest.is_empty()
                && !crate::ast::is_builtin_pipe_vocabulary(&method.name)
            {
                // A lone literal never pipes into a construction —
                // collapse to the prefix call: `"hi" -> Greeting`
                // becomes `Greeting("hi")`. Builtins (`Sum`, `Print`, …)
                // are receiver-oriented machine operations with no
                // prefix form, and multi-input calls bind their
                // components commutatively through the pipe — both keep
                // the arrow.
                if primitive_literal_wrap(&method.name, &subject) {
                    return subject;
                }
                return prefix_call(method.clone(), vec![subject], *span);
            }
            make_pipe(subject, method.clone(), rest, *span)
        }
    }
}

// ── Module ──────────────────────────────────────────────────────────────────

fn emit_module(module: &Module) -> String {
    let mut sections: Vec<String> = Vec::new();

    // All items, each separated by a blank line, in canonical
    // declaration order (see `sort_items`).
    let others: Vec<&Item> = module.items.iter().collect();
    for item in sort_items(&others) {
        sections.push(emit_item(item));
    }

    let mut out = sections.join("\n\n");
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Canonical top-level declaration order: type definitions sort
/// alphabetically among the type-definition slots and functions sort
/// alphabetically among the function slots (the same subsequence rule
/// the checker enforces, so sorted output always passes
/// `check_ordering`). Two functions are exempt and keep their
/// position, mirroring the checker's exemptions: `main` and an HTTP
/// entry (a free function returning `Response` / `Result<Response,
/// _>`).
fn sort_items<'a>(items: &[&'a Item]) -> Vec<&'a Item> {
    let mut out: Vec<&'a Item> = Vec::with_capacity(items.len());
    let mut segment: Vec<&'a Item> = items.to_vec();
    flush_sorted_segment(&mut segment, &mut out);
    out
}

fn flush_sorted_segment<'a>(segment: &mut Vec<&'a Item>, out: &mut Vec<&'a Item>) {
    let mut type_defs: Vec<&'a Item> = segment
        .iter()
        .copied()
        .filter(|i| matches!(i, Item::TypeDef(_)))
        .collect();
    type_defs.sort_by_key(|i| match i {
        Item::TypeDef(td) => td.name.name.clone(),
        _ => String::new(),
    });
    let mut funcs: Vec<&'a Item> = segment
        .iter()
        .copied()
        .filter(|i| matches!(i, Item::Function(f) if !is_pinned_entry(f)))
        .collect();
    funcs.sort_by_key(|i| match i {
        Item::Function(f) => (
            f.name.name.clone(),
            f.receiver
                .as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    });
    let mut ti = 0;
    let mut fi = 0;
    for item in segment.drain(..) {
        match item {
            Item::TypeDef(_) => {
                out.push(type_defs[ti]);
                ti += 1;
            }
            Item::Function(f) if !is_pinned_entry(f) => {
                out.push(funcs[fi]);
                fi += 1;
            }
            other => out.push(other),
        }
    }
}

/// Mirrors the checker's ordering exemptions (`check_ordering`): the
/// entry point is a distinguished role, not a regular free function,
/// so the formatter leaves it wherever the author put it.
fn is_pinned_entry(f: &FunctionDef) -> bool {
    if f.receiver.is_some() {
        return false;
    }
    f.name.name == "main"
        || entry_world_of(&f.return_ty) == Some(EntryWorld::Http)
        || (f.anonymous && entry_world_of(&f.return_ty) == Some(EntryWorld::Cli))
}

fn emit_item(item: &Item) -> String {
    match item {
        Item::TypeDef(td) => emit_type_def(td),
        Item::Function(f) => emit_function(f),
    }
}

// ── Type Definitions ────────────────────────────────────────────────────────

fn emit_type_def(td: &TypeDef) -> String {
    let g = emit_generic_params(&td.generic_params);
    let header = format!("{}{} = ", td.name.name, g);

    // Union types: break one variant per line when there are 3+ variants,
    // or when the single-line form would exceed MAX_WIDTH.
    if let TypeExpr::Union { variants, .. } = &td.body {
        let inline = emit_type_expr(&td.body);
        let full = format!("{}{}", header, inline);
        if variants.len() >= 3 || full.len() > MAX_WIDTH {
            let first = emit_type_expr(&variants[0]);
            let rest = variants[1..]
                .iter()
                .map(|v| format!("  + {}", emit_type_expr(v)))
                .collect::<Vec<_>>()
                .join("\n");
            return format!("{}{}\n{}", header, first, rest);
        }
    }

    // Product types: break one field per line only when the single-line
    // form would exceed MAX_WIDTH. Unlike unions we don't break on field
    // count alone — short products like `Ipv4Address = Int * Int * Int * Int`
    // read better on one line.
    if let TypeExpr::Product { fields, .. } = &td.body {
        let inline = emit_type_expr(&td.body);
        let full = format!("{}{}", header, inline);
        if full.len() > MAX_WIDTH && fields.len() >= 2 {
            let first = emit_type_in_product(&fields[0]);
            let rest = fields[1..]
                .iter()
                .map(|f| format!("  * {}", emit_type_in_product(f)))
                .collect::<Vec<_>>()
                .join("\n");
            return format!("{}{}\n{}", header, first, rest);
        }
    }

    format!("{}{}", header, emit_type_expr(&td.body))
}

// ── Function Definitions ────────────────────────────────────────────────────

fn emit_function(func: &FunctionDef) -> String {
    let mut out = String::new();

    // An anonymous constructor drops every parenthesis around its input:
    // `Request => Response`, the product `Todos * String => Update`, and
    // the nullary `Unit => Program` (the single-value type is the name of
    // "no input"). The declaration arrow `=>` binds looser than the type
    // operators, so the input reads back unambiguously — the one case
    // that still needs parens is a compound input whose *first* component
    // is generic (`(List<Int> * B) => C`), since a leading `<` steers the
    // parser to generic-params instead. Named function declarations
    // (`name = (params) => R`) keep their `()`.
    let anon_input = if func.anonymous && func.generic_params.is_empty() {
        anon_input_paren_free(&func.params)
    } else {
        None
    };
    if let Some(input) = anon_input {
        out.push_str(&input);
        out.push_str(" => ");
        out.push_str(&emit_type_expr(&func.return_ty));
    } else {
        if !func.anonymous {
            out.push_str(&func.name.name);
            out.push_str(&emit_generic_params(&func.generic_params));
            out.push_str(" = (");
        } else {
            out.push_str(&emit_generic_params(&func.generic_params));
            out.push('(');
        }
        out.push_str(&emit_fn_params(func));
        out.push_str(") => ");
        out.push_str(&emit_type_expr(&func.return_ty));
    }

    // Body. Functions whose body was synthesized by the loader (i.e.
    // the `extern_wasm` field is populated because they came from a
    // vendored binding file) get no body emitted — the source they
    // came from already used the bodyless `name = (P) -> R` form. The
    // loader recreates that shape on every parse, so the formatter
    // just needs to mirror it.
    if func.extern_wasm.is_none() {
        out.push_str(" {\n");
        for expr in &func.body.exprs {
            out.push_str("    ");
            out.push_str(&emit_expr(expr, 1));
            out.push('\n');
        }
        out.push('}');
    }

    out
}

/// The paren-free input rendering of an anonymous constructor, or `None`
/// when it must keep its parentheses. Zero params render as `Unit` (the
/// single-value "no input" type). A single param renders bare
/// — `Request`, or the product `Todos * String` — as long as its first
/// atom is a generics-free named type, since only then does the parser's
/// paren-free path (a leading bare ident, then `=>`/`*`/`+`) round-trip.
/// A leading generic (`List<Int> * B`) or the multi-param comma form
/// keeps parens.
fn anon_input_paren_free(params: &[Param]) -> Option<String> {
    match params {
        [] => Some("Unit".to_string()),
        [p] if first_atom_is_bare_named(&p.ty) => Some(emit_type_expr(&p.ty)),
        _ => None,
    }
}

/// Whether a type's leading atom is a named type with no generic args —
/// the shape the parser can re-read without parentheses.
fn first_atom_is_bare_named(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Named { generics, .. } => generics.is_empty(),
        TypeExpr::Product { fields, .. } => fields.first().is_some_and(first_atom_is_bare_named),
        TypeExpr::Union { variants, .. } => variants.first().is_some_and(first_atom_is_bare_named),
        _ => false,
    }
}

fn emit_fn_params(func: &FunctionDef) -> String {
    let first_char = func.name.name.chars().next().unwrap_or('a');
    let is_pascal = first_char.is_uppercase();
    let is_main = func.name.name == "main";

    if is_pascal || is_main {
        // Params stored as-is (no receiver extraction happened).
        emit_param_list(&func.params)
    } else if let Some(recv) = &func.receiver {
        // camelCase: receiver was extracted from the first product component.
        // Reconstruct the original product: Receiver * Param1 * Param2 …
        if func.params.is_empty() {
            recv.name.clone()
        } else {
            let mut parts = vec![recv.name.clone()];
            for p in &func.params {
                parts.push(emit_type_expr(&p.ty));
            }
            parts.join(" * ")
        }
    } else {
        emit_param_list(&func.params)
    }
}

fn emit_param_list(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| emit_type_expr(&p.ty))
        .collect::<Vec<_>>()
        .join(", ")
}

fn emit_generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match &p.bound {
            Some(b) => format!("{}: {}", p.name.name, emit_type_expr(b)),
            None => p.name.name.clone(),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

// ── Type Expressions ────────────────────────────────────────────────────────

fn emit_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if generics.is_empty() {
                name.clone()
            } else {
                let gs: Vec<String> = generics.iter().map(emit_type_expr).collect();
                format!("{}<{}>", name, gs.join(", "))
            }
        }
        TypeExpr::Union { variants, .. } => variants
            .iter()
            .map(emit_type_expr)
            .collect::<Vec<_>>()
            .join(" + "),
        TypeExpr::Product { fields, .. } => fields
            .iter()
            .map(emit_type_in_product)
            .collect::<Vec<_>>()
            .join(" * "),
        TypeExpr::Repeat { ty, count, .. } => {
            format!("{}^{}", emit_type_in_postfix(ty), count)
        }
        TypeExpr::Spread { ty, .. } => {
            format!("{}^*", emit_type_in_postfix(ty))
        }
        TypeExpr::Function {
            generic_params,
            params,
            return_ty,
            ..
        } => {
            let g = emit_generic_params(generic_params);
            let ps = if params.is_empty() {
                String::new()
            } else if params.len() == 1 {
                emit_type_expr(&params[0])
            } else {
                params
                    .iter()
                    .map(emit_type_in_product)
                    .collect::<Vec<_>>()
                    .join(" * ")
            };
            format!("{}({}) => {}", g, ps, emit_type_expr(return_ty))
        }
    }
}

/// Wraps unions and function types in parens when they appear inside a product.
fn emit_type_in_product(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Union { .. } | TypeExpr::Function { .. } => {
            format!("({})", emit_type_expr(ty))
        }
        _ => emit_type_expr(ty),
    }
}

/// Wraps compound types in parens when they appear before `^N` or `^*`.
fn emit_type_in_postfix(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Union { .. } | TypeExpr::Product { .. } | TypeExpr::Function { .. } => {
            format!("({})", emit_type_expr(ty))
        }
        _ => emit_type_expr(ty),
    }
}

// ── Expression Formatting ───────────────────────────────────────────────────

/// A flattened piece of a method-call / dispatch / try chain.
#[derive(Clone)]
enum ChainPart {
    Base(Expr),
    Method { method: Ident, args: Vec<Expr> },
    Dispatch { arms: Vec<MatchArm> },
    Try,
}

fn flatten_chain(expr: &Expr) -> Vec<ChainPart> {
    let mut parts = Vec::new();
    flatten_into(expr, &mut parts);
    parts
}

fn flatten_into(expr: &Expr, parts: &mut Vec<ChainPart>) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            flatten_into(receiver, parts);
            parts.push(ChainPart::Method {
                method: method.clone(),
                args: args.clone(),
            });
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            flatten_into(scrutinee, parts);
            parts.push(ChainPart::Dispatch {
                arms: sort_arms(arms),
            });
        }
        Expr::Try { inner, .. } => {
            flatten_into(inner, parts);
            parts.push(ChainPart::Try);
        }
        other => {
            parts.push(ChainPart::Base(other.clone()));
        }
    }
}

/// Top-level expression formatter.  Tries single-line first; falls back to
/// chain breaking when the line would exceed [`MAX_WIDTH`], a dispatch is
/// present, or there are 2+ method calls in the chain.
fn emit_expr(expr: &Expr, indent: usize) -> String {
    let chain = flatten_chain(expr);
    let has_dispatch = chain
        .iter()
        .any(|p| matches!(p, ChainPart::Dispatch { .. }));

    // Count only Method parts (not Try or Dispatch) for the force-break
    // threshold.  Two or more chained method calls always break to separate
    // lines so the code reads like a pipeline regardless of total width.
    let method_count = chain
        .iter()
        .filter(|p| matches!(p, ChainPart::Method { .. }))
        .count();
    let force_break = method_count >= 2;

    if !has_dispatch && !force_break {
        let single = emit_chain_inline(&chain);
        if indent * 4 + single.len() <= MAX_WIDTH {
            return single;
        }
    }

    // Multi-line needed.
    if chain.len() > 1 || has_dispatch {
        emit_chain_multi(&chain, indent)
    } else if let [ChainPart::Base(e)] = chain.as_slice() {
        // A single expression with no `->` to break at — a literal can
        // still break inside its interpolation holes.
        emit_base_at(e, indent)
    } else {
        emit_chain_inline(&chain)
    }
}

/// Render a full expression on a single line (no newlines).
fn emit_inline(expr: &Expr) -> String {
    emit_chain_inline(&flatten_chain(expr))
}

/// The PascalCase name a method pipes to, or `None` when it stays a
/// dot-call. A method pipes when its name is a PascalCase
/// user/stdlib constructor or a builtin with a PascalCase vocabulary
/// spelling (`concat` → `Joined`, `add` → `Sum`, `print` → `Print`). A
/// *camelCase* method with no builtin mapping is an FFI binding call
/// (`.now()`, `.fetch()`, `.getRandomU64()`) — camelCase is legal at
/// the binding boundary, so those keep the dot.
fn method_pipe_name(name: &str) -> Option<&str> {
    let piped = builtin_pipe_name(name);
    if piped.chars().next().is_some_and(|c| c.is_uppercase()) {
        Some(piped)
    } else {
        None
    }
}

/// Emit a method as a pipe (`-> Name(args)`) or a dot-call (`.name(args)`).
/// `broken` selects the continuation-line pipe lead (`-> `) vs the inline
/// lead (` -> `). Field access (`.Field`) and dispatch (`.( )`) never
/// reach here — those *read*, they don't apply a function.
fn emit_method(out: &mut String, method: &Ident, args: &[Expr], broken: bool) {
    match method_pipe_name(&method.name) {
        Some(pname) => {
            out.push_str(if broken { "-> " } else { " -> " });
            out.push_str(pname);
            if !args.is_empty() {
                out.push('(');
                emit_args_inline(out, args);
                out.push(')');
            }
        }
        None => {
            out.push('.');
            out.push_str(&method.name);
            out.push('(');
            emit_args_inline(out, args);
            out.push(')');
        }
    }
}

fn emit_chain_inline(chain: &[ChainPart]) -> String {
    let mut out = String::new();
    for part in chain {
        match part {
            ChainPart::Base(e) => out.push_str(&emit_base_inline(e)),
            ChainPart::Method { method, args, .. } => {
                emit_method(&mut out, method, args, false);
            }
            ChainPart::Dispatch { arms } => {
                out.push_str(" -> (");
                for arm in arms.iter() {
                    out.push_str(" * ");
                    out.push_str(&emit_arm_inline(arm));
                }
                out.push(')');
            }
            ChainPart::Try => out.push('?'),
        }
    }
    out
}

fn emit_chain_multi(chain: &[ChainPart], indent: usize) -> String {
    let mut out = String::new();

    // Find first dispatch position.
    let dispatch_pos = chain
        .iter()
        .position(|p| matches!(p, ChainPart::Dispatch { .. }));

    if let Some(dpos) = dispatch_pos {
        // ── Pre-dispatch ──
        let before = &chain[..dpos];
        let before_str = emit_chain_inline(before);
        if indent * 4 + before_str.len() + 2 <= MAX_WIDTH {
            out.push_str(&before_str);
        } else if before.len() > 1 {
            out.push_str(&emit_chain_broken(before, indent));
        } else if let [ChainPart::Base(e)] = before {
            // A lone base with no `->` to break at — a literal can
            // still break inside its interpolation holes.
            out.push_str(&emit_base_at(e, indent));
        } else {
            out.push_str(&before_str);
        }

        // ── Dispatch ──
        if let ChainPart::Dispatch { arms } = &chain[dpos] {
            let arm_pad = "    ".repeat(indent + 1);
            let close_pad = "    ".repeat(indent);
            out.push_str(" -> (\n");
            for arm in arms.iter() {
                out.push_str(&arm_pad);
                out.push_str("* ");
                out.push_str(&emit_arm(arm, indent + 1));
                out.push('\n');
            }
            out.push_str(&close_pad);
            out.push(')');
        }

        // ── Post-dispatch ──
        let after = &chain[dpos + 1..];
        for part in after {
            match part {
                ChainPart::Method { method, args, .. } => {
                    emit_method(&mut out, method, args, false);
                }
                ChainPart::Try => out.push('?'),
                _ => {}
            }
        }
    } else {
        // No dispatch — just break the method chain.
        out.push_str(&emit_chain_broken(chain, indent));
    }

    out
}

/// Format a chain with each method call on its own continuation line.
fn emit_chain_broken(chain: &[ChainPart], indent: usize) -> String {
    let cont_pad = "    ".repeat(indent + 1);
    let mut out = String::new();

    for part in chain {
        match part {
            ChainPart::Base(e) => {
                out.push_str(&emit_base_at(e, indent));
            }
            ChainPart::Method { method, args, .. } => {
                out.push('\n');
                out.push_str(&cont_pad);
                emit_method(&mut out, method, args, true);
            }
            ChainPart::Try => out.push('?'),
            ChainPart::Dispatch { arms } => {
                // A dispatch is a pipe step (`-> ( … )`), so it gets its
                // own continuation line like every other `->` in a
                // broken chain.
                out.push('\n');
                out.push_str(&cont_pad);
                let arm_pad = "    ".repeat(indent + 2);
                let close_pad = "    ".repeat(indent + 1);
                out.push_str("-> (\n");
                for arm in arms.iter() {
                    out.push_str(&arm_pad);
                    out.push_str("* ");
                    out.push_str(&emit_arm(arm, indent + 2));
                    out.push('\n');
                }
                out.push_str(&close_pad);
                out.push(')');
            }
        }
    }

    out
}

// ── Base Expression (inline) ────────────────────────────────────────────────

/// Indent-aware base rendering: a literal breaks an interpolation hole
/// onto its own indented lines when the hole would push its line past
/// [`MAX_WIDTH`] — the one place a single expression can grow without a
/// `->` to break at. Static text is content and never moves (an HTML
/// literal's own newlines and indentation stay verbatim; each hole is
/// judged against the column it actually sits at), and the braces stay
/// glued to the surrounding text:
///
///     `<td>{
///         1 -> Inline(String)
///     }</td>`
///
/// A constructor wrapping a lone literal argument (the `Html(...)`
/// shape the `Joined`-chain fold produces) breaks through the parens.
/// Everything else falls back to the inline renderer.
fn emit_base_at(expr: &Expr, indent: usize) -> String {
    match expr {
        Expr::FormatLit { .. } | Expr::HtmlLit { .. } | Expr::JsonLit { .. } => {
            emit_literal_at(expr, indent, indent * 4)
        }
        Expr::Constructor { name, args, .. } => match args.as_slice() {
            [lit @ (Expr::FormatLit { .. } | Expr::HtmlLit { .. } | Expr::JsonLit { .. })] => {
                // The wrapper shifts the literal right by `Name(`.
                let inner = emit_literal_at(lit, indent, indent * 4 + name.name.len() + 1);
                format!("{}({})", name.name, inner)
            }
            _ => emit_base_inline(expr),
        },
        _ => emit_base_inline(expr),
    }
}

/// Render a literal starting at column `col`, breaking holes as needed.
fn emit_literal_at(lit: &Expr, indent: usize, col: usize) -> String {
    let mut w = LitWriter {
        out: String::new(),
        indent,
        col,
        line_indent: indent * 4,
    };
    match lit {
        Expr::FormatLit { parts, .. } => {
            w.out.push('`');
            w.col += 1;
            for p in parts {
                match p {
                    FormatLitPart::Static(s) => w.push_static(&escape_fmt_static(s)),
                    FormatLitPart::Interp(e) => w.push_hole(e),
                }
            }
            w.out.push('`');
        }
        Expr::HtmlLit { parts, .. } => {
            for p in parts {
                match p {
                    HtmlLitPart::Static(s) => {
                        w.push_static(&s.replace('{', "{{").replace('}', "}}"))
                    }
                    HtmlLitPart::Interp(e) => w.push_hole(e),
                }
            }
        }
        Expr::JsonLit { parts, .. } => {
            for p in parts {
                match p {
                    JsonLitPart::Static(s) => w.push_static(s),
                    JsonLitPart::Interp(e) => w.push_hole(e),
                }
            }
        }
        _ => unreachable!("emit_literal_at is only called on literal exprs"),
    }
    w.out
}

/// Column-tracking writer for literal bodies. Static text advances the
/// column (resetting at its own newlines — they are content); a hole
/// breaks onto indented lines exactly when its inline form would push
/// the current line past [`MAX_WIDTH`]. Bare references (`{Model}`,
/// `{Node.Rest}`) never break — there is nothing to break them at.
/// A broken hole indents from the current line's leading whitespace,
/// so a hole deep inside an HTML literal's own indentation stays
/// visually nested in the markup around it.
struct LitWriter {
    out: String,
    indent: usize,
    col: usize,
    /// Leading whitespace of the current visual line, in spaces — the
    /// code pad on the literal's first line, the static text's own
    /// indentation after each of its newlines.
    line_indent: usize,
}

impl LitWriter {
    fn push_static(&mut self, s: &str) {
        self.out.push_str(s);
        if let Some(i) = s.rfind('\n') {
            let tail = &s[i + 1..];
            self.col = tail.len();
            self.line_indent = tail.len() - tail.trim_start_matches(' ').len();
        } else {
            self.col += s.len();
        }
    }

    fn push_hole(&mut self, e: &Expr) {
        let inline = emit_inline(e);
        let atomic = matches!(e, Expr::Ident(_) | Expr::FieldAccess { .. });
        if atomic || self.col + inline.len() + 2 <= MAX_WIDTH {
            self.out.push('{');
            self.out.push_str(&inline);
            self.out.push('}');
            self.col += inline.len() + 2;
        } else {
            let base = self.indent.max(self.line_indent / 4);
            self.out.push_str("{\n");
            self.out.push_str(&"    ".repeat(base + 1));
            self.out.push_str(&emit_expr(e, base + 1));
            self.out.push('\n');
            self.out.push_str(&"    ".repeat(base));
            self.out.push('}');
            self.col = base * 4 + 1;
            self.line_indent = base * 4;
        }
    }
}

fn emit_base_inline(expr: &Expr) -> String {
    match expr {
        Expr::Ident(id) => id.name.clone(),
        Expr::StringLit { value, .. } => format!("\"{}\"", escape_string(value)),
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::FloatLit { value, .. } => format_float(*value),
        Expr::Constructor { name, args, .. } => {
            if args.is_empty() {
                format!("{}()", name.name)
            } else {
                let mut s = format!("{}(", name.name);
                match args.as_slice() {
                    // A product-type constructor is positionless: its
                    // fields bind to type-named slots, so canonicalise
                    // the order alphabetically. `List` is the exception —
                    // its `*`-separated arguments are ordered sequence
                    // elements, not product fields, so they keep their
                    // order.
                    [Expr::ProductValue { fields, .. }] if name.name != "List" => {
                        emit_product_fields_sorted(&mut s, fields);
                    }
                    _ => emit_args_inline(&mut s, args),
                }
                s.push(')');
                s
            }
        }
        Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } => {
            let ps = emit_param_list(params);
            let ret = emit_type_expr(return_ty);
            let body_str = body
                .exprs
                .iter()
                .map(emit_inline)
                .collect::<Vec<_>>()
                .join(" ");
            format!("({}) => {} {{ {} }}", ps, ret, body_str)
        }
        Expr::ProductValue { fields, .. } => fields
            .iter()
            .map(emit_inline)
            .collect::<Vec<_>>()
            .join(" * "),
        Expr::FieldAccess {
            receiver, field, ..
        } => {
            // The receiver may itself be a method chain
            // (`Counter(1).bump().Int`) — `emit_base_inline` renders
            // chain shapes as empty strings, so route through the
            // chain-aware inline emitter.
            format!("{}.{}", emit_inline(receiver), field.name)
        }
        Expr::JsonLit { parts, .. } => {
            // Reconstruct the source-level JSON literal by emitting each
            // Static part verbatim and each Interp part as its formatted
            // expression. The Static fragments already include the
            // surrounding `{` / `[` / `,` / `:` / `}` / `]` scaffolding
            // and any literal string keys/values, so joining them with
            // the formatted interps in place is a valid round-trip.
            let mut out = String::new();
            for p in parts {
                match p {
                    JsonLitPart::Static(s) => out.push_str(s),
                    JsonLitPart::Interp(e) => out.push_str(&emit_inline(e)),
                }
            }
            out
        }
        Expr::HtmlLit { parts, .. } => {
            // Reconstruct the source-level HTML literal. Static parts
            // hold the *unescaped* text (the scanner resolved `{{` /
            // `}}`), so literal braces must be re-escaped; interpolated
            // expressions get their `{…}` hole back.
            let mut out = String::new();
            for p in parts {
                match p {
                    HtmlLitPart::Static(s) => {
                        out.push_str(&s.replace('{', "{{").replace('}', "}}"))
                    }
                    HtmlLitPart::Interp(e) => {
                        out.push('{');
                        out.push_str(&emit_inline(e));
                        out.push('}');
                    }
                }
            }
            out
        }
        Expr::FormatLit { parts, .. } => {
            // Reconstruct the source-level backtick string. Static parts
            // hold the resolved text (the scanner applied escapes and
            // `{{` / `}}`), so re-escape the delimiter, backslash, and
            // braces; interpolated expressions get their `{…}` hole back.
            let mut out = String::from("`");
            for p in parts {
                match p {
                    FormatLitPart::Static(s) => out.push_str(&escape_fmt_static(s)),
                    FormatLitPart::Interp(e) => {
                        out.push('{');
                        out.push_str(&emit_inline(e));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        // MethodCall, Match, Try are handled by chain flattening and should
        // never appear as a Base.  Return empty as a safeguard.
        _ => String::new(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn emit_args_inline(out: &mut String, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&emit_inline(arg));
    }
}

/// Emit the fields of a product-type constructor, sorted alphabetically
/// by their rendered form. Construction is positionless (values bind to
/// fields by type), so a canonical order keeps `canon check --fix` output
/// stable regardless of the order the author wrote the fields.
fn emit_product_fields_sorted(out: &mut String, fields: &[Expr]) {
    let mut parts: Vec<String> = fields.iter().map(emit_inline).collect();
    parts.sort();
    out.push_str(&parts.join(" * "));
}

fn emit_arm_pattern(arm: &MatchArm) -> String {
    match &arm.literal {
        Some(ArmLiteral::Str(s)) => format!("\"{}\"", escape_string(s)),
        Some(ArmLiteral::Int(v)) => v.to_string(),
        None => emit_type_expr(&arm.param_ty),
    }
}

fn emit_arm_inline(arm: &MatchArm) -> String {
    let pat = emit_arm_pattern(arm);
    let ret = emit_type_expr(&arm.return_ty);
    let body = arm
        .body
        .exprs
        .iter()
        .map(emit_inline)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{} => {} {{ {} }}", pat, ret, body)
}

/// Render a dispatch arm at the given indent level. Short arms whose
/// bodies contain no nested dispatch stay on one line; an arm that
/// nests another dispatch — or whose inline form would overflow
/// `MAX_WIDTH` — breaks its body onto indented lines, so route-style
/// nested dispatch reads as a tree instead of one opaque line.
fn emit_arm(arm: &MatchArm, arm_indent: usize) -> String {
    let inline = emit_arm_inline(arm);
    let nested = arm.body.exprs.iter().any(contains_dispatch);
    if !nested && arm_indent * 4 + 2 + inline.len() <= MAX_WIDTH {
        return inline;
    }
    let pat = emit_arm_pattern(arm);
    let ret = emit_type_expr(&arm.return_ty);
    let body_pad = "    ".repeat(arm_indent + 1);
    let close_pad = "    ".repeat(arm_indent);
    let body = arm
        .body
        .exprs
        .iter()
        .map(|e| format!("{}{}", body_pad, emit_expr(e, arm_indent + 1)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{} => {} {{\n{}\n{}}}", pat, ret, body, close_pad)
}

/// Does this expression (or any sub-expression) contain a dispatch?
fn contains_dispatch(expr: &Expr) -> bool {
    match expr {
        Expr::Match { .. } => true,
        Expr::MethodCall { receiver, args, .. } => {
            contains_dispatch(receiver) || args.iter().any(contains_dispatch)
        }
        Expr::Try { inner, .. } => contains_dispatch(inner),
        Expr::Await { inner, .. } => contains_dispatch(inner),
        Expr::FieldAccess { receiver, .. } => contains_dispatch(receiver),
        Expr::Constructor { args, .. } => args.iter().any(contains_dispatch),
        Expr::ProductValue { fields, .. } => fields.iter().any(contains_dispatch),
        Expr::Lambda { body, .. } => body.exprs.iter().any(contains_dispatch),
        _ => false,
    }
}

/// Canonical dispatch-arm order: literal arms first (alphabetical for
/// strings, ascending for ints), then type arms alphabetically — which
/// puts a literal dispatch's catch-all last, and sorts a union
/// dispatch's arms into variant (alphabetical) order. Arm order never
/// carries meaning (union arms are matched by variant name, literal
/// arms by equality), so sorting is safe here and makes the ordering
/// rule auto-fixable via `canon check --fix` instead of a hand-edit.
fn sort_arms(arms: &[MatchArm]) -> Vec<MatchArm> {
    let mut sorted: Vec<MatchArm> = arms.to_vec();
    sorted.sort_by_key(arm_sort_key);
    sorted
}

fn arm_sort_key(arm: &MatchArm) -> (u8, i64, String) {
    match &arm.literal {
        Some(ArmLiteral::Int(v)) => (0, *v, String::new()),
        Some(ArmLiteral::Str(s)) => (0, 0, s.clone()),
        None => (1, 0, emit_type_expr(&arm.param_ty)),
    }
}

/// Re-escape a string literal's contents for emission. The lexer
/// stores the decoded value (with `\n`, `\t`, `\\`, `\"` already
/// translated to their raw bytes); the formatter has to put those
/// escapes back so the emitted source is parseable Canon again.
/// Escape a backtick format string's static text for re-emission: the
/// backtick delimiter and backslash take C-style escapes, `{` / `}`
/// double so they aren't read as interpolation, and control characters
/// use the lexer's escapes so the round-trip stays lossless.
fn escape_fmt_static(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Other control characters use the lexer's `\uNNNN` escape
            // so the round-trip stays lossless.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn format_float(value: f64) -> String {
    let s = value.to_string();
    if s.contains('.') {
        s
    } else {
        format!("{}.0", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_format(input: &str, expected: &str) {
        let result = format(input).expect("format failed");
        assert_eq!(result, expected);
    }

    fn assert_idempotent(input: &str) {
        let first = format(input).expect("first format failed");
        let second = format(&first).expect("second format failed");
        assert_eq!(first, second, "formatter is not idempotent");
    }

    #[test]
    fn test_simple_main() {
        assert_format(
            "main = (Stdout) => Unit {\n    \"hello\".print(Stdout)\n}\n",
            "main = (Stdout) => Unit {\n    \"hello\" -> Print(Stdout)\n}\n",
        );
    }

    #[test]
    fn test_normalize_spacing() {
        assert_format(
            "main=(Stdout)=>Unit{\n\"hello\".print(Stdout)\n}\n",
            "main = (Stdout) => Unit {\n    \"hello\" -> Print(Stdout)\n}\n",
        );
    }

    #[test]
    fn test_type_def_union() {
        assert_format("Bool=False+True\n", "Bool = False + True\n");
    }

    #[test]
    fn test_type_def_product() {
        assert_format("User=Birthday*Username\n", "User = Birthday * Username\n");
    }

    #[test]
    fn test_type_def_product_short_stays_inline() {
        // Four short fields fit under MAX_WIDTH — must stay on one line.
        assert_idempotent("Ipv4Address = Int * Int * Int * Int\n");
    }

    #[test]
    fn test_type_def_product_wide_wraps() {
        let input = "Ipv6SocketAddress = Ipv6SocketAddressAddress * Ipv6SocketAddressFlowInfo * Ipv6SocketAddressPort * Ipv6SocketAddressScopeId\n";
        let expected = "Ipv6SocketAddress = Ipv6SocketAddressAddress\n  * Ipv6SocketAddressFlowInfo\n  * Ipv6SocketAddressPort\n  * Ipv6SocketAddressScopeId\n";
        assert_format(input, expected);
        assert_idempotent(input);
    }

    #[test]
    fn test_type_def_repeat() {
        assert_format("Byte = Bit^8\n", "Byte = Bit^8\n");
    }

    #[test]
    fn test_type_def_spread() {
        assert_format("Bytes = Byte^*\n", "Bytes = Byte^*\n");
    }

    #[test]
    fn test_sorts_free_functions() {
        assert_format(
            "beta = () => Unit {\n    \"b\".print()\n}\n\nalpha = () => Unit {\n    \"a\".print()\n}\n",
            "alpha = () => Unit {\n    \"a\" -> Print\n}\n\nbeta = () => Unit {\n    \"b\" -> Print\n}\n",
        );
    }

    #[test]
    fn test_sorts_type_definitions() {
        assert_format(
            "Zed = Int\n\nAlpha = Int\n\nmain = () => Unit {\n    \"x\".print()\n}\n",
            "Alpha = Int\n\nZed = Int\n\nmain = () => Unit {\n    \"x\" -> Print\n}\n",
        );
    }

    #[test]
    fn test_main_is_pinned_not_sorted() {
        // `main` is exempt from alphabetical order (a distinguished
        // role, mirroring the checker) — it keeps its position while
        // its peers sort around it.
        assert_idempotent(
            "main = () => Unit {\n    \"hi\" -> Print\n}\n\nalpha = () => Unit {\n    \"a\" -> Print\n}\n",
        );
    }

    #[test]
    fn test_dispatch_arms_sorted() {
        // Union arms sort into variant (alphabetical) order.
        assert_format(
            "main = () => Unit {\n    True() -> (\n        * True => Unit { \"yes\".print() }\n        * False => Unit { \"no\".print() }\n    )\n}\n",
            "main = () => Unit {\n    True() -> (\n        * False => Unit { \"no\" -> Print }\n        * True => Unit { \"yes\" -> Print }\n    )\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_arms_sorted_catchall_last() {
        // Literal arms sort alphabetically; the catch-all sorts last.
        assert_format(
            "Route = (String) => String {\n    String -> (\n        * String => String { \"other\" }\n        * \"/b\" => String { \"b\" }\n        * \"/a\" => String { \"a\" }\n    )\n}\n\nmain = () => Unit {\n    \"/a\".Route().print()\n}\n",
            "Route = (String) => String {\n    String -> (\n        * \"/a\" => String { \"a\" }\n        * \"/b\" => String { \"b\" }\n        * String => String { \"other\" }\n    )\n}\n\nmain = () => Unit {\n    Route(\"/a\") -> Print\n}\n",
        );
    }

    #[test]
    fn test_dispatch_pipe_form_idempotent() {
        // The canonical dispatch spelling pipes the scrutinee in with
        // `->` (the last `.` that used to execute a flow step).
        assert_idempotent(
            "main = () => Unit {\n    True() -> (\n        * False => Unit { \"no\" -> Print }\n        * True => Unit { \"yes\" -> Print }\n    )\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_int_arms_idempotent() {
        assert_idempotent(
            "Describe = (Int) => Unit {\n    Int -> (\n        * 0 => Unit { \"zero\" -> Print }\n        * 1 => Unit { \"one\" -> Print }\n        * Int => Unit { Int -> Print }\n    )\n}\n\nmain = () => Unit {\n    0 -> Describe\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_string_escapes_round_trip() {
        assert_idempotent(
            "Kind = (String) => String {\n    String -> (\n        * \"line\\none\" => String { \"escaped\" }\n        * String => String { \"plain\" }\n    )\n}\n\nmain = () => Unit {\n    \"x\"\n        -> Kind\n        -> Print\n}\n",
        );
    }

    #[test]
    fn test_dispatch() {
        let src = "Bool = False + True\n\nmain = (Stdout) => Unit {\n    True -> (\n        * False => Unit { \"no\" -> Print(Stdout) }\n        * True => Unit { \"yes\" -> Print(Stdout) }\n    )\n}\n";
        assert_idempotent(src);
    }

    #[test]
    fn test_idempotent_hello() {
        assert_idempotent("main = (Stdout) => Unit {\n    \"hello\".print(Stdout)\n}\n");
    }

    #[test]
    fn test_idempotent_types() {
        assert_idempotent(
            "Bit = One + Zero\n\nBirthday = String\n\nBool = False + True\n\nByte = Bit^8\n\nBytes = Byte^*\n\nOrd = Equal + Greater + Less\n\nUsername = String\n\nUser = Birthday * Username\n\nOtherUser = User\n\nmain = (Stdout) => Unit {\n    \"type definitions parsed\".print(Stdout)\n}\n",
        );
    }

    #[test]
    fn test_body_less_function_type_aliases_round_trip() {
        // Bare function-type aliases are the shape of a binding file
        // (the vendored path, not a header, carries the URN). The
        // formatter passes them through as ordinary type aliases.
        assert_format(
            "getResolution = () => Duration\n\nnow = () => Mark\n",
            "getResolution = () => Duration\n\nnow = () => Mark\n",
        );
    }

    #[test]
    fn test_generics() {
        assert_format(
            "parse = <T: Deserialize>(Json * String) => Result<T, MalformedJson>\n",
            "parse = <T: Deserialize>(Json * String) => Result<T, MalformedJson>\n",
        );
    }

    #[test]
    fn test_lambda() {
        let input = "main = (Stdout) => Unit {\n    List(10 * 20 * 30).map((Int) => Int { Int.mul(2) }).print(Stdout)\n}\n";
        assert_idempotent(input);
    }

    #[test]
    fn test_product_values_canonicalized() {
        // Literal operands keep their written order — untagged
        // same-typed components bind by declaration order, so
        // reordering would change which field gets which value. The
        // canonical call form pipes the first written input and keeps
        // the rest in the parens (`Node("c" * "a" * "b")` →
        // `"c" -> Node("a" * "b")`).
        assert_format(
            "main = () => Unit {\n    Node(\"c\" * \"a\" * \"b\").print()\n}\n",
            "main = () => Unit {\n    \"c\"\n        -> Node(\"a\" * \"b\")\n        -> Print\n}\n",
        );
    }

    #[test]
    fn test_list_elements_not_sorted() {
        // `List` is an ordered sequence, not a product — its `*`-joined
        // elements keep their written order.
        assert_idempotent("main = () => Unit {\n    List(30 * 10 * 20) -> Print\n}\n");
    }

    #[test]
    fn test_try_operator() {
        let input =
            "main = (Stdout) => Result<Unit, Unit> {\n    Ok(42)?.print(Stdout)\n    Ok(Unit)\n}\n";
        assert_idempotent(input);
    }

    #[test]
    fn test_trait_function_type() {
        assert_format("Show = () => String\n", "Show = () => String\n");
    }

    #[test]
    fn test_blank_lines_between_items() {
        assert_format(
            "Greeting = String\nName = String\n",
            "Greeting = String\n\nName = String\n",
        );
    }
}
