//! String interning and per-function local scratch layout.
//!
//! [`StringTable`] packs literal bytes and hands back (offset, len) pairs;
//! [`LocalScope`] maps Canon parameter names to wasm local indices and
//! names the fixed scratch locals appended after a function's params.
use super::*;

/// Maps Canon parameter names to their local variable index + repr.
///
/// Extra locals (indices after params, declared via `extra_locals_decl()`):
///   pc+0, pc+1  (i32): rptr, rlen   — for Str match results
///   pc+2        (i32): rbool         — for I32/Ptr match results
///   pc+3        (i32): tmp_i32       — general scratch i32
///   pc+4        (i64): tmp_i64       — general scratch i64
///   pc+5        (i32): alloc_ptr     — result of $alloc
///   pc+6        (i32): tmp_i32_b     — second scratch i32
///   pc+7, pc+8  (i32): arm_payload_ptr (+1) — bound arm payload
///                       (outermost dispatch only; a dispatch nested
///                       inside an arm body binds into the tail pairs
///                       from pc+32 on, one pair per extra level)
///   pc+9, pc+10 (i32): str_scratch_ptr (+1) — string-builtin scratch
///   pc+11..pc+18 (i32): par_subtask_a/b, par_retarea_a/b, par_set,
///                       par_event_ptr, par_seen_a/b — parallel/race state.
///                       Eight locals, kept always-on so the wasm validator
///                       sees a stable local layout regardless of whether
///                       the function actually uses concurrency combinators.
///                       Cost: ~32 bytes of dead locals per non-using
///                       function, which is fine.
#[derive(Clone, Default)]
pub(super) struct LocalScope {
    pub(super) vars: HashMap<String, (u32, Ty)>,
    pub(super) param_count: u32, // first extra-local index
    /// How many dispatches enclose the code being compiled. Selects
    /// which `arm_payload_ptr` pair a bound payload lives in, so an
    /// inner dispatch can't overwrite the name an outer arm bound.
    pub(super) arm_depth: u32,
}

impl LocalScope {
    pub(super) fn empty() -> Self {
        LocalScope {
            vars: HashMap::new(),
            param_count: 0,
            arm_depth: 0,
        }
    }
    pub(super) fn rptr(&self) -> u32 {
        self.param_count
    }
    pub(super) fn rlen(&self) -> u32 {
        self.param_count + 1
    }
    pub(super) fn rbool(&self) -> u32 {
        self.param_count + 2
    }
    pub(super) fn tmp_i32(&self) -> u32 {
        self.param_count + 3
    }
    pub(super) fn tmp_i64(&self) -> u32 {
        self.param_count + 4
    }
    pub(super) fn alloc_ptr(&self) -> u32 {
        self.param_count + 5
    }
    pub(super) fn tmp_i32_b(&self) -> u32 {
        self.param_count + 6
    }
    /// Adjacent pair of i32s holding the (ptr, len) of a string payload
    /// bound inside a match arm. Adjacency matters: `push_local` for
    /// `Ty::Str` pushes `LocalGet(idx)` followed by `LocalGet(idx + 1)`,
    /// so the two slots must sit at consecutive indices.
    ///
    /// One pair per dispatch depth: a tokenizer's `* Name` arm sits
    /// inside a `* Cons` arm and still reads `Cons.Tail`, so a single
    /// shared pair would have the inner bind erase the outer name. The
    /// outermost dispatch keeps the historical fixed slot, so a
    /// function whose dispatches never nest declares no extra locals;
    /// deeper levels take the tail pairs `extra_locals_decl` reserves
    /// from `max_arm_depth`.
    pub(super) fn arm_payload_ptr(&self) -> u32 {
        match self.arm_depth {
            0 => self.param_count + 7,
            d => self.param_count + 37 + 2 * (d - 1),
        }
    }
    /// Adjacent pair of i32s reserved as scratch for string-shaped
    /// builtins (`concat`, `substring`, …) that need to stash a
    /// `(ptr, len)` pair across an `$alloc` + copy loop. Kept distinct
    /// from `arm_payload_ptr` so a builtin call inside a dispatch arm
    /// body can't corrupt the bound payload — see the
    /// "Heap allocations inside `Ok`/`Err` dispatch arm bodies" gap in
    /// CLAUDE.md.
    pub(super) fn str_scratch_ptr(&self) -> u32 {
        self.param_count + 9
    }

