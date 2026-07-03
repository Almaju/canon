//! Auto-await transformation.
//!
//! Runs after `loader::load_module` and before `checker::check`. Walks the
//! AST and inserts `Expr::Await` nodes wherever a `Future<T>` value is used
//! in a position that expects `T` — most importantly as a method receiver.
//!
//! The user never writes `.await`; instead the compiler infers it from the
//! type mismatch. See `DESIGN.md` § "Async" and `WASM.md` § "Auto-await
//! rule" for the rationale.
//!
//! ## Status (Phase 5, in progress)
//!
//! Handled today:
//!   - **Method receiver**: `future.method()` where `future` has static
//!     type `Future<T>` is rewritten to `Await(future).method()` so the
//!     method dispatches against `T`, not `Future<T>`.
//!   - **`?` operand**: `future?` where `future` has static type
//!     `Future<Result<T, E>>` (or `Future<Option<T>>`) is rewritten to
//!     `Await(future)?` so the `?` peels the Result/Option after the
//!     await, matching the semantics of every other language with async
//!     + Result.
//!   - **Function argument**: `f(future)` where `f` declares its
//!     corresponding parameter as `T` and the argument has static type
//!     `Future<T>` is rewritten to `f(Await(future))`. Handles both
//!     `MethodCall { args }` and `Constructor { args }` shapes, but the
//!     callee resolution is conservative — we match against
//!     free-function and `Self`-renamed-constructor keys only. The
//!     capability-receiver-as-arg pattern (`read(Filesystem)` resolving
//!     to a method on `Filesystem`) is left alone: those calls already
//!     have to chain through methods to be useful, and the receiver auto-
//!     await rule covers the chain.
//!
//! Not yet handled (acceptable while end-to-end async lowering is still
//! being built):
//!   - `Stream<T>` consumption (handled by `.each` / `.next` recognition in
//!     `async_analysis::expr_has_async_trigger` instead — no AST rewrite is
//!     needed because those method names are already async-by-construction).
//!
//! Once `wit-component` async lowering is wired into `component::wrap`, the
//! remaining cases will be added here.
//!
//! The transform is **idempotent** and **type-conservative**: it only
//! rewrites when the receiver's static type is unambiguously a `Future<T>`
//! according to the symbol table. Ambiguous cases (calls through generics,
//! lambdas without type annotations) are left alone — the worst that can
//! happen is the codegen treats the call as sync, which is the status quo.

use std::collections::HashMap;

use crate::ast::{Block, Expr, Item, MatchArm, Module, TypeExpr};

/// Apply the transform to every function body in `module`.
pub fn transform(module: &mut Module) {
    let returns = collect_method_returns(module);
    let params = collect_method_params(module);
    let ctx = Ctx {
        returns: &returns,
        params: &params,
    };
    for item in &mut module.items {
        if let Item::Function(func) = item {
            transform_block(&mut func.body, &ctx);
        }
    }
}

/// Bundle of read-only maps threaded through every recursive walker.
/// Keeps the function signatures slim as we add more derived information.
struct Ctx<'m> {
    returns: &'m MethodReturnMap,
    params: &'m MethodParamMap,
}

/// `(receiver_type, method_name) -> declared return type`. We keep the
/// full `TypeExpr` (not just the summary) so we can tell whether the
/// return is `Future<X>` and recover `X` for further propagation.
///
/// Async functions in Canon are declared by returning `Future<T>` in
/// source. The loader's `apply_bindings_directive` unwraps that to `T`
/// inside `func.return_ty` so the codegen gets the canonical-ABI shape,
/// and sets `extern_wasm.is_async = true`. The auto-await transform
/// wants `Future<T>` *back* for its static-type analysis (so call
/// sites trigger the await), so we re-wrap here whenever `is_async`
/// is set. This single point of re-wrapping keeps every other consumer
/// (codegen, type checker) seeing the unwrapped form.
type MethodReturnMap = HashMap<(String, String), TypeExpr>;

/// `(receiver_type, function_name) -> declared parameter types` (excluding
/// the receiver). Parallels `MethodReturnMap` and is used by the function-
/// argument auto-await rule to look up the expected type at each arg
/// position.
type MethodParamMap = HashMap<(String, String), Vec<TypeExpr>>;

fn collect_method_returns(module: &Module) -> MethodReturnMap {
    let mut out: MethodReturnMap = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            let ret = if func.extern_wasm.as_ref().is_some_and(|e| e.is_async) {
                wrap_future(func.return_ty.clone())
            } else {
                func.return_ty.clone()
            };
            let key = (
                func.receiver
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_default(),
                func.name.name.clone(),
            );
            out.insert(key, ret);
        }
    }
    out
}

