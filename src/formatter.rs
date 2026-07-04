//! Canon source code formatter.
//!
//! Parses an `.can` source file and emits it in canonical format.
//! The formatter enforces "one way" to write Canon code — consistent
//! spacing, indentation, and line breaking.

use crate::ast::*;
use crate::error::Result;
use crate::lexer::Scanner;
use crate::parser::Parser;

const MAX_WIDTH: usize = 100;

/// Format an Canon source string, returning the canonically formatted version.
pub fn format(source: &str) -> Result<String> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let module = parser.parse()?;
    Ok(emit_module(&module))
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
    f.name.name == "main" || entry_world_of(&f.return_ty) == Some(EntryWorld::Http)
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

    // Signature: name<G> = (params) -> ReturnType
    out.push_str(&func.name.name);
    out.push_str(&emit_generic_params(&func.generic_params));
    out.push_str(" = (");
    out.push_str(&emit_fn_params(func));
    out.push_str(") -> ");
    out.push_str(&emit_type_expr(&func.return_ty));

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
                parts.push(emit_param(p));
            }
            parts.join(" * ")
        }
    } else {
        emit_param_list(&func.params)
    }
}

fn emit_param_list(params: &[Param]) -> String {
    params.iter().map(emit_param).collect::<Vec<_>>().join(", ")
}

