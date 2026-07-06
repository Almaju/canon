//! Dependency inference (implicit capability threading).
//!
//! Runs after `loader::load_module` and before `checker::check`, alongside
//! the [`auto_await`](super::auto_await) pass. Where a constructor call
//! *omits* an argument the callee requires, and the enclosing function has
//! exactly one in-scope value of that type, the missing argument is supplied
//! automatically — a reference to the enclosing parameter, by type.
//!
//! The user writes the terse form:
//!
//! ```canon
//! SqliteConnection => Main {
//!     Foo()
//! }
//!
//! SqliteConnection => Foo {
//!     …
//! }
//! ```
//!
//! and this pass rewrites `Foo()` to `Foo(SqliteConnection)` before the
//! checker ever sees it — identical to what the author would have written by
//! hand. Dependencies flow down the call tree without being repeated at every
//! call site.
//!
//! ## Honesty first
//!
//! This is implicit *threading*, never implicit *authority*. `Foo` still
//! declares `SqliteConnection => Foo` in its signature; the capability stays
//! in the type, and a function cannot touch a dependency its signature does
//! not name. Only the re-passing at the call site is inferred. Reading a
//! function's type still tells you exactly what it requires. See the language
//! spec (`docs/src/spec/effects-and-async.md`, "Effects Are Values").
//!
//! ## The rule
//!
//! For a call to a single-constructor type `Name` with parameter types
//! `P₁ … Pₙ` — the prefix form `Name(provided…)` or its piped canonical form
//! `x -> Name(provided…)`, into which `canon fmt` rewrites any call carrying
//! an explicit argument (the receiver is then one of the provided values, a
//! piped call to a type constructor being construction in Canon):
//!
//! 1. Every provided value must match a distinct parameter by type name (so
//!    we know exactly which slots remain unfilled). If any provided value's
//!    type can't be inferred or doesn't match a parameter, the call is left
//!    untouched.
//! 2. For each remaining parameter type, the enclosing scope must contain
//!    **exactly one** value of that type (a parameter or receiver, matched by
//!    type name). Zero candidates or two-or-more (ambiguous) → the call is
//!    left untouched, and the checker reports the original error.
//! 3. Only when *every* remaining slot is uniquely fillable are the inferred
//!    arguments appended. We never half-fill a call.
//!
//! ## Conservative by construction
//!
//! The transform only ever turns a call the checker would *reject*
//! (under-applied) into one it *accepts*, and only when the fill is
//! unambiguous. A fully-applied call has no missing slots, so it is never
//! touched — no existing program changes meaning. It also only fires for a
//! type with exactly one constructor: families, trait dispatch, builtins, and
//! the concurrency combinators are all left alone. Matching is by exact
//! declared type name; alias/newtype widening is deliberately out of scope for
//! this first cut.

use crate::ast::{Block, Expr, Ident, Item, MatchArm, Module, TypeExpr};
use crate::error::Span;
use std::collections::{HashMap, HashSet};

/// One parameter's type as seen by inference: `Some(name)` for a plain
/// `Named` type with no generics (the only shape we can fill from scope),
/// `None` for anything else (product, generic, function type).
type ParamTy = Option<String>;

/// Callee signatures keyed like [`auto_await`](super::auto_await): a
/// constructor `Name` is found under `(Name, "Self")` after
/// `resolve_new_syntax`, a free function under `("", Name)`. The value is the
/// list of *distinct* signatures found for that key — a constructor family
/// (several constructors of one type) yields more than one, and inference
/// bails rather than guess which member a bare call means.
type SigMap = HashMap<(String, String), Vec<Vec<ParamTy>>>;

/// Read-only maps threaded through the recursive walk.
struct Ctx<'m> {
    sigs: &'m SigMap,
    /// Names declared as constructible types in the module — used to
    /// recognise that a piped call `x -> T` constructs a value of type `T`.
    types: &'m HashSet<String>,
}

/// Apply the transform to every function body in `module`.
pub fn transform(module: &mut Module) {
    let sigs = collect_signatures(module);
    let types = collect_type_names(module);
    let ctx = Ctx {
        sigs: &sigs,
        types: &types,
    };
    for item in &mut module.items {
        if let Item::Function(func) = item {
            let mut scope: Vec<String> = Vec::new();
            for p in &func.params {
                push_param_type_names(&p.ty, &mut scope);
            }
            if let Some(recv) = &func.receiver {
                scope.push(recv.name.clone());
            }
            transform_block(&mut func.body, &scope, &ctx);
        }
    }
}