fn collect_method_params(module: &Module) -> MethodParamMap {
    let mut out: MethodParamMap = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            let key = (
                func.receiver
                    .as_ref()
                    .map(|r| r.name.clone())
                    .unwrap_or_default(),
                func.name.name.clone(),
            );
            let params = func.params.iter().map(|p| p.ty.clone()).collect();
            out.insert(key, params);
        }
    }
    out
}

fn wrap_future(ty: TypeExpr) -> TypeExpr {
    let span = ty.span();
    TypeExpr::Named {
        name: "Future".to_string(),
        generics: vec![ty],
        span,
    }
}

fn transform_block(block: &mut Block, ctx: &Ctx<'_>) {
    for expr in &mut block.exprs {
        transform_expr(expr, ctx);
    }
}

fn transform_expr(expr: &mut Expr, ctx: &Ctx<'_>) {
    // Recurse first so inner subtrees are rewritten before we use their
    // (post-transform) static types to decide what to do at this node.
    match expr {
        Expr::MethodCall { receiver, args, .. } => {
            transform_expr(receiver, ctx);
            for a in args {
                transform_expr(a, ctx);
            }
        }
        Expr::Constructor { args, .. } => {
            for a in args {
                transform_expr(a, ctx);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            transform_expr(scrutinee, ctx);
            for arm in arms {
                transform_arm(arm, ctx);
            }
        }
        Expr::Try { inner, .. } => {
            transform_expr(inner, ctx);
            // After the recursive walk, the `?` operand might be a
            // `Future<Result<T, E>>` or `Future<Option<T>>`. The user wrote
            // `someAsyncCall()?` and the language semantics say this awaits
            // first, then extracts. Wrap the inner in `Expr::Await` so the
            // `?` peels Result/Option against the awaited payload.
            //
            // We do this *before* the outer method-receiver auto-await
            // pass returns. Otherwise the `Future<Result<…>>` shape would
            // bubble up through the Try (whose `infer_raw_type` doesn't
            // peel Future), the outer method-receiver rule would fire on
            // the Try, and we'd end up with `Await(Try(Future<Result<…>>))`
            // — a meaningless AST where `Try` runs against a non-Result.
            let inner_ty = infer_raw_type(inner, ctx.returns);
            if is_future_of_extractable(&inner_ty) {
                wrap_in_await(inner);
            }
        }
        Expr::Lambda { body, .. } => transform_block(body, ctx),
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                transform_expr(f, ctx);
            }
        }
        Expr::FieldAccess { receiver, .. } => transform_expr(receiver, ctx),
        Expr::Await { inner, .. } => transform_expr(inner, ctx),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. }
        | Expr::JsonLit { .. } => {}
    }

    // After the recursive walk, look for the function-argument auto-await
    // opportunity at *this* node. For each arg position, if the arg's
    // static type is `Future<T>` and the callee's declared parameter at
    // that position is `T`, wrap the arg in `Expr::Await`. The callee is
    // looked up via the same key-candidate sequence used for return-type
    // inference, but only the free-function (`("", name)`) and
    // Self-renamed-constructor (`(name, "Self")`) keys are tried —
    // capability-receiver-as-arg dispatch is conservatively skipped.
    auto_await_call_args(expr, ctx);

    // After the recursive walk, look for the auto-await opportunity at *this*
    // node: a method call whose receiver is `Future<T>`. The concurrency
    // combinators are the one exception — `a.parallel(b)` / `a.race(b)`
    // *want* the un-awaited futures on both sides; the codegen drives the
    // subtasks through the canonical-ABI waitable-set sequence itself.
    if let Expr::MethodCall {
        receiver, method, ..
    } = expr
    {
        if matches!(method.name.as_str(), "parallel" | "race") {
            return;
        }
        let recv_raw = infer_raw_type(receiver, ctx.returns);
        if is_future(&recv_raw) {
            // Wrap `receiver` in `Expr::Await(receiver)`. The codegen treats
            // `Expr::Await` as a pass-through today; `async_analysis` uses
            // it as the seed for the bottom-up fixpoint.
            wrap_in_await(receiver);
        }
    }
}

