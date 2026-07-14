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
}

impl LocalScope {
    pub(super) fn empty() -> Self {
        LocalScope {
            vars: HashMap::new(),
            param_count: 0,
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
    pub(super) fn arm_payload_ptr(&self) -> u32 {
        self.param_count + 7
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
    /// an unclobbered value. Same single-slot nesting caveat as
    /// `arm_payload_ptr`: a literal dispatch nested inside another
    /// literal dispatch's arm body reuses the pair.
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
}

/// Local declarations appended after the function params.
pub(super) fn extra_locals_decl() -> Vec<(u32, ValType)> {
    vec![
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
    ]
}

// ── Function table ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(super) struct FuncInfo {
    pub(super) func_idx: u32,
    pub(super) type_idx: u32,
    pub(super) result_ty: Ty,
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
    pub(super) fn get(&self, s: &str) -> Option<(u32, u32)> {
        self.offsets.get(s).copied()
    }
}

// ── Main compiler struct ──────────────────────────────────────────────────────