fn collect_type_names(module: &Module) -> HashSet<String> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::TypeDef(td) if !matches!(td.body, TypeExpr::Function { .. }) => {
                Some(td.name.name.clone())
            }
            _ => None,
        })
        .collect()
}

fn collect_signatures(module: &Module) -> SigMap {
    let mut out: SigMap = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            let key = (
                func.receiver
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_default(),
                func.name.name.clone(),
            );
            let sig: Vec<ParamTy> = func.params.iter().map(|p| param_ty(&p.ty)).collect();
            out.entry(key).or_default().push(sig);
        }
    }
    out
}

/// The fillable type name of a parameter, or `None` for shapes inference
/// can't source from an enclosing value.
fn param_ty(ty: &TypeExpr) -> ParamTy {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// Mirror of the checker's `push_param_names`: a value is referenced inside a
/// body by its type name, so a `Named` param contributes its name and a
/// `Product` param contributes each component.
fn push_param_type_names(ty: &TypeExpr, out: &mut Vec<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => out.push(name.clone()),
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                push_param_type_names(f, out);
            }
        }
        _ => {}
    }
}

fn transform_block(block: &mut Block, scope: &[String], ctx: &Ctx<'_>) {
    for expr in &mut block.exprs {
        transform_expr(expr, scope, ctx);
    }
}

fn transform_expr(expr: &mut Expr, scope: &[String], ctx: &Ctx<'_>) {
    // Recurse first, so nested calls are filled before we consider this node.
    // Match arms and lambdas extend the in-scope set, exactly as the checker
    // does when building the inner `ExprScope`.
    match expr {
        Expr::Constructor { args, .. } => {
            for a in args.iter_mut() {
                transform_expr(a, scope, ctx);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            transform_expr(receiver, scope, ctx);
            for a in args.iter_mut() {
                transform_expr(a, scope, ctx);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            transform_expr(scrutinee, scope, ctx);
            for arm in arms.iter_mut() {
                let inner = arm_scope(arm, scope);
                transform_block(&mut arm.body, &inner, ctx);
            }
        }
        Expr::Try { inner, .. } => transform_expr(inner, scope, ctx),
        Expr::Lambda { params, body, .. } => {
            let mut inner = scope.to_vec();
            for p in params.iter() {
                push_param_type_names(&p.ty, &mut inner);
            }
            transform_block(body, &inner, ctx);
        }
        Expr::ProductValue { fields, .. } => {
            for f in fields.iter_mut() {
                transform_expr(f, scope, ctx);
            }
        }
        Expr::FieldAccess { receiver, .. } => transform_expr(receiver, scope, ctx),
        Expr::Await { inner, .. } => transform_expr(inner, scope, ctx),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. }
        | Expr::JsonLit { .. }
        | Expr::HtmlLit { .. } => {}
    }

    // Fill omitted dependencies on the two call shapes that reach a
    // single-constructor type: the prefix form `Foo(…)` and its piped
    // canonical form `x -> Foo(…)` (which `canon fmt` produces for any call
    // carrying an explicit argument). A piped call to a type constructor *is*
    // construction in Canon, so the receiver is a component and binds
    // commutatively alongside the rest — we gate on the callee having exactly
    // one constructor signature, which leaves trait dispatch, builtins, and
    // constructor families untouched.
    match expr {
        Expr::Constructor { name, args, span } => {
            let provided: Vec<ParamTy> = args.iter().map(|a| arg_type_name(a, ctx)).collect();
            if let Some(inferred) = infer_missing(&name.name, &provided, *span, scope, ctx) {
                args.extend(inferred);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
            ..
        } if !CONCURRENT_COMBINATORS.contains(&method.name.as_str()) => {
            let mut provided: Vec<ParamTy> = vec![arg_type_name(receiver, ctx)];
            provided.extend(args.iter().map(|a| arg_type_name(a, ctx)));
            if let Some(inferred) = infer_missing(&method.name, &provided, *span, scope, ctx) {
                args.extend(inferred);
            }
        }
        _ => {}
    }
}

/// Concurrency combinators are methods on a future, not type constructors —
/// `a -> Parallel(b)` must never be treated as construction. Mirrors the
/// checker's list of the same name.
const CONCURRENT_COMBINATORS: &[&str] = &["parallel", "race", "Parallel", "Race"];

/// Extend `scope` with the value(s) a match arm binds, mirroring the checker's
/// inner-scope construction for dispatch arms.
fn arm_scope(arm: &MatchArm, scope: &[String]) -> Vec<String> {
    let mut inner = scope.to_vec();
    if let TypeExpr::Named { name, generics, .. } = &arm.param_ty {
        for g in generics {
            push_param_type_names(g, &mut inner);
        }
        inner.push(name.clone());
    }
    inner
}

/// Compute the arguments to append to a call to `name` given the types
/// already `provided` (receiver included, for a piped call). Returns `None` —
/// leaving the call untouched — unless every unfilled slot is uniquely
/// sourced from `scope`.
fn infer_missing(
    name: &str,
    provided: &[ParamTy],
    span: Span,
    scope: &[String],
    ctx: &Ctx<'_>,
) -> Option<Vec<Expr>> {
    // Exactly one signature must be known for this name — a constructor
    // family (several members) is ambiguous and left alone.
    let sig = single_signature(name, ctx.sigs)?;
    if sig.is_empty() {
        return None; // nullary — nothing to fill (`List()`, `Unit()`, variants, …)
    }
    // Repeated components (`T^2`) bind positionally, not by type; don't touch.
    if has_duplicate_named(sig) {
        return None;
    }

    // Consume each provided value against a distinct parameter slot by type
    // name. Every provided value must land on a still-empty slot — otherwise
    // we can't tell which slots remain, and we bail. Slot names are distinct
    // here (`has_duplicate_named` above), so a name identifies one slot.
    let mut filled = vec![false; sig.len()];
    for pty in provided {
        let arg_ty = pty.as_ref()?; // un-inferable provided value → bail
        let slot = sig
            .iter()
            .position(|p| p.as_deref() == Some(arg_ty.as_str()))?;
        if filled[slot] {
            return None; // two values for one slot — not a shape we fill
        }
        filled[slot] = true;
    }

    // Every remaining slot must be a plain named type with exactly one
    // matching value in the enclosing scope.
    let mut inferred: Vec<Expr> = Vec::new();
    for (i, p) in sig.iter().enumerate() {
        if filled[i] {
            continue;
        }
        let param_name = p.as_ref()?; // non-nameable slot we can't source from scope
        if count_in_scope(scope, param_name) != 1 {
            return None; // zero candidates, or ambiguous — leave for the checker
        }
        inferred.push(Expr::Ident(Ident {
            name: param_name.clone(),
            span,
        }));
    }

    (!inferred.is_empty()).then_some(inferred)
}

fn single_signature<'a>(name: &str, sigs: &'a SigMap) -> Option<&'a Vec<ParamTy>> {
    // Constructor key first (`(Name, "Self")`), then free function.
    for key in [
        (name.to_string(), "Self".to_string()),
        (String::new(), name.to_string()),
    ] {
        if let Some(list) = sigs.get(&key) {
            if list.len() == 1 {
                return list.first();
            }
            return None; // a family — ambiguous
        }
    }
    None
}