    // ── Parallel / race scratch locals ───────────────────────────────
    //
    // Eight i32s used by `compile_parallel` and `compile_race` to thread
    // the multi-subtask wait state through the emitted instruction stream.
    // Kept in a contiguous block from `pc+11..pc+18` so the wasm validator
    // can statically prove they exist regardless of the call site.
    pub(super) fn par_subtask_a(&self) -> u32 {
        self.param_count + 11
    }
    pub(super) fn par_subtask_b(&self) -> u32 {
        self.param_count + 12
    }
    pub(super) fn par_retarea_a(&self) -> u32 {
        self.param_count + 13
    }
    pub(super) fn par_retarea_b(&self) -> u32 {
        self.param_count + 14
    }
    pub(super) fn par_set(&self) -> u32 {
        self.param_count + 15
    }
    pub(super) fn par_event_ptr(&self) -> u32 {
        self.param_count + 16
    }
    pub(super) fn par_seen_a(&self) -> u32 {
        self.param_count + 17
    }
    pub(super) fn par_seen_b(&self) -> u32 {
        self.param_count + 18
    }

    /// Single i32 scratch holding a store-target address for the
    /// duration of one `store_payload_at_offset` string store. Only
    /// ever live between adjacent instructions (never across a nested
    /// `compile_expr`), so it can't be clobbered by nested
    /// constructors the way `alloc_ptr` can.
    pub(super) fn addr_scratch(&self) -> u32 {
        self.param_count + 19
    }

    /// f64 scratch, the floating-point sibling of `tmp_i64`. Kept
    /// separate because wasm locals are monomorphically typed — an f64
    /// value cannot pass through the i64-typed `tmp_i64` without an
    /// explicit reinterpret, and mixing the two was exactly the bug
    /// that made `Float` union payloads emit invalid wasm.
    pub(super) fn tmp_f64(&self) -> u32 {
        self.param_count + 20
    }

    /// i64 local holding the current element while a `list.map` lambda
    /// body runs. The lambda's parameter name binds to this slot.
    /// Caveat: a `.map` nested inside another `.map`'s lambda body
    /// reuses the slot, clobbering the outer element — acceptable
    /// until real iteration state lands.
    pub(super) fn map_elem_i64(&self) -> u32 {
        self.param_count + 21
    }

    /// Adjacent i32 pair holding the current `(ptr, len)` string
    /// element during `list.map`, and doubling as the result stash
    /// between the lambda body finishing and the store into the
    /// destination list. Same nesting caveat as `map_elem_i64`.
    pub(super) fn map_elem_ptr(&self) -> u32 {
        self.param_count + 22
    }

    /// Adjacent i32 pair holding the scrutinee `(ptr, len)` across a
    /// string literal-dispatch compare chain (`* ("/notes") -> …`).
    /// Kept distinct from `arm_payload_ptr` and the eq-compare scratch
    /// (`rptr`/`rbool`/`tmp_i32`/`tmp_i32_b`) so each successive
    /// compare — and the scrutinee binding inside arm bodies — reads
    /// an unclobbered value. Still one pair per function, though: a
    /// literal dispatch nested inside another literal dispatch's arm
    /// body reuses it. `arm_payload_ptr` shed that caveat by taking a
    /// pair per depth; this one has not needed to yet.
    pub(super) fn lit_scrut_ptr(&self) -> u32 {
        self.param_count + 24
    }

    /// i64 sibling of `lit_scrut_ptr` for `Int` literal dispatch.
    pub(super) fn lit_scrut_i64(&self) -> u32 {
        self.param_count + 26
    }

    /// Second f64 scratch. `Float.rem` needs both operands available
    /// twice (`a - trunc(a/b) * b`), and wasm has no stack dup — the
    /// pair of f64 locals holds `a`/`b` across the sequence.
    pub(super) fn tmp_f64_b(&self) -> u32 {
        self.param_count + 27
    }

