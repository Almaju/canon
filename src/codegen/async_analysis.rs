//! Async inference — bottom-up fixpoint over the call graph.
//!
//! A Canon function is **suspending** (i.e. compiled as `async` at the
//! Component Model boundary) if any of the following hold:
//!
//!   1. It is declared `extern Wasm.async("…")`.
//!   2. Its body uses an `Expr::Await` (inserted by `auto_await::transform`
//!      whenever a `Future<T>` is consumed where `T` is expected).
//!   3. It transitively calls a function that is suspending.
//!
//! The set is computed bottom-up by a textbook worklist fixpoint: seed with
//! the direct triggers (1–2), then propagate to callers until stable.
//!
//! This module is **target-agnostic**: it only walks the AST. The binary
//! itself doesn't need the set — `component::wrap` keys every per-import
//! async decision on `ExternImport::is_async`, and `run` is always lifted
//! async-stackful (see `emit_async_call` / `compile_parallel` /
//! `compile_race` for the guest-side call sequences). The set's one
//! consumer is `generate_wit`, which surfaces it as the async-inference
//! summary in the emitted `.wit`.

use std::collections::{HashMap, HashSet};

use crate::ast::{Block, Expr, FunctionDef, Item, Module};

/// A function-table key matching the codegen's:
/// `(receiver_type_name_if_method, function_name)`.
pub type FuncKey = (Option<String>, String);

/// The set of functions inferred to be suspending. Look up by
/// `(receiver, name)` — free functions use `(None, name)`.
#[derive(Debug, Default, Clone)]
pub struct AsyncSet {
    inner: HashSet<FuncKey>,
}

impl AsyncSet {
    pub fn contains(&self, key: &FuncKey) -> bool {
        self.inner.contains(key)
    }

