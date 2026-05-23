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
//! This module currently handles the most common case: a method call whose
//! receiver has a `Future<T>` type. The receiver is rewritten as
//! `Expr::Await(receiver)` so that `codegen::async_analysis::analyse` can
//! pick it up and mark the enclosing function suspending.
//!
//! Not yet handled (acceptable while end-to-end async lowering is still
//! being built):
//!   - `Future<T>` passed as a function argument expecting `T`.
//!   - `Future<T>` used as the operand of `?`.
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
    for item in &mut module.items {
        if let Item::Function(func) = item {
            transform_block(&mut func.body, &returns);
        }
    }
}

/// `(receiver_type, method_name) -> declared return type`. We keep the full
/// `TypeExpr` (not just the summary) so we can tell whether the return is
/// `Future<X>` and recover `X` for further propagation.
///
/// **Implicit `Future<…>` wrapping**: when a function is declared
/// `extern Wasm.async(…)`, its declared return `T` is recorded here as
/// `Future<T>`. The user never writes `Future<T>` directly — it's an
/// internal compile-time wrapping that drives the auto-await transform.
/// At call sites, the receiver's static type is `Future<T>`, which
/// triggers `Expr::Await` insertion, which in turn marks the enclosing
/// function suspending via `async_analysis::analyse`.
type MethodReturnMap = HashMap<(String, String), TypeExpr>;

fn collect_method_returns(module: &Module) -> MethodReturnMap {
    let mut out: MethodReturnMap = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            // Implicitly wrap the return type in `Future<…>` for extern
            // async functions — see the type alias's doc-comment above.
            let ret = if func
                .extern_wasm
                .as_ref()
                .map(|e| e.is_async)
                .unwrap_or(false)
            {
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

fn wrap_future(ty: TypeExpr) -> TypeExpr {
    let span = ty.span();
    TypeExpr::Named {
        name: "Future".to_string(),
        generics: vec![ty],
        span,
    }
}

fn transform_block(block: &mut Block, returns: &MethodReturnMap) {
    for expr in &mut block.exprs {
        transform_expr(expr, returns);
    }
}

fn transform_expr(expr: &mut Expr, returns: &MethodReturnMap) {
    // Recurse first so inner subtrees are rewritten before we use their
    // (post-transform) static types to decide what to do at this node.
    match expr {
        Expr::MethodCall { receiver, args, .. } => {
            transform_expr(receiver, returns);
            for a in args {
                transform_expr(a, returns);
            }
        }
        Expr::Constructor { args, .. } => {
            for a in args {
                transform_expr(a, returns);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            transform_expr(scrutinee, returns);
            for arm in arms {
                transform_arm(arm, returns);
            }
        }
        Expr::Try { inner, .. } => transform_expr(inner, returns),
        Expr::Lambda { body, .. } => transform_block(body, returns),
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                transform_expr(f, returns);
            }
        }
        Expr::FieldAccess { receiver, .. } => transform_expr(receiver, returns),
        Expr::Await { inner, .. } => transform_expr(inner, returns),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. }
        | Expr::JsonLit { .. } => {}
    }

    // After the recursive walk, look for the auto-await opportunity at *this*
    // node: a method call whose receiver is `Future<T>`.
    if let Expr::MethodCall { receiver, .. } = expr {
        let recv_raw = infer_raw_type(receiver, returns);
        if is_future(&recv_raw) {
            // Wrap `receiver` in `Expr::Await(receiver)`. The codegen treats
            // `Expr::Await` as a pass-through today; `async_analysis` uses
            // it as the seed for the bottom-up fixpoint.
            let span = receiver.span();
            let inner = std::mem::replace(receiver.as_mut(), Expr::IntLit { value: 0, span });
            **receiver = Expr::Await {
                inner: Box::new(inner),
                span,
            };
        }
    }
}

fn transform_arm(arm: &mut MatchArm, returns: &MethodReturnMap) {
    transform_block(&mut arm.body, returns);
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
                    span,
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