/// For a call-shaped expression (`MethodCall` or `Constructor`), look up
/// the callee's expected parameter types and wrap each Future-typed arg
/// whose corresponding parameter is `T` (not `Future<T>`) in an explicit
/// `Expr::Await`.
///
/// The matching rule is intentionally narrow: we only wrap when the
/// argument's static type is `Future<T>` *and* the parameter's type name
/// equals `T`'s name. Generic / unknown / mismatched shapes fall through
/// untouched. The worst that can happen is a missed await, which the user
/// can then trigger explicitly by chaining through a method instead.
fn auto_await_call_args(expr: &mut Expr, ctx: &Ctx<'_>) {
    let (callee_keys, args): (Vec<(String, String)>, &mut Vec<Expr>) = match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // The callee's receiver type is the receiver expression's
            // static type (after inner Future/Stream peeling, since the
            // method dispatches against the inner type).
            let recv_ty = infer_raw_type(receiver, ctx.returns);
            let recv_name = match unwrap_async_name(&recv_ty) {
                Some(n) => n,
                None => return,
            };
            (vec![(recv_name, method.name.clone())], args)
        }
        Expr::Constructor { name, args, .. } => {
            // Try the same key candidates as `infer_raw_type` does for
            // constructors, in the same order:
            //   1. Free function `("", name)`
            //   2. Self-renamed constructor `(name, "Self")`
            //
            // The capability-receiver-as-arg pattern is intentionally
            // skipped — see the module docstring for why.
            (
                vec![
                    (String::new(), name.name.clone()),
                    (name.name.clone(), "Self".to_string()),
                ],
                args,
            )
        }
        _ => return,
    };

    // Find the first key that resolves and use its param list.
    let param_tys: &[TypeExpr] = match callee_keys.iter().find_map(|k| ctx.params.get(k)) {
        Some(ps) => ps.as_slice(),
        None => return,
    };

    for (i, arg) in args.iter_mut().enumerate() {
        let Some(param_ty) = param_tys.get(i) else {
            break;
        };
        let arg_ty = infer_raw_type(arg, ctx.returns);
        if future_inner_matches(&arg_ty, param_ty) {
            wrap_in_await_expr(arg);
        }
    }
}

/// Helper: replace `*target` with `Expr::Await { inner: take(*target) }`
/// preserving the original span.
fn wrap_in_await(target: &mut Box<Expr>) {
    use crate::error::Span;
    let span = target.span();
    let taken = std::mem::replace(
        target.as_mut(),
        Expr::IntLit {
            value: 0,
            span: Span::default(),
        },
    );
    **target = Expr::Await {
        inner: Box::new(taken),
        span,
    };
}

/// Same as `wrap_in_await` but operates on `&mut Expr` directly (used in
/// arg-list iteration where we don't have a `Box`).
fn wrap_in_await_expr(target: &mut Expr) {
    use crate::error::Span;
    let span = target.span();
    let taken = std::mem::replace(
        target,
        Expr::IntLit {
            value: 0,
            span: Span::default(),
        },
    );
    *target = Expr::Await {
        inner: Box::new(taken),
        span,
    };
}

/// True when `arg_ty` is `Future<T>` and `param_ty` is structurally `T`
/// (same top-level name; we don't recurse into generics because the only
/// realistic mismatch shape that matters today is `Future<String>` vs
/// `String`, `Future<Int>` vs `Int`, etc.). A `<unknown>` placeholder
/// never matches.
fn future_inner_matches(arg_ty: &TypeExpr, param_ty: &TypeExpr) -> bool {
    let TypeExpr::Named {
        name: arg_name,
        generics: arg_gens,
        ..
    } = arg_ty
    else {
        return false;
    };
    if arg_name != "Future" || arg_gens.len() != 1 {
        return false;
    }
    let TypeExpr::Named {
        name: inner_name, ..
    } = &arg_gens[0]
    else {
        return false;
    };
    let TypeExpr::Named {
        name: param_name, ..
    } = param_ty
    else {
        return false;
    };
    inner_name == param_name && inner_name != "<unknown>"
}

fn transform_arm(arm: &mut MatchArm, ctx: &Ctx<'_>) {
    transform_block(&mut arm.body, ctx);
}