fn emit_param(p: &Param) -> String {
    let ty = emit_type_expr(&p.ty);
    if p.mutable {
        format!("mut {}", ty)
    } else {
        ty
    }
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
            format!("{}({}) -> {}", g, ps, emit_type_expr(return_ty))
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
    Method {
        method: Ident,
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
    },
    Dispatch {
        arms: Vec<MatchArm>,
    },
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
            type_args,
            args,
            ..
        } => {
            flatten_into(receiver, parts);
            parts.push(ChainPart::Method {
                method: method.clone(),
                type_args: type_args.clone(),
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
    } else {
        // A single expression with no chain to break — emit as-is.
        emit_chain_inline(&chain)
    }
}

/// Render a full expression on a single line (no newlines).
fn emit_inline(expr: &Expr) -> String {
    emit_chain_inline(&flatten_chain(expr))
}

fn emit_chain_inline(chain: &[ChainPart]) -> String {
    let mut out = String::new();
    for part in chain {
        match part {
            ChainPart::Base(e) => out.push_str(&emit_base_inline(e)),
            ChainPart::Method {
                method,
                type_args,
                args,
            } => {
                out.push('.');
                out.push_str(&method.name);
                emit_turbofish(&mut out, type_args);
                out.push('(');
                emit_args_inline(&mut out, args);
                out.push(')');
            }
            ChainPart::Dispatch { arms } => {
                out.push_str(".(");
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
        } else {
            out.push_str(&before_str);
        }

        // ── Dispatch ──
        if let ChainPart::Dispatch { arms } = &chain[dpos] {
            let arm_pad = "    ".repeat(indent + 1);
            let close_pad = "    ".repeat(indent);
            out.push_str(".(\n");
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
                ChainPart::Method {
                    method,
                    type_args,
                    args,
                } => {
                    out.push('.');
                    out.push_str(&method.name);
                    emit_turbofish(&mut out, type_args);
                    out.push('(');
                    emit_args_inline(&mut out, args);
                    out.push(')');
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
                out.push_str(&emit_base_inline(e));
            }
            ChainPart::Method {
                method,
                type_args,
                args,
            } => {
                out.push('\n');
                out.push_str(&cont_pad);
                out.push('.');
                out.push_str(&method.name);
                emit_turbofish(&mut out, type_args);
                out.push('(');
                emit_args_inline(&mut out, args);
                out.push(')');
            }
            ChainPart::Try => out.push('?'),
            ChainPart::Dispatch { arms } => {
                let arm_pad = "    ".repeat(indent + 2);
                let close_pad = "    ".repeat(indent + 1);
                out.push_str(".(\n");
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

fn emit_base_inline(expr: &Expr) -> String {
    match expr {
        Expr::Ident(id) => id.name.clone(),
        Expr::StringLit { value, .. } => format!("\"{}\"", escape_string(value)),
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::FloatLit { value, .. } => format_float(*value),
        Expr::HexLit { value, .. } => format!("0x{:X}", value),
        Expr::Constructor { name, args, .. } => {
            if args.is_empty() {
                format!("{}()", name.name)
            } else {
                let mut s = format!("{}(", name.name);
                emit_args_inline(&mut s, args);
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
            format!("({}) -> {} {{ {} }}", ps, ret, body_str)
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
        // MethodCall, Match, Try are handled by chain flattening and should
        // never appear as a Base.  Return empty as a safeguard.
        _ => String::new(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn emit_turbofish(out: &mut String, type_args: &[TypeExpr]) {
    if !type_args.is_empty() {
        out.push_str("::<");
        let targs: Vec<String> = type_args.iter().map(emit_type_expr).collect();
        out.push_str(&targs.join(", "));
        out.push('>');
    }
}

fn emit_args_inline(out: &mut String, args: &[Expr]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&emit_inline(arg));
    }
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
    format!("({}) -> {} {{ {} }}", pat, ret, body)
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
    format!("({}) -> {} {{\n{}\n{}}}", pat, ret, body, close_pad)
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
/// rule auto-fixable via `canon fmt` instead of a hand-edit.
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
            "main = (Stdout) -> Unit {\n    \"hello\".print(Stdout)\n}\n",
            "main = (Stdout) -> Unit {\n    \"hello\".print(Stdout)\n}\n",
        );
    }

    #[test]
    fn test_normalize_spacing() {
        assert_format(
            "main=(Stdout)->Unit{\n\"hello\".print(Stdout)\n}\n",
            "main = (Stdout) -> Unit {\n    \"hello\".print(Stdout)\n}\n",
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
            "beta = () -> Unit {\n    \"b\".print()\n}\n\nalpha = () -> Unit {\n    \"a\".print()\n}\n",
            "alpha = () -> Unit {\n    \"a\".print()\n}\n\nbeta = () -> Unit {\n    \"b\".print()\n}\n",
        );
    }

    #[test]
    fn test_sorts_type_definitions() {
        assert_format(
            "Zed = Int\n\nAlpha = Int\n\nmain = () -> Unit {\n    \"x\".print()\n}\n",
            "Alpha = Int\n\nZed = Int\n\nmain = () -> Unit {\n    \"x\".print()\n}\n",
        );
    }

    #[test]
    fn test_main_is_pinned_not_sorted() {
        // `main` is exempt from alphabetical order (a distinguished
        // role, mirroring the checker) — it keeps its position while
        // its peers sort around it.
        assert_idempotent(
            "main = () -> Unit {\n    \"hi\".print()\n}\n\nalpha = () -> Unit {\n    \"a\".print()\n}\n",
        );
    }

    #[test]
    fn test_dispatch_arms_sorted() {
        // Union arms sort into variant (alphabetical) order.
        assert_format(
            "main = () -> Unit {\n    True().(\n        * (True) -> Unit { \"yes\".print() }\n        * (False) -> Unit { \"no\".print() }\n    )\n}\n",
            "main = () -> Unit {\n    True().(\n        * (False) -> Unit { \"no\".print() }\n        * (True) -> Unit { \"yes\".print() }\n    )\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_arms_sorted_catchall_last() {
        // Literal arms sort alphabetically; the catch-all sorts last.
        assert_format(
            "route = (String) -> String {\n    String.(\n        * (String) -> String { \"other\" }\n        * (\"/b\") -> String { \"b\" }\n        * (\"/a\") -> String { \"a\" }\n    )\n}\n\nmain = () -> Unit {\n    \"/a\".route().print()\n}\n",
            "route = (String) -> String {\n    String.(\n        * (\"/a\") -> String { \"a\" }\n        * (\"/b\") -> String { \"b\" }\n        * (String) -> String { \"other\" }\n    )\n}\n\nmain = () -> Unit {\n    \"/a\"\n        .route()\n        .print()\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_int_arms_idempotent() {
        assert_idempotent(
            "describe = (Int) -> Unit {\n    Int.(\n        * (0) -> Unit { \"zero\".print() }\n        * (1) -> Unit { \"one\".print() }\n        * (Int) -> Unit { Int.print() }\n    )\n}\n\nmain = () -> Unit {\n    0.describe()\n}\n",
        );
    }

    #[test]
    fn test_literal_dispatch_string_escapes_round_trip() {
        assert_idempotent(
            "kind = (String) -> String {\n    String.(\n        * (\"line\\none\") -> String { \"escaped\" }\n        * (String) -> String { \"plain\" }\n    )\n}\n\nmain = () -> Unit {\n    \"x\".kind().print()\n}\n",
        );
    }

    #[test]
    fn test_dispatch() {
        let src = "Bool = False + True\n\nmain = (Stdout) -> Unit {\n    True.(\n        * (False) -> Unit { \"no\".print(Stdout) }\n        * (True) -> Unit { \"yes\".print(Stdout) }\n    )\n}\n";
        assert_idempotent(src);
    }

    #[test]
    fn test_idempotent_hello() {
        assert_idempotent("main = (Stdout) -> Unit {\n    \"hello\".print(Stdout)\n}\n");
    }

    #[test]
    fn test_idempotent_types() {
        assert_idempotent(
            "Bit = One + Zero\n\nBirthday = String\n\nBool = False + True\n\nByte = Bit^8\n\nBytes = Byte^*\n\nOrd = Equal + Greater + Less\n\nUsername = String\n\nUser = Birthday * Username\n\nOtherUser = User\n\nmain = (Stdout) -> Unit {\n    \"type definitions parsed\".print(Stdout)\n}\n",
        );
    }

    #[test]
    fn test_body_less_function_type_aliases_round_trip() {
        // Bare function-type aliases are the shape of a binding file
        // (the vendored path, not a header, carries the URN). The
        // formatter passes them through as ordinary type aliases.
        assert_format(
            "getResolution = () -> Duration\n\nnow = () -> Mark\n",
            "getResolution = () -> Duration\n\nnow = () -> Mark\n",
        );
    }

    #[test]
    fn test_generics() {
        assert_format(
            "parse = <T: Deserialize>(Json * String) -> Result<T, MalformedJson>\n",
            "parse = <T: Deserialize>(Json * String) -> Result<T, MalformedJson>\n",
        );
    }

    #[test]
    fn test_lambda() {
        let input = "main = (Stdout) -> Unit {\n    List(10, 20, 30).map((Int) -> Int { Int.mul(2) }).print(Stdout)\n}\n";
        assert_idempotent(input);
    }

    #[test]
    fn test_try_operator() {
        let input =
            "main = (Stdout) -> Result<Unit, Unit> {\n    Ok(42)?.print(Stdout)\n    Ok(Unit)\n}\n";
        assert_idempotent(input);
    }

    #[test]
    fn test_trait_function_type() {
        assert_format("Show = () -> String\n", "Show = () -> String\n");
    }

    #[test]
    fn test_blank_lines_between_items() {
        assert_format(
            "Greeting = String\nName = String\n",
            "Greeting = String\n\nName = String\n",
        );
    }
}