fn has_duplicate_named(sig: &[ParamTy]) -> bool {
    for i in 0..sig.len() {
        if let Some(a) = &sig[i] {
            if sig[i + 1..]
                .iter()
                .any(|b| b.as_deref() == Some(a.as_str()))
            {
                return true;
            }
        }
    }
    false
}

fn count_in_scope(scope: &[String], ty: &str) -> usize {
    scope.iter().filter(|n| n.as_str() == ty).count()
}

/// Best-effort static type name of an argument expression, used only to work
/// out which parameter slots are already filled. `None` (unknown) makes the
/// enclosing call bail — we never guess.
///
/// A piped construction `x -> T` (a `MethodCall` whose method names a type)
/// yields a value of type `T`; recognising this lets the receiver of an outer
/// piped call be matched, which is how partial application survives `canon
/// fmt`'s rewrite of `T(x -> …)` into the pipe form. A `MethodCall` whose
/// method is a builtin or trait (not a type) stays `None` and bails safely.
fn arg_type_name(expr: &Expr, ctx: &Ctx<'_>) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.name.clone()),
        Expr::Constructor { name, .. } => Some(name.name.clone()),
        Expr::StringLit { .. } => Some("String".to_string()),
        Expr::IntLit { .. } | Expr::HexLit { .. } => Some("Int".to_string()),
        Expr::FloatLit { .. } => Some("Float".to_string()),
        Expr::MethodCall { method, .. }
            if ctx.types.contains(&method.name) || is_primitive_ctor(&method.name) =>
        {
            Some(method.name.clone())
        }
        _ => None,
    }
}

fn is_primitive_ctor(name: &str) -> bool {
    matches!(name, "Int" | "Float" | "String" | "Bool")
}