/// Best-effort static type inference *without* unwrapping `Future` /
/// `Stream`. This is the dual of `checker::expr_type_name_in_scope`, which
/// strips those wrappers eagerly for downstream type-checking.
///
/// Returns a placeholder `TypeExpr::Named("<unknown>")` when the shape
/// isn't statically obvious — auto-await will then leave the receiver
/// alone.
fn infer_raw_type(expr: &Expr, returns: &MethodReturnMap) -> TypeExpr {
    use crate::error::Span;
    let unknown = TypeExpr::Named {
        name: "<unknown>".to_string(),
        generics: vec![],
        span: Span::default(),
    };
    match expr {
        Expr::MethodCall {
            receiver, method, ..
        } => {
            let recv_ty = infer_raw_type(receiver, returns);
            let recv_name = match unwrap_async_name(&recv_ty) {
                Some(n) => n,
                None => return unknown,
            };
            returns
                .get(&(recv_name, method.name.clone()))
                .cloned()
                .unwrap_or(unknown)
        }
        Expr::Constructor { name, args, .. } => {
            // A constructor-shaped call `Foo(…)` can be four things:
            //   1. A type constructor (`Path(s)`) — the value's type is
            //      literally `Foo`.
            //   2. A free function call (`syncRead()`) — the value's type
            //      is the function's declared return type, which
            //      `collect_method_returns` keys under `("", "syncRead")`.
            //   3. A `Self`-renamed constructor (`Now()`) — keyed under
            //      `("Now", "Self")` after `resolve_new_syntax`.
            //   4. A capability-receiver method written as
            //      `slowRead(Filesystem)` — the parser turns this into
            //      `Constructor { name: "slowRead", args: [Filesystem] }`
            //      but the real declaration is
            //      `slowRead = (Filesystem) -> …` with `Filesystem` as the
            //      receiver, keyed `("Filesystem", "slowRead")`.
            //
            // Probe in the order (2), (3), (4); if none match, fall back
            // to the bare constructor name.
            if let Some(ty) = returns.get(&(String::new(), name.name.clone())) {
                return ty.clone();
            }
            if let Some(ty) = returns.get(&(name.name.clone(), "Self".to_string())) {
                return ty.clone();
            }
            for a in args {
                if let Expr::Ident(id) = a {
                    if let Some(ty) = returns.get(&(id.name.clone(), name.name.clone())) {
                        return ty.clone();
                    }
                }
            }
            TypeExpr::Named {
                name: name.name.clone(),
                generics: vec![],
                span: name.span,
            }
        }
        Expr::Ident(id) => TypeExpr::Named {
            name: id.name.clone(),
            generics: vec![],
            span: id.span,
        },
        Expr::StringLit { span, .. } => TypeExpr::Named {
            name: "String".to_string(),
            generics: vec![],
            span: *span,
        },
        Expr::IntLit { span, .. } | Expr::HexLit { span, .. } => TypeExpr::Named {
            name: "Int".to_string(),
            generics: vec![],
            span: *span,
        },
        Expr::FloatLit { span, .. } => TypeExpr::Named {
            name: "Float".to_string(),
            generics: vec![],
            span: *span,
        },
        Expr::Await { inner, .. } => {
            // After an explicit await, the type is the inner type's payload.
            let inner_ty = infer_raw_type(inner, returns);
            match inner_ty {
                TypeExpr::Named {
                    name,
                    mut generics,
                    span: _,
                } if (name == "Future" || name == "Stream") && generics.len() == 1 => {
                    generics.remove(0)
                }
                other => other,
            }
        }
        Expr::Try { inner, .. } => {
            // `?` extracts the Ok/Some payload — the unwrapped Future was
            // already handled by the recursive transform.
            let inner_ty = infer_raw_type(inner, returns);
            match inner_ty {
                TypeExpr::Named {
                    name, mut generics, ..
                } if (name == "Result" || name == "Option") && !generics.is_empty() => {
                    generics.remove(0)
                }
                other => other,
            }
        }
        _ => unknown,
    }
}

/// If `ty` is a single-typed wrapper (`Future<X>` or `Stream<X>`), return
/// the inner type's name; otherwise the type's own name. Used to find the
/// receiver type for method-return lookup so a chained
/// `path.File().read()` resolves the `read` method on `File` even when
/// `File()` returns `Future<File>`.
fn unwrap_async_name(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            if (name == "Future" || name == "Stream") && generics.len() == 1 {
                if let TypeExpr::Named { name: inner, .. } = &generics[0] {
                    return Some(inner.clone());
                }
                return None;
            }
            Some(name.clone())
        }
        _ => None,
    }
}

fn is_future(ty: &TypeExpr) -> bool {
    matches!(
        ty,
        TypeExpr::Named { name, generics, .. }
            if name == "Future" && generics.len() == 1
    )
}

/// True when `ty` is `Future<Result<…>>` or `Future<Option<…>>` — i.e. a
/// future whose payload is something `?` can peel. Used by the `?`-operand
/// auto-await rule to decide whether to insert an implicit await before
/// the `?` strip.
fn is_future_of_extractable(ty: &TypeExpr) -> bool {
    let TypeExpr::Named { name, generics, .. } = ty else {
        return false;
    };
    if name != "Future" || generics.len() != 1 {
        return false;
    }
    matches!(
        &generics[0],
        TypeExpr::Named { name: inner, generics: inner_g, .. }
            if (inner == "Result" || inner == "Option") && !inner_g.is_empty()
    )
}