    /// Adjacent i32 pair holding a binding dispatch's scrutinee — a
    /// heap pointer in the first slot, or a `(ptr, len)` string/list
    /// pair across both. Dedicated (not shared with `lit_scrut_ptr`)
    /// so a binding dispatch inside a literal-dispatch arm body can't
    /// clobber the outer scrutinee. Same single-slot nesting caveat as
    /// `lit_scrut_ptr` for binding-inside-binding.
    pub(super) fn bind_scrut_ptr(&self) -> u32 {
        self.param_count + 28
    }

    /// i64 sibling of `bind_scrut_ptr` for `Int` scrutinees.
    pub(super) fn bind_scrut_i64(&self) -> u32 {
        self.param_count + 30
    }

    /// f64 sibling of `bind_scrut_ptr` for `Float` scrutinees.
    pub(super) fn bind_scrut_f64(&self) -> u32 {
        self.param_count + 31
    }

    /// f64 sibling of `map_elem_i64` for a `Float` element.
    pub(super) fn map_elem_f64(&self) -> u32 {
        self.param_count + 32
    }

    /// The `list.fold` accumulator across iterations: an i64, an f64,
    /// or an adjacent i32 pair (one pointer / `Bool`, or a `(ptr, len)`
    /// string / list), by the accumulator's repr. Same nesting caveat as
    /// `map_elem_i64`: a fold inside a fold's lambda reuses the slots.
    pub(super) fn fold_acc_i64(&self) -> u32 {
        self.param_count + 33
    }
    pub(super) fn fold_acc_f64(&self) -> u32 {
        self.param_count + 34
    }
    pub(super) fn fold_acc_ptr(&self) -> u32 {
        self.param_count + 35
    }
}

/// Deepest chain of dispatches nested inside one another's arm bodies
/// (`0` when the block holds none, `1` for a flat dispatch). Each level
/// past the first needs its own `arm_payload_ptr` pair — see that
/// accessor — so this is what sizes a function's tail locals.
pub(super) fn max_arm_depth(block: &Block) -> u32 {
    block.exprs.iter().map(expr_arm_depth).max().unwrap_or(0)
}