    /// Iterate every `(receiver, name)` pair that has been inferred to
    /// suspend. Used by tests; the codegen itself only needs `contains`.
    pub fn iter(&self) -> impl Iterator<Item = &FuncKey> {
        self.inner.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Run the bottom-up fixpoint and return the set of suspending functions.
pub fn analyse(module: &Module) -> AsyncSet {
    // ── 1. Index every function by its `(receiver, name)` key.
    let mut funcs: HashMap<FuncKey, &FunctionDef> = HashMap::new();
    for item in &module.items {
        if let Item::Function(func) = item {
            let key = (
                func.receiver.as_ref().map(|r| r.name.clone()),
                func.name.name.clone(),
            );
            funcs.insert(key, func);
        }
    }

    // ── 2. Seed the async set with direct triggers.
    let mut async_set: HashSet<FuncKey> = HashSet::new();
    for (key, func) in &funcs {
        if is_direct_trigger(func) {
            async_set.insert(key.clone());
        }
    }

    // ── 3. Pre-compute each function's call list (the set of
    // `(receiver, name)` pairs it invokes). Calls to functions not in
    // `funcs` are filtered out — they refer to externs or methods on
    // built-in types, which the seed pass already accounted for via
    // `extern_wasm.is_async`.
    let mut callees: HashMap<FuncKey, Vec<FuncKey>> = HashMap::new();
    for (key, func) in &funcs {
        let mut out = Vec::new();
        collect_calls_block(&func.body, &mut out);
        // Keep duplicates out, and drop calls we can't resolve. We don't
        // bother resolving aliases here — the codegen's `func_table`
        // collapses aliases at lookup time, so propagation through aliases
        // happens when the codegen calls `is_suspending`.
        out.sort();
        out.dedup();
        callees.insert(key.clone(), out);
    }

    // ── 4. Worklist fixpoint: a function becomes async iff it calls one.
    let mut changed = true;
    while changed {
        changed = false;
        for (caller, calls) in &callees {
            if async_set.contains(caller) {
                continue;
            }
            for callee in calls {
                if async_set.contains(callee) {
                    async_set.insert(caller.clone());
                    changed = true;
                    break;
                }
            }
        }
    }

    AsyncSet { inner: async_set }
}

/// Direct trigger: the function is itself async without needing a caller.
fn is_direct_trigger(func: &FunctionDef) -> bool {
    // (1) extern Wasm.async
    if let Some(ext) = &func.extern_wasm {
        if ext.is_async {
            return true;
        }
    }
    // (2) body contains Expr::Await.
    body_has_async_trigger(&func.body)
}

fn body_has_async_trigger(block: &Block) -> bool {
    block.exprs.iter().any(expr_has_async_trigger)
}

fn expr_has_async_trigger(expr: &Expr) -> bool {
    match expr {
        Expr::Await { .. } => true,
        Expr::MethodCall { receiver, args, .. } => {
            expr_has_async_trigger(receiver) || args.iter().any(expr_has_async_trigger)
        }
        Expr::Constructor { args, .. } => args.iter().any(expr_has_async_trigger),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_has_async_trigger(scrutinee)
                || arms.iter().any(|a| body_has_async_trigger(&a.body))
        }
        Expr::Try { inner, .. } => expr_has_async_trigger(inner),
        Expr::Lambda { body, .. } => body_has_async_trigger(body),
        Expr::ProductValue { fields, .. } => fields.iter().any(expr_has_async_trigger),
        Expr::FieldAccess { receiver, .. } => expr_has_async_trigger(receiver),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::JsonLit { .. }
        | Expr::HtmlLit { .. }
        | Expr::FormatLit { .. } => false,
    }
}

/// Walk a block and collect every method/constructor call as a
/// `(receiver_type, name)` key the call-graph propagation can match against
/// the indexed functions. This is intentionally *syntactic* — it doesn't
/// resolve receivers through aliases, generics, or capability shadowing.
/// The result is an over-approximation suitable for fixpoint propagation:
/// false positives just mark extra functions async, which is harmless.
fn collect_calls_block(block: &Block, out: &mut Vec<FuncKey>) {
    for e in &block.exprs {
        collect_calls_expr(e, out);
    }
}

fn collect_calls_expr(expr: &Expr, out: &mut Vec<FuncKey>) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let recv_name = receiver_type_name(receiver);
            out.push((recv_name, method.name.clone()));
            collect_calls_expr(receiver, out);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        Expr::Constructor { name, args, .. } => {
            // Free functions used as constructors (e.g. `Now()`) live under
            // key `(None, name)` in the codegen's func table. Self-renamed
            // constructors live under `(Some(Type), "Self")`. We push both
            // candidates so the fixpoint sees the match either way; the
            // worklist propagation step filters to functions that actually
            // exist.
            out.push((None, name.name.clone()));
            out.push((Some(name.name.clone()), "Self".to_string()));
            // A constructor-shaped call `foo(Filesystem)` may also resolve
            // to a method `foo` on the *argument's* type — this is how
            // capability-receiver functions like
            // `read = (Filesystem) -> Result<…>` are invoked: the user
            // writes `read(Filesystem)` but the declared receiver is
            // `Filesystem`. Mirror the codegen's commutative dispatch by
            // also probing each arg's syntactic type name. Over-approximate
            // is fine: the worklist drops keys with no matching function.
            for a in args {
                if let Some(arg_ty) = receiver_type_name(a) {
                    out.push((Some(arg_ty), name.name.clone()));
                }
                collect_calls_expr(a, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_calls_expr(scrutinee, out);
            for arm in arms {
                collect_calls_block(&arm.body, out);
            }
        }
        Expr::Try { inner, .. } => collect_calls_expr(inner, out),
        Expr::Lambda { body, .. } => collect_calls_block(body, out),
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                collect_calls_expr(f, out);
            }
        }
        Expr::FieldAccess { receiver, .. } => collect_calls_expr(receiver, out),
        Expr::Await { inner, .. } => collect_calls_expr(inner, out),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::JsonLit { .. }
        | Expr::HtmlLit { .. }
        | Expr::FormatLit { .. } => {}
    }
}

/// Best-effort syntactic guess at the receiver's Canon type *name*. Used
/// only for call-graph keying; full type information is unavailable at
/// this AST-only layer. Conservative — returns `None` (free-function key)
/// when the shape isn't obvious.
fn receiver_type_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.name.clone()),
        Expr::Constructor { name, .. } => Some(name.name.clone()),
        // For chained `.foo().bar()` the immediate receiver type is the
        // return type of `.foo()`, which we don't track here. The fixpoint
        // propagation step compensates by also propagating through the
        // `(None, name)` and `(Some(_), name)` keys we record.
        _ => None,
    }
}