fn expr_arm_depth(expr: &Expr) -> u32 {
    let deepest = |es: &[Expr]| es.iter().map(expr_arm_depth).max().unwrap_or(0);
    match expr {
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let inner = arms
                .iter()
                .map(|a| max_arm_depth(&a.body))
                .max()
                .unwrap_or(0);
            expr_arm_depth(scrutinee).max(1 + inner)
        }
        Expr::FieldAccess { receiver, .. } => expr_arm_depth(receiver),
        Expr::MethodCall { receiver, args, .. } => expr_arm_depth(receiver).max(deepest(args)),
        Expr::Constructor { args, .. } => deepest(args),
        Expr::ProductValue { fields, .. } => deepest(fields),
        Expr::Lambda { body, .. } => max_arm_depth(body),
        Expr::Try { inner, .. } => expr_arm_depth(inner),
        Expr::JsonLit { parts, .. } => parts
            .iter()
            .map(|p| match p {
                crate::ast::JsonLitPart::Interp(e) => expr_arm_depth(e),
                _ => 0,
            })
            .max()
            .unwrap_or(0),
        Expr::HtmlLit { parts, .. } => parts
            .iter()
            .map(|p| match p {
                crate::ast::HtmlLitPart::Interp(e) => expr_arm_depth(e),
                _ => 0,
            })
            .max()
            .unwrap_or(0),
        Expr::FormatLit { parts, .. } => parts
            .iter()
            .map(|p| match p {
                crate::ast::FormatLitPart::Interp(e) => expr_arm_depth(e),
                _ => 0,
            })
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

/// Local declarations appended after the function params. `arm_depth`
/// is the function body's `max_arm_depth`; every level past the first
/// takes one more `(ptr, len)` pair at the tail.
pub(super) fn extra_locals_decl(arm_depth: u32) -> Vec<(u32, ValType)> {
    let mut decl = vec![
        (4, ValType::I32), // rptr, rlen, rbool, tmp_i32
        (1, ValType::I64), // tmp_i64
        (2, ValType::I32), // alloc_ptr, tmp_i32_b
        (2, ValType::I32), // arm_payload_ptr, arm_payload_ptr + 1 (len)
        (2, ValType::I32), // str_scratch_ptr, str_scratch_ptr + 1 (len)
        (8, ValType::I32), // par_subtask_a/b, par_retarea_a/b, par_set,
        // par_event_ptr, par_seen_a/b (parallel/race state)
        (1, ValType::I32), // addr_scratch (store-target address)
        (1, ValType::F64), // tmp_f64
        (1, ValType::I64), // map_elem_i64 (list.map current element)
        (2, ValType::I32), // map_elem_ptr, map_elem_ptr + 1 (len)
        (2, ValType::I32), // lit_scrut_ptr, lit_scrut_ptr + 1 (len)
        (1, ValType::I64), // lit_scrut_i64 (Int literal-dispatch scrutinee)
        (1, ValType::F64), // tmp_f64_b (Float.rem second operand)
        (2, ValType::I32), // bind_scrut_ptr, bind_scrut_ptr + 1 (len)
        (1, ValType::I64), // bind_scrut_i64 (Int binding-dispatch scrutinee)
        (1, ValType::F64), // bind_scrut_f64 (Float binding-dispatch scrutinee)
        (1, ValType::F64), // map_elem_f64 (list.map current Float element)
        (1, ValType::I64), // fold_acc_i64 (list.fold accumulator)
        (1, ValType::F64), // fold_acc_f64
        (2, ValType::I32), // fold_acc_ptr, fold_acc_ptr + 1 (len)
    ];
    let extra_pairs = arm_depth.saturating_sub(1);
    if extra_pairs > 0 {
        decl.push((2 * extra_pairs, ValType::I32));
    }
    decl
}

// ── Function table ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(super) struct FuncInfo {
    pub(super) func_idx: u32,
    pub(super) type_idx: u32,
    pub(super) result_ty: Ty,
    /// The callee's declared input components, by type name, in
    /// parameter order — the receiver first when it is a runtime value
    /// (a `Self`-renamed constructor's receiver is a type marker, so it
    /// contributes nothing). Call sites bind their inputs to these by
    /// type rather than by position; see `commutative_order`.
    pub(super) input_types: Vec<String>,
    /// `Some(shape)` when this is an `extern Wasm` whose canonical-ABI
    /// lowering uses indirect return. Call sites allocate a return area,
    /// pass its pointer as an extra last arg, and decode the result
    /// according to `shape` after the call.
    pub(super) indirect_return: Option<IndirectReturnShape>,
    /// Per-component-parameter conversion flags for extern functions
    /// (empty for user body functions): true where the WIT-informed
    /// lowering narrowed Canon's i64 `Int` slot to core i32, so the
    /// call site must `i32.wrap_i64` that argument.
    pub(super) narrow_params: Vec<bool>,
    /// `Some(signed)` when the extern's result narrowed from i64 to
    /// i32 — the call site extends back to Canon's i64.
    pub(super) narrow_result_signed: Option<bool>,
    /// True for an extern with a WIT bare `result;` return: the call
    /// site receives one i32 discriminant directly and re-shapes it
    /// into a Canon `Result` struct (flipping 0=ok/1=err into Canon's
    /// Err=0/Ok=1 tags). Always false for user body functions.
    pub(super) bare_result: bool,
    /// `true` for `extern Wasm.async` functions. Call sites use the
    /// component-model async-lower calling convention: the args go flat
    /// on the stack (as in sync), but the function returns an `i32`
    /// status code instead of the result. A ret-area pointer is
    /// appended to the params when the function has a result; the result
    /// is read out of the ret-area after the call. See
    /// `emit_async_call` for the full sequence.
    pub(super) is_async: bool,
}

// ── String table ──────────────────────────────────────────────────────────────

pub(super) struct StringTable {
    pub(super) data: Vec<u8>,
    pub(super) offsets: HashMap<String, (u32, u32)>, // content → (abs_offset, len)
}

impl StringTable {
    pub(super) fn new() -> Self {
        StringTable {
            data: Vec::new(),
            offsets: HashMap::new(),
        }
    }
    pub(super) fn intern(&mut self, s: &str) -> (u32, u32) {
        if let Some(&p) = self.offsets.get(s) {
            return p;
        }
        let offset = MEM_STR_START + self.data.len() as u32;
        let len = s.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.offsets.insert(s.to_string(), (offset, len));
        (offset, len)
    }
}

// ── Main compiler struct ──────────────────────────────────────────────────────
