//! Expression and method-call compilation, plus the intrinsic function
//! builders (print helpers, alloc, list ops) and the CLI-world module
//! assembler. This is the heart of codegen: it walks the AST and emits
//! core WASM for every Canon construct, handing constructors
//! (`construct`), dispatch (`dispatch`), the builtin vocabulary
//! (`builtins`), list lambdas (`lists`) and effects (`effects`) to their
//! own modules.
use super::*;

/// Walks the module's items, collects every `extern Wasm` function, parses the
/// path, derives the WASM signature, and assigns each a function index. The
/// resulting list is sorted by `(core_namespace, fn_name)` so the output is
/// deterministic across runs (matching Canon's "alphabetical" ethos).
/// The `(start, end)` bound expressions of a `substring`/`slice` call.
/// Canonically the bounds arrive as a `From * To` product, which is
/// *positionless*: the start is whichever component is `From(…)` and the
/// end whichever is `To(…)`, regardless of written order. Two positional
/// args are still accepted during migration (start, then end).
pub(super) fn substring_bounds(args: &[Expr]) -> Option<(&Expr, &Expr)> {
    fn ctor_name(e: &Expr) -> Option<&str> {
        match e {
            Expr::Constructor { name, .. } => Some(name.name.as_str()),
            _ => None,
        }
    }
    match args {
        [Expr::ProductValue { fields, .. }] if fields.len() == 2 => {
            let (a, b) = (&fields[0], &fields[1]);
            if ctor_name(a) == Some("To") || ctor_name(b) == Some("From") {
                Some((b, a))
            } else {
                Some((a, b))
            }
        }
        [a, b] => Some((a, b)),
        _ => None,
    }
}

/// Emits a load of a scalar primitive at `offset` from the address on
/// top of the stack, widened to Canon's 8-byte value representation:
/// integer widths and `bool`/`char` widen to i64 (sign- or
/// zero-extending per the WIT signedness), `f32` promotes to f64,
/// `u64`/`s64`/`f64` load directly. Leaves one i64 (or f64 for floats).
pub(super) fn emit_prim_load_widen(
    f: &mut Function,
    prim: wasm_encoder::PrimitiveValType,
    offset: u64,
) {
    use wasm_encoder::PrimitiveValType as P;
    let mem = |align: u32| MemArg {
        offset,
        align,
        memory_index: 0,
    };
    match prim {
        P::U64 | P::S64 => {
            f.instruction(&Instruction::I64Load(mem(3)));
        }
        P::F64 => {
            f.instruction(&Instruction::F64Load(mem(3)));
        }
        P::F32 => {
            f.instruction(&Instruction::F32Load(mem(2)));
            f.instruction(&Instruction::F64PromoteF32);
        }
        P::S32 => {
            f.instruction(&Instruction::I32Load(mem(2)));
            f.instruction(&Instruction::I64ExtendI32S);
        }
        P::S16 => {
            f.instruction(&Instruction::I32Load16S(mem(1)));
            f.instruction(&Instruction::I64ExtendI32S);
        }
        P::S8 => {
            f.instruction(&Instruction::I32Load8S(mem(0)));
            f.instruction(&Instruction::I64ExtendI32S);
        }
        P::U32 | P::Char => {
            f.instruction(&Instruction::I32Load(mem(2)));
            f.instruction(&Instruction::I64ExtendI32U);
        }
        P::U16 => {
            f.instruction(&Instruction::I32Load16U(mem(1)));
            f.instruction(&Instruction::I64ExtendI32U);
        }
        P::U8 | P::Bool => {
            f.instruction(&Instruction::I32Load8U(mem(0)));
            f.instruction(&Instruction::I64ExtendI32U);
        }
        // `string` and every compound never reach here — the shape
        // classifiers only produce scalar primitives.
        _ => {
            f.instruction(&Instruction::I64Const(0));
        }
    }
}

pub(super) fn arm_type_name(arm: &MatchArm) -> Option<&str> {
    if let TypeExpr::Named { name, .. } = &arm.param_ty {
        Some(name.as_str())
    } else {
        None
    }
}

/// True when this arm's pattern names `variant_name`. Used by the
/// N-variant dispatch to pair each variant tag with the arm that
/// handles it. Matches by exact name only — the 2-variant fast path
/// has extra fallbacks (`Some`/`Ok`/`True` for the `1` tag,
/// `None`/`Err`/`False` for `0`) because the built-in unions don't
/// always go through `union_variants`. For user-defined N-variant
/// unions, the variant names are exactly what the user wrote, so a
/// plain match is enough.
pub(super) fn arm_matches_variant(arm: &MatchArm, variant_name: &str) -> bool {
    arm_type_name(arm) == Some(variant_name)
}

/// Newtype field access: for `A = B`, `aValue.B` returns the underlying
/// `B` value with the same wire representation but retyped. Returns the
/// post-unwrap `Ty` to leave on the stack, or `None` when the field name
/// doesn't match the newtype's underlying type (in which case the caller
/// falls back to drop-and-Unit for real products).
///
/// Handles string-shaped newtypes (the common case): an `A = String`
/// value lives on the stack as `(ptr, len)`, and `.String` keeps both
/// values on the stack while changing the static type from
/// `Ty::NamedStr("A")` to `Ty::Str`. Numeric newtypes don't currently
/// carry their alias name through the codegen, so they need no work
/// here — the field-access expression is already a no-op at the wasm
/// level.
pub(super) fn newtype_unwrap_ty(recv_ty: &Ty, field: &str) -> Option<Ty> {
    match (recv_ty, field) {
        (Ty::NamedStr(_), "String") => Some(Ty::Str),
        (Ty::Str, "String") => Some(Ty::Str), // idempotent
        // Idempotent unwrap for primitive payloads. `ParsePos.Int`
        // (where `ParsePos = Int`) is a no-op at the wasm level —
        // the value on the stack is already an i64 — but the
        // surface-level type changes from the newtype to the base.
        // Matches the way `Ty::Str` handles `.String`.
        (Ty::I64, "Int") => Some(Ty::I64),
        (Ty::F64, "Float") => Some(Ty::F64),
        (Ty::I32, "Bool") => Some(Ty::I32),
        // Idempotent unwrap through a product alias: `X.Instant` where
        // `X = Instant` and the compiled value is already the `Instant`
        // struct pointer (a string-anchored binding's record decode
        // pushes the product it aliases). Static-type retype only.
        (Ty::NamedPtr(product), _) if product == field => Some(Ty::NamedPtr(product.clone())),
        _ => None,
    }
}

/// Returns the discriminant tag for this arm, based on known variant names.
pub(super) fn arm_tag(arm: &MatchArm) -> Option<u32> {
    match arm_type_name(arm)? {
        "False" | "None" | "Err" => Some(0),
        "True" | "Some" | "Ok" => Some(1),
        _ => None,
    }
}

impl<'m> WasmGen<'m> {
    /// Build the `fn_list_to_json_array` helper function.
    ///
    /// Core signature: `(list_ptr: i32, list_len: i32) -> (i32, i32)`
    /// returning `(out_ptr, out_len)` of a freshly-allocated string
    /// containing `[elem0,elem1,…,elemN]`. Element slots in the list
    /// follow the storage convention of `build_list_literal`:
    /// `(i32 ptr, i32 len)` at offsets 0 / 4 of an 8-byte slot.
    ///
    /// Algorithm: two passes. Pass 1 sums the byte budget
    /// (`2 + sum(elem_len) + max(0, len-1)`), so we allocate the
    /// output buffer exactly once. Pass 2 fills it by walking the
    /// list, writing `[`, comma separators, each element body, and
    /// finally `]`.
    pub(super) fn build_list_to_json_array(&self) -> Function {
        // Locals declared in order. Indices follow the params (2 i32s),
        // so the first local is index 2.
        //   0: list_ptr  (param)
        //   1: list_len  (param)
        //   2: total     (output size accumulator / final length)
        //   3: i         (loop counter)
        //   4: out_ptr   (allocated buffer)
        //   5: out_pos   (write offset within buffer)
        //   6: elem_ptr  (per-iteration element pointer)
        //   7: elem_len  (per-iteration element length)
        //   8: slot_addr (list_ptr + i*8, reused twice per iteration)
        let mut f = Function::new([(7, ValType::I32)]);

        // ── Pass 1: total = 2 + sum(elem_len) + max(0, len-1) ─────────────────
        // Start total with 2 (for `[` and `]`).
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::LocalSet(2));
        // If len > 1, add (len - 1) for the commas.
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32GtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::End);
        // Loop: i = 0; while i < len: total += elem_len[i]; i++
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if i >= len: break
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::BrIf(1));
        // slot_addr = list_ptr + i*8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(8));
        // total += i32.load offset=4 (slot_addr) = elem_len
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(2));
        // i++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block

        // ── Allocate output buffer (size = total) ──────────────────────────────
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(4));

        // Write `[` at out_ptr+0
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(b'[' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // out_pos = 1
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(5));

        // ── Pass 2: walk elements, write to buffer ─────────────────────────
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if i >= len: break
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::BrIf(1));
        // if i > 0: write `,` at out_ptr+out_pos, out_pos++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32GtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(b',' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::End);
        // slot_addr = list_ptr + i*8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        // elem_ptr = i32.load(slot_addr+0)
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(6));
        // elem_len = i32.load(slot_addr+4)
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(7));
        // Inline byte-copy loop: copy elem_len bytes from elem_ptr to
        // out_ptr+out_pos. We use local 6 (elem_ptr) as src cursor,
        // local 8 as dst cursor (= out_ptr+out_pos initially), local 7
        // as remaining count.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // *dst = *src
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // dst++, src++, n--
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end inner loop
        f.instruction(&Instruction::End); // end inner block
                                          // out_pos += original elem_len (re-load from slot+4)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        // i++
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end outer loop
        f.instruction(&Instruction::End); // end outer block

        // Write `]` at out_ptr+out_pos
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(b']' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));

        // Return (out_ptr, total). `total` was the pass-1 budget,
        // which equals the final length — we wrote exactly that many
        // bytes.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::End);
        f
    }

    // ── Pre-passes ────────────────────────────────────────────────────────────

    /// `print_str(ptr: i32, len: i32) -> ()` — writes the byte buffer
    /// `[ptr .. ptr+len)` to stdout using the **native WASI Preview 3**
    /// canonical-ABI stream sequence. The resulting `.wasm` imports
    /// `wasi:cli/stdout` and nothing else — it is portable to any
    /// compliant Component Model runtime.
    ///
    /// ## Sequence emitted
    ///
    /// ```text
    ///   handles = stream.new<u8>()       ;; () -> i64 (low=reader, high=writer)
    ///   reader  = (i32) handles
    ///   writer  = (i32) (handles >> 32)
    ///   future  = write-via-stream(reader)  ;; (i32) -> i32
    ///   _       = stream.write<u8>(writer, ptr, len)
    ///   stream.drop-writable<u8>(writer)
    ///   future.drop-readable(future)
    /// ```
    ///
    /// - `stream.new<u8>` returns both ends packed in an i64; the reader
    ///   goes to the host, the writer stays with us.
    /// - `write-via-stream` is sync-lowered: it synchronously installs
    ///   the host-side pump and returns a future handle.
    /// - `stream.write` posts our bytes. For buffers smaller than
    ///   wasmtime-wasi's default capacity (~8 KiB) this completes
    ///   synchronously; we ignore the status code.
    /// - `stream.drop-writable` signals end-of-stream so the host
    ///   flushes to the OS file descriptor.
    /// - `future.drop-readable` discards the unused completion handle.
    ///
    /// All five canonical builtins are imported from `wasi:cli/stdout`
    /// under `wit-component`'s names for a function's stream and future
    /// builtins (`[stream-new-0]write-via-stream`, …).
    pub(super) fn build_print_str(&self) -> Function {
        // Locals declared in order:
        //   0..1 — params (ptr, len)
        //   2    — i64: packed handles from stream.new
        //   3..5 — i32 × 3: reader, writer, future
        let mut f = Function::new([(1, ValType::I64), (3, ValType::I32)]);

        // handles = stream.new<u8>()
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_NEW));
        f.instruction(&Instruction::LocalSet(2));

        // reader = (i32) handles                      (low 32 bits)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        // writer = (i32) (handles >> 32)              (high 32 bits)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        // future = write-via-stream(reader)
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::Call(FN_STDOUT_WRITE_VIA_STREAM));
        f.instruction(&Instruction::LocalSet(5));

        // stream.write<u8>(writer, ptr, len)  — status code dropped.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_WRITE));
        f.instruction(&Instruction::Drop);

        // stream.drop-writable<u8>(writer)
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::Call(FN_STDOUT_STREAM_DROP_WRITABLE));

        // future.drop-readable(future)
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::Call(FN_STDOUT_FUTURE_DROP_READABLE));

        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_str_cmp` helper: `(ptr1, len1, ptr2, len2) -> i32`
    /// returning -1 / 0 / 1 — byte-wise lexicographic order, with the
    /// shorter string ordering first on a shared prefix. Backs the
    /// `String.lt/le/gt/ge/ne` builtins (and, transitively, the
    /// alphabetical-order rule the language enforces everywhere else).
    pub(super) fn build_str_cmp(&self) -> Function {
        let load8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        // Locals: 0..3 = params (ptr1, len1, ptr2, len2);
        // 4 = i; 5 = b1; 6 = b2; 7 = minlen.
        let mut f = Function::new([(4, ValType::I32)]);
        // minlen = min(len1, len2)
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::Select);
        f.instruction(&Instruction::LocalSet(7));
        // for i in 0..minlen: compare bytes, early-return on mismatch.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Shared prefix — order by length: len1 < len2 → -1,
        // len1 > len2 → 1, equal → 0.
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32LtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(-1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_list_append` helper:
    /// `(list_ptr, count, slot: i64) -> (ptr, count)` — fresh list with
    /// `slot` in the last position. The call site packs the element
    /// (i64 verbatim; strings as `ptr | len << 32`, the
    /// `build_list_literal` slot layout).
    pub(super) fn build_list_append(&self) -> Function {
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        // Locals: 0 = ptr, 1 = count, 2 = slot (i64); 3 = new_ptr, 4 = j.
        let mut f = Function::new([(2, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_list_concat` helper:
    /// `(ptr1, count1, ptr2, count2) -> (ptr, count)`.
    pub(super) fn build_list_concat(&self) -> Function {
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        // Locals: 0..3 = params; 4 = new_ptr, 5 = j.
        let mut f = Function::new([(2, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(4));
        // First list.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Second list: j runs 0..count2, dst index = count1 + j.
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(mem64));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::End);
        f
    }

    /// Emit an in-place byte-copy loop reading from `scope.rbool()`
    /// (src) and writing to `scope.rptr()` (dst) for `scope.rlen()`
    /// (n) bytes. All three locals are modified by the loop (dst++,
    /// src++, n--), so they must be set up by the caller and not
    /// relied on after the call returns.
    ///
    /// A stand-in for `memory.copy` (bulk-memory proposal), which the
    /// playground's browser host and the web target would also have
    /// to accept; the call sites in `concat` won't change when it
    /// takes over.
    pub(super) fn emit_byte_copy_loop(&self, scope: &LocalScope, f: &mut Function) {
        // Wasm structured control: outer block (break target),
        // inner loop (continue target).
        //   block
        //     loop
        //       if n == 0: br 1  (out of block)
        //       store8(dst, load8(src))
        //       dst += 1; src += 1; n -= 1
        //       br 0  (continue loop)
        //     end
        //   end
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if (n == 0) break out of the block
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // store8(dst, load8(src))
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // dst++
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // src++
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // n--
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.rlen()));
        // continue loop
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block
    }

    /// $alloc(size: i32) → i32  — simple bump allocator.
    /// `$alloc(size: i32) -> i32` — bump-allocates `size` bytes from the
    /// shared `bump_ptr` global, rounding the returned pointer up to an
    /// 8-byte alignment. 8 is the strictest alignment the canonical ABI
    /// asks of us: extern-call return areas can carry u64/s64 fields
    /// (e.g. `wasi:clocks` records), and wasmtime validates the guest's
    /// ret-area pointer against the record's natural alignment. The
    /// host-side `cabi_realloc` uses the same heap and honours the
    /// caller's requested alignment explicitly.
    pub(super) fn build_alloc(&self) -> Function {
        // locals: 1 = aligned_ptr, 2 = new bump (allocation end)
        let mut f = Function::new([(2, ValType::I32)]);
        // aligned_ptr = (bump_ptr + 7) & ~7
        f.instruction(&Instruction::GlobalGet(GLOBAL_BUMP_PTR));
        f.instruction(&Instruction::I32Const(7));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(-8));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalTee(1));
        // bump_ptr = aligned_ptr + size
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(2));
        f.instruction(&Instruction::GlobalSet(GLOBAL_BUMP_PTR));
        // Grow memory when the allocation end passes the current
        // size. Long-lived instances (web apps dispatching events,
        // HTTP handlers) outlive the initial two pages; short-lived
        // CLI runs never hit this branch. A failed grow is ignored —
        // the subsequent store traps, which is the honest failure.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        // pages = (end - mem_bytes + 65535) >> 16
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(65535));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::MemoryGrow(0));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);
        // return aligned_ptr
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::End);
        f
    }

    /// Builds the `run` function exported by the core module.
    ///
    /// Inlines the body of `main` (Canon's entry point), drops any value
    /// it leaves on the stack, and delivers `result::ok` via
    /// `task.return(0)`. The core signature is `() -> ()` because the
    /// component-level `run` is lifted *async stackful*: results are
    /// returned through `task.return` rather than as a wasm return value.
    /// This is also what enables `extern Wasm.async` calls inside `main`
    /// to suspend on `waitable-set.wait` — wasmtime won't let a sync
    /// task block, so `run` itself has to be async-lifted.
    pub(super) fn build_start(&mut self) -> Function {
        // Locate the entry (`main`, the resolver's name for the
        // anonymous `Unit => Program`).
        let main_func: Option<FunctionDef> = self.ast.items.iter().find_map(|item| {
            if let Item::Function(func) = item {
                if func.name.name == "main" && func.receiver.is_none() {
                    return Some(func.clone());
                }
            }
            None
        });

        let scope = LocalScope::empty();
        let mut f = Function::new(extra_locals_decl(
            main_func
                .as_ref()
                .map_or(0, |func| max_arm_depth(&func.body)),
        ));
        // A `Result` / `Option` entry fails when its value is `Err` /
        // `None`: `?` inside it delivers the error result straight away
        // (`entry_fails`), and the value it ends on is checked the same
        // way. Either prints a string payload and exits 1.
        self.entry_fails = main_func
            .as_ref()
            .map(|func| self.resolve_return_ty(func))
            .is_some_and(|ret| matches!(ret.canon_name(), Some("Result" | "Option")));
        let result_ty = main_func
            .as_ref()
            .map(|func| self.compile_block_return(&func.body, &scope, &mut f));
        let entry_fails = std::mem::take(&mut self.entry_fails);
        // The error type the entry declares (`Result<Program, IoError>`),
        // for printing the payload it ends on.
        let declared_err = main_func
            .as_ref()
            .and_then(|func| match &func.return_ty {
                TypeExpr::Named { generics, .. } => generics.get(1).and_then(named_type_name),
                _ => None,
            })
            .unwrap_or_else(|| "Unit".to_string());
        match result_ty {
            Some(Ty::NamedPtrOf(_, _, _) | Ty::NamedPtr(_)) if entry_fails => {
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                self.emit_entry_failure(&declared_err, &scope, &mut f);
            }
            Some(ty) => self.drop_value(ty, &mut f),
            None => {}
        }
        // Deliver the run `result` discriminant to the component-level
        // caller via `task.return` (0 = ok). This must precede `End` and
        // is how the async-stackful lift signals completion. Reaching
        // the end of the body is success — an exact exit code goes
        // through the hard `Exited(n)` (`exit-with-code`) escape hatch.
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Call(self.fn_task_return));
        f.instruction(&Instruction::End);
        f
    }

    /// With the container in `alloc_ptr`: when its tag is `Err` / `None`,
    /// print a string payload (typed `err_name`) and leave the entry with
    /// the error result. Falls through on `Ok` / `Some`.
    pub(super) fn emit_entry_failure(
        &mut self,
        err_name: &str,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        let payload = match err_name {
            "String" => Ty::Str,
            n => self.resolve_repr(n),
        };
        if payload.is_str_like() {
            self.load_payload_at(scope.alloc_ptr(), 4, &payload, f);
            self.emit_print(payload, scope, f);
        }
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Call(self.fn_task_return));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
    }

    pub(super) fn build_user_function(&mut self, func: &FunctionDef) -> Function {
        let (params, scope) = self.build_local_scope(func);
        let _ = params; // params are implicit in the function type
        let mut f = Function::new(extra_locals_decl(max_arm_depth(&func.body)));
        let body = func.body.clone();
        // `?` may early-return the whole Result/Option value, but only
        // when the enclosing function itself returns the same shape
        // (one i32 pointer at the core level). Record which kind for
        // the duration of this body.
        let ret = self.resolve_return_ty(func);
        self.cur_fn_early_return = match &ret {
            Ty::NamedPtrOf(n, _, _) | Ty::NamedPtr(n) if n == "Result" => Some("Result"),
            Ty::NamedPtrOf(n, _, _) | Ty::NamedPtr(n) if n == "Option" => Some("Option"),
            _ => None,
        };
        let result = self.compile_block_return(&body, &scope, &mut f);
        self.cur_fn_early_return = None;
        // The function's WASM type already declares the result type;
        // the value should already be on the stack.
        let _ = result;
        f.instruction(&Instruction::End);
        f
    }

    /// Collect every name in a type's alias chain. For `Json = String`,
    /// returns `["Json", "String"]`. For a base type like `String`,
    /// returns `["String"]`. A generic alias ends the chain at its base
    /// (`Stdin = Stream<String>` is `["Stdin", "Stream"]`), as the
    /// checker's alias map does. Bounded by `resolve_repr_depth`'s
    /// 20-step guard so a malformed cycle can't infinite-loop.
    pub(super) fn collect_alias_chain(&self, name: &str) -> Vec<String> {
        let mut out = vec![name.to_string()];
        let mut current = name.to_string();
        for _ in 0..20 {
            let body = match self.type_defs.get(&current) {
                Some(b) => b.clone(),
                None => break,
            };
            if let TypeExpr::Named {
                name: next,
                generics,
                ..
            } = &body
            {
                if out.iter().any(|n| n == next) {
                    break;
                }
                if !generics.is_empty() {
                    out.push(next.clone());
                    break;
                }
                if out.iter().any(|n| n == next) {
                    break;
                }
                out.push(next.clone());
                current = next.clone();
            } else {
                break;
            }
        }
        out
    }

    /// Build LocalScope for a function's params + receiver.
    pub(super) fn build_local_scope(&self, func: &FunctionDef) -> (Vec<ValType>, LocalScope) {
        let mut scope = LocalScope::default();
        let mut local_idx: u32 = 0;
        let mut params = Vec::new();

        // For Self-ctor functions (`Name = (P) -> R<Name, E>` after
        // `resolve_new_syntax`), the WASM signature omits the receiver
        // — the value lives as the first param. The receiver name is
        // a type-level handle, not a runtime value. We still register
        // it under the *param's* local index (so the body can reference
        // it by either the newtype name like `Json` or the underlying
        // type name like `String`) but we don't allocate a separate
        // slot for it.
        // Exact declared names always win; alias-chain names (the
        // newtype's underlying types) only fill slots no exact name
        // claims, receiver-first. Without this precedence, a later
        // param whose newtype erases to the same underlying type
        // clobbers an earlier exact param — in
        // `elAttr = (Attr * String * Tag)`, `Tag`'s alias registration
        // used to steal the body's `String` references.
        let mut alias_pending: Vec<(String, u32, Ty)> = Vec::new();
        let skip_receiver_slot = is_self_ctor(func);
        if let Some(recv) = &func.receiver {
            if !skip_receiver_slot {
                let repr = self.resolve_repr(&recv.name);
                let vt = repr.val_types();
                let mut chain = self.collect_alias_chain(&recv.name).into_iter();
                if let Some(exact) = chain.next() {
                    scope.vars.insert(exact, (local_idx, repr.clone()));
                }
                for alias in chain {
                    alias_pending.push((alias, local_idx, repr.clone()));
                }
                local_idx += vt.len() as u32;
                params.extend(vt);
            }
        }
        for param in &func.params {
            // A `T^N` repetition input binds no bare name: each of its N
            // components gets a positional entry (`T.1` … `T.N`) that
            // `Expr::FieldAccess` with a numeric field reads back.
            if let TypeExpr::Repeat { ty, count, .. } = &param.ty {
                if let TypeExpr::Named { name, .. } = ty.as_ref() {
                    let repr = self.resolve_repr(name);
                    let vt = repr.val_types();
                    for i in 1..=*count {
                        scope
                            .vars
                            .insert(format!("{name}.{i}"), (local_idx, repr.clone()));
                        local_idx += vt.len() as u32;
                        params.extend(vt.iter().copied());
                    }
                }
                continue;
            }
            if let TypeExpr::Named { name, .. } = &param.ty {
                let repr = self.resolve_repr(name);
                let vt = repr.val_types();
                let mut chain = self.collect_alias_chain(name).into_iter();
                if let Some(exact) = chain.next() {
                    scope.vars.insert(exact, (local_idx, repr.clone()));
                }
                for alias in chain {
                    alias_pending.push((alias, local_idx, repr.clone()));
                }
                // For a Self-ctor, also register the receiver-type name
                // (`Json` for `Self = (String) -> ...`) as an alias of
                // the first param so `Json` inside the body refers to
                // the same value as `String`.
                if skip_receiver_slot && local_idx == 0 {
                    if let Some(recv) = &func.receiver {
                        scope
                            .vars
                            .insert(recv.name.clone(), (local_idx, repr.clone()));
                    }
                }
                local_idx += vt.len() as u32;
                params.extend(vt);
            }
        }
        for (alias, idx, repr) in alias_pending {
            scope.vars.entry(alias).or_insert((idx, repr));
        }
        scope.param_count = local_idx;
        (params, scope)
    }

    // ── Expression compilation ─────────────────────────────────────────────────

    /// Compile a block, leaving the last expression's value on the stack.
    pub(super) fn compile_block_return(
        &mut self,
        block: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let n = block.exprs.len();
        for expr in &block.exprs[..n.saturating_sub(1)] {
            let ty = self.compile_expr(expr, scope, f);
            self.drop_value(ty, f);
        }
        if let Some(last) = block.exprs.last() {
            self.compile_expr(last, scope, f)
        } else {
            Ty::Unit
        }
    }

    pub(super) fn compile_expr(&mut self, expr: &Expr, scope: &LocalScope, f: &mut Function) -> Ty {
        match expr {
            // ── Literals ──────────────────────────────────────────────────────
            Expr::IntLit { value, .. } => {
                f.instruction(&Instruction::I64Const(*value));
                Ty::I64
            }
            Expr::FloatLit { value, .. } => {
                f.instruction(&Instruction::F64Const((*value).into()));
                Ty::F64
            }
            Expr::StringLit { value, .. } => {
                // Literal data is stored without a trailing newline; `.print`
                // appends one universally (see `emit_print`).
                let (ptr, len) = self.strings.intern(value);
                f.instruction(&Instruction::I32Const(ptr as i32));
                f.instruction(&Instruction::I32Const(len as i32));
                Ty::Str
            }

            // ── Identifier: param / capability ───────────────────────────────
            Expr::Ident(id) => {
                if let Some((idx, repr)) = scope.vars.get(&id.name).cloned() {
                    self.push_local(idx, &repr, f);
                    repr
                } else {
                    // Capability or unknown — no runtime value
                    Ty::Unit
                }
            }

            // ── Constructors ──────────────────────────────────────────────────
            Expr::Constructor { name, args, .. } => {
                self.compile_constructor(&name.name, args, scope, f)
            }

            // ── Field access (.field) ──────────────────────────────────────
            //
            // Newtype unwrap (`value.B` where the value's type is `A = B`)
            // is a no-op coercion at the wasm level since the newtype and
            // its underlying type share representation — we just retype
            // the value on the stack. See the language spec
            // (docs/src/spec/) on newtypes as 1-component products.
            //
            // Real product field selection (`user.Birthday`) isn't yet
            // implemented; the checker accepts the syntax (registered in
            // `product_fields`), codegen catch-up is a follow-up.
            //
            // Method calls — including `.print()` — go through `MethodCall`
            // instead, so we don't special-case any method name here.
            Expr::FieldAccess {
                receiver, field, ..
            } => {
                // Positional access into a repetition parameter:
                // `Int.1` reads the local `build_local_scope` registered
                // under the composite key `Int.1`. The receiver is the
                // parameter's name, not a compilable value — no bare
                // binding exists for it — so this must run before the
                // receiver compile below.
                if field.name.parse::<u64>().is_ok() {
                    if let Expr::Ident(recv_ident) = receiver.as_ref() {
                        let key = format!("{}.{}", recv_ident.name, field.name);
                        if let Some((idx, repr)) = scope.vars.get(&key).cloned() {
                            self.push_local(idx, &repr, f);
                            return repr;
                        }
                    }
                }
                let recv_ty = self.compile_expr(receiver, scope, f);
                if let Some(unwrapped) = newtype_unwrap_ty(&recv_ty, &field.name) {
                    return unwrapped;
                }
                // Newtype-alias field access: `x.Inner` where `x`'s static
                // type is a pointer-repr newtype `Name = Inner` (union,
                // product, or alias chain). Both erase to the same wasm
                // value, so selecting the wrapped component is a pure
                // retype — no instructions. Without this a recursive
                // newtype like `Link = Next` (`Link.Next`) fell through to
                // `drop_value` below, dropping the pointer and desyncing
                // the stack.
                //
                // The checker accepts this projection through the *whole*
                // alias chain, not just one hop (`method_known_via_aliases`:
                // `Cleared = Todos = String` makes `Cleared.String` valid),
                // so codegen has to walk it the same way rather than only
                // matching the receiver's immediate alias target.
                //
                // A string-aliased chain (`Head = Token = Number`, all
                // reaching `String`) is the same pure retype and walks
                // the same way — the (ptr, len) pair on the stack is
                // already the projection. Only the `String` hop had a
                // case before (`newtype_unwrap_ty`), so `Head.Number`
                // fell through and dropped the pair.
                if let Ty::NamedPtr(name) | Ty::NamedStr(name) = &recv_ty {
                    let mut current = name.as_str();
                    let mut depth = 0;
                    while depth < 20 {
                        match self.type_defs.get(current) {
                            Some(TypeExpr::Named {
                                name: inner,
                                generics,
                                ..
                            }) if generics.is_empty() => {
                                if *inner == field.name {
                                    return self.resolve_repr(inner);
                                }
                                current = inner.as_str();
                                depth += 1;
                            }
                            _ => break,
                        }
                    }
                }
                // Product field access: the receiver is a heap pointer
                // to a struct laid out by `build_product_value`. Read
                // back from the matching byte offset.
                if let Ty::NamedPtr(product_name) = &recv_ty {
                    if self.type_defs.get(product_name).is_some_and(|t| {
                        matches!(t, TypeExpr::Product { .. } | TypeExpr::Repeat { .. })
                    }) {
                        if let Some(ty) =
                            self.load_product_field(product_name, &field.name, scope, f)
                        {
                            return ty;
                        }
                    }
                }
                self.drop_value(recv_ty, f);
                Ty::Unit
            }

            // ── Method calls ──────────────────────────────────────────────────
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.compile_method_call(receiver, &method.name, args, scope, f),

            // ── Dispatch ──────────────────────────────────────────────────────
            Expr::Match {
                scrutinee, arms, ..
            } => self.compile_match(scrutinee, arms, scope, f),

            // ── Try operator `?` ───────────────────────────────────────────────
            Expr::Try { inner, .. } => {
                let inner_ty = self.compile_expr(inner, scope, f);
                // `?` extracts the Ok/Some payload; when the enclosing
                // function itself returns a Result (same core shape:
                // one i32 pointer), an Err short-circuits by returning
                // the whole Result value unchanged. In non-Result
                // contexts (e.g. `main`) extraction is unconditional,
                // as before. Payload width by inner type:
                //   - `Ty::NamedPtrOf(_, ok, _)` → whatever `ok`'s repr stores at
                //     offset 4: an `i64` / `f64`, one `i32` pointer, or a
                //     `(ptr, len)` pair at offsets 4 and 8.
                //   - `Ty::NamedPtr("Result"|"Option")` → `i64` at offset 4 (legacy).
                match &inner_ty {
                    Ty::NamedPtrOf(container, ok_name, err_name) => {
                        let container = container.clone();
                        let ok_name = ok_name.clone();
                        let err_name = err_name.clone();
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        if self.entry_fails {
                            self.emit_entry_failure(&err_name, scope, f);
                        } else if self.cur_fn_early_return == Some(container.as_str()) {
                            // tag == 0 (Err) → return the Result as-is.
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::I32Load(MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));
                            f.instruction(&Instruction::I32Eqz);
                            f.instruction(&Instruction::If(BlockType::Empty));
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::Return);
                            f.instruction(&Instruction::End);
                        }
                        // The payload keeps its Canon-level type so
                        // subsequent method calls dispatch correctly
                        // (e.g. `.read()` on `File` after
                        // `Path(…).File()?`, `Todo.Title` after
                        // `Todos -> At(1)?`), and is read back in the
                        // shape that type stores at +4.
                        let payload_ty = match ok_name.as_str() {
                            "String" => Ty::Str,
                            n => self.resolve_repr(n),
                        };
                        self.load_payload_at(scope.alloc_ptr(), 4, &payload_ty, f);
                        payload_ty
                    }
                    Ty::NamedPtr(n) if n == "Result" || n == "Option" => {
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        if self.entry_fails {
                            self.emit_entry_failure("Unit", scope, f);
                        } else if self.cur_fn_early_return == Some(n.as_str()) {
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::I32Load(MemArg {
                                offset: 0,
                                align: 2,
                                memory_index: 0,
                            }));
                            f.instruction(&Instruction::I32Eqz);
                            f.instruction(&Instruction::If(BlockType::Empty));
                            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                            f.instruction(&Instruction::Return);
                            f.instruction(&Instruction::End);
                        }
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I64Load(MemArg {
                            offset: 4,
                            align: 3,
                            memory_index: 0,
                        }));
                        Ty::I64
                    }
                    other => other.clone(),
                }
            }

            // ── Lambda ────────────────────────────────────────────────────────
            Expr::Lambda { .. } => {
                // Lambda values are handled at call sites (.map etc.)
                // Push a placeholder i32 (0) for now.
                f.instruction(&Instruction::I32Const(0));
                Ty::I32
            }

            // ── Product literal ───────────────────────────────────────────────
            Expr::ProductValue { fields, .. } => {
                // Phase 3: compile each field for side effects; return the
                // last value (used when constructing union payloads).
                for field in &fields[..fields.len().saturating_sub(1)] {
                    let ty = self.compile_expr(field, scope, f);
                    self.drop_value(ty, f);
                }
                if let Some(last) = fields.last() {
                    self.compile_expr(last, scope, f)
                } else {
                    Ty::Unit
                }
            }

            // ── JSON literal ──────────────────────────────────────────────
            // ── JSON literal ──────────────────────────────────
            //
            // All-static fast path: collapse the parts into one string
            // literal and push directly — zero runtime cost.
            //
            // Mixed (with interpolations): synthesize a left-associated
            // chain of `String.concat` calls over alternating `StringLit`
            // (Static fragments) and piped `Encoded` constructions
            // (Interp expressions), then compile that. This reuses the
            // existing `concat` builtin and the `Encoded` family
            // dispatch so we don't need new codegen for either; the
            // surface-syntax `{"k": foo}` is purely parser sugar over
            // machinery that already exists.
            Expr::JsonLit { parts, span } => {
                let all_static = parts
                    .iter()
                    .all(|p| matches!(p, crate::ast::JsonLitPart::Static(_)));
                if all_static {
                    let mut merged = String::new();
                    for p in parts {
                        if let crate::ast::JsonLitPart::Static(s) = p {
                            merged.push_str(s);
                        }
                    }
                    let (ptr, len) = self.strings.intern(&merged);
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::I32Const(len as i32));
                    Ty::Str
                } else {
                    let chain = literals::json_lit_to_concat_chain(parts, *span);
                    self.compile_expr(&chain, scope, f)
                }
            }

            // ── HTML literal ──────────────────────────────────
            //
            // Same two-tier lowering as the JSON literal above: an
            // all-static literal collapses to one interned string; a
            // literal with interpolation holes becomes a
            // `String.concat` chain whose `Interp` links are
            // `-> Escaped` constructions (escaping for `String`/`Int` via the
            // stdlib's `text()`, identity for `Html` — see
            // `packages/canon/src/web/html.can`).
            Expr::HtmlLit { parts, span } => {
                let all_static = parts
                    .iter()
                    .all(|p| matches!(p, crate::ast::HtmlLitPart::Static(_)));
                if all_static {
                    let mut merged = String::new();
                    for p in parts {
                        if let crate::ast::HtmlLitPart::Static(s) = p {
                            merged.push_str(s);
                        }
                    }
                    let (ptr, len) = self.strings.intern(&merged);
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::I32Const(len as i32));
                    Ty::Str
                } else {
                    let chain = literals::html_lit_to_concat_chain(parts, *span);
                    self.compile_expr(&chain, scope, f)
                }
            }

            // ── Backtick format string ────────────────────────
            //
            // The plain-`String` mirror of the HTML literal above. The
            // parser folds an all-static backtick string to a
            // `StringLit`, so a `FormatLit` always has interpolation
            // holes and lowers to a `String.concat` chain whose `Interp`
            // links are `-> String` conversions.
            Expr::FormatLit { parts, span } => {
                let chain = literals::format_lit_to_concat_chain(parts, *span);
                self.compile_expr(&chain, scope, f)
            }

            // ── Await (checker-inserted, Phase 5) ─────────────────────────────
            Expr::Await { inner, .. } => self.compile_expr(inner, scope, f),
        }
    }

    // ── Method call dispatch ────────────────────────────────────────────────────

    /// The `Option` type `At` / `First` yield for a list receiver:
    /// `Ty::NamedPtrOf` carrying the element's type name when the
    /// receiver's syntax reveals it, so `?` and dispatch read the payload
    /// back in the element's own shape; the plain option otherwise. The
    /// in-memory layout is the same either way — the slot at +4 holds
    /// whatever the element stores.
    pub(super) fn option_ty_for_list(&self, receiver: &Expr) -> Ty {
        match self.list_elem_name(receiver) {
            Some(elem) => Ty::NamedPtrOf("Option".to_string(), elem.clone(), elem),
            None => Ty::NamedPtr("Option".to_string()),
        }
    }

    /// The type name of a list-typed receiver's elements.
    ///
    /// `Ty::List` records no element type, so `At` / `First` — which
    /// wrap an element in an `Option` — cannot otherwise tell a
    /// `List<String>` from a `List<Int>` from a `List<Todo>`. Every slot
    /// is 8 bytes, so only the *type* of the result differs, and it is
    /// recovered from the receiver's syntax: a `List(…)` literal's first
    /// element, a named list type resolved through `type_defs` (`Args =
    /// List<String>`, `Todos = List<Todo>`), a `Mapped` lambda's return
    /// type, or the receiver of a transform that keeps the element type
    /// (`Filtered`, `Taken`, `Appended`, `Joined`).
    pub(super) fn list_elem_name(&self, receiver: &Expr) -> Option<String> {
        if let Expr::Constructor { name, args, .. } = receiver {
            if name.name == "List" {
                let first = match args.as_slice() {
                    [Expr::ProductValue { fields, .. }] => fields.first(),
                    _ => args.first(),
                }?;
                return match first {
                    Expr::StringLit { .. } | Expr::FormatLit { .. } => Some("String".into()),
                    Expr::IntLit { .. } => Some("Int".into()),
                    Expr::FloatLit { .. } => Some("Float".into()),
                    Expr::Constructor { name, .. } if name.name == "List" => Some("List".into()),
                    other => syntactic_type_name(other)
                        .filter(|n| !matches!(self.resolve_repr(n), Ty::Unit))
                        .map(str::to_string),
                };
            }
        }
        if let Expr::MethodCall {
            receiver: inner,
            method,
            args,
            ..
        } = receiver
        {
            match method.name.as_str() {
                "Mapped" => {
                    if let Some(Expr::Lambda { return_ty, .. }) = args.first() {
                        return named_type_name(return_ty);
                    }
                }
                "Filtered" | "Taken" | "Skipped" | "Reversed" | "Sorted" | "Appended"
                | "Joined" => {
                    return self.list_elem_name(inner);
                }
                _ => {}
            }
        }
        // A named list type — chase the alias chain to its `List<T>`
        // body and name the element.
        let name = syntactic_type_name(receiver)?;
        for alias in self.collect_alias_chain(name) {
            if let Some(TypeExpr::Named { name, generics, .. }) = self.type_defs.get(&alias) {
                if name == "List" && generics.len() == 1 {
                    return named_type_name(&generics[0]);
                }
            }
        }
        None
    }

    pub(super) fn compile_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // Concurrency combinators: `a.parallel(b)` / `a.race(b)`. The
        // receiver and argument are *un-awaited* async calls (the
        // auto-await pass exempts these two methods); compile_parallel /
        // compile_race emit the non-blocking call for each side
        // themselves, so the receiver must NOT be compiled here.
        if matches!(method, "parallel" | "race" | "Parallel" | "Race") && args.len() == 1 {
            let combined = [receiver.clone(), args[0].clone()];
            return if method.eq_ignore_ascii_case("parallel") {
                self.compile_parallel(&combined, scope, f)
            } else {
                self.compile_race(&combined, scope, f)
            };
        }

        // The pipe form of prefix construction: `A -> B(C)` is the same
        // call as `B(A * C)` — the receiver fills the first slot of `B`'s
        // input product. When `B` names a type constructor, route it
        // through `compile_constructor` (the single construction path)
        // so piped and prefix spellings build identically: products,
        // union variants, newtypes, primitive conversions, constructor
        // families, shapes, and the HTTP `Response` all handled there.
        // Builtins (`Sum`, `Lt`, `Joined`, …) and pure operations are
        // not type names, so they fall through to the method paths
        // below. Runs before the receiver is compiled, so
        // `compile_constructor` owns every input — no double emit.
        // A name in the builtin vocabulary (`Length`, `Sum`, `Mapped`,
        // `Eq`, …) is never construction even when it also names a type
        // (`Length = Int`, `Mapped<U> = List<U>`): the method paths below
        // own it as a builtin on the receiver (list length / map) or a
        // stdlib shape. Excluding it keeps `list -> Length` a length, not
        // a `Length(list)` newtype wrap.
        // A declared constructor family of the same name owns it
        // (`Depth * Tokens => Skipped` in canonc, `Map => Length` in the
        // stdlib): its receiver-typed lookups run first, and only a miss
        // falls through to the builtin.
        let has_func_body = self.func_table.keys().any(|(_, m)| m == method);
        let is_builtin_op = crate::ast::builtin_method_alias(method).is_some() && !has_func_body;

        // Union injection from a payload: `Broken -> Outcome` inside a
        // `* Broken` arm. A value *referenced* by a variant's name — an
        // arm-bound payload, a parameter, a product field — is the bare
        // payload, so the pipe has to build the variant around it. (A
        // *construction* — `Nil()`, `x -> Made` — already produced the
        // union struct; that injection is the identity relabel further
        // down.) Owned here, ahead of every other route: the construction
        // path treats a union name as a newtype and would relabel the
        // payload without tagging it, and the method path resolves
        // nothing for a payload receiver and drops it. Both were silent —
        // dispatch then read whatever memory happened to hold.
        if args.is_empty() && matches!(receiver, Expr::Ident(_) | Expr::FieldAccess { .. }) {
            if let Some(variant) = syntactic_type_name(receiver) {
                if self
                    .variant_parent
                    .get(variant)
                    .is_some_and(|p| p == method)
                {
                    let variant = variant.to_string();
                    return self.inject_union_variant(method, &variant, receiver, scope, f);
                }
            }
        }

        // Message application: `map -> Insert(Key("a") * Value("1"))`
        // builds the message and calls the receiver's command for it,
        // inputs in the command's declared order. A value that already
        // is the message passes through; a payload-less message (`Clear
        // = Unit`) has no stack shape and compiles to nothing.
        if let Some((_, info)) = self
            .infer_ctor_arg_type_name(receiver)
            .and_then(|r| self.message_target(&r, method))
        {
            let whole_message = args.len() == 1
                && self
                    .infer_ctor_arg_type_name(&args[0])
                    .is_some_and(|t| self.dispatch_candidates(&t).iter().any(|c| c == method));
            let message: Option<Expr> = if whole_message {
                Some(args[0].clone())
            } else if self.collect_alias_chain(method).iter().any(|t| t == "Unit") {
                None
            } else {
                Some(Expr::Constructor {
                    name: crate::ast::Ident {
                        name: method.to_string(),
                        span: receiver.span(),
                    },
                    type_args: Vec::new(),
                    args: args.to_vec(),
                    span: receiver.span(),
                })
            };
            let message_first = info.input_types[0] == method;
            let mut ordered: Vec<Expr> = Vec::new();
            if message_first {
                ordered.extend(message.clone());
                ordered.push(receiver.clone());
            } else {
                ordered.push(receiver.clone());
                ordered.extend(message);
            }
            let _ = self.compile_expr(&ordered[0], scope, f);
            return self.emit_func_table_call(&info, &ordered[1..], scope, f);
        }

        // A name with a func-table body is a shape / constructor family
        // (`Route`, `Served`, `TestResult`, `Greeting`'s Int member, …).
        // Those resolve on the method path below, keyed on the receiver's
        // *compiled* type — routing them through `compile_constructor`
        // would rebuild the receiver and lose handle/repr threading. Only
        // *pure* construction (a product / newtype / variant / primitive
        // with no func body) needs the construction route.
        let is_ctor_name = (!is_builtin_op
            && !has_func_body
            && crate::ast::is_type_name(method)
            && (self.type_defs.contains_key(method)
                || self.variant_parent.contains_key(method)
                || matches!(method, "Some" | "None" | "Ok" | "Err")))
            // HTTP `Response` construction is owned by codegen
            // (`build_http_response`) regardless of its checker binding,
            // so route the piped form there too.
            || (self.http_mode && method == "Response");
        if is_ctor_name {
            let mut ctor_inputs = vec![receiver.clone()];
            match args {
                [Expr::ProductValue { fields, .. }] => ctor_inputs.extend(fields.iter().cloned()),
                _ => ctor_inputs.extend(args.iter().cloned()),
            }
            let ctor_args = if ctor_inputs.len() == 1 {
                ctor_inputs
            } else {
                vec![Expr::ProductValue {
                    fields: ctor_inputs,
                    span: receiver.span(),
                }]
            };
            return self.compile_constructor(method, &ctor_args, scope, f);
        }

        // A single product argument stands for its flattened components:
        // `headers.set(Name * Value)`, `server.route(a * b * c * d)`, and
        // every other multi-input builtin/binding receive positional args
        // this way now that comma argument lists are gone. (The checker's
        // `effective_call_arity` already flattens for arity; codegen
        // matches here.) `substring`/`slice` keep the product intact —
        // `substring_bounds` reads the `From`/`To` components by type, so
        // it stays positionless.
        let flat_args: Vec<Expr>;
        let args: &[Expr] = match args {
            [Expr::ProductValue { fields, .. }] if !matches!(method, "substring" | "Substring") => {
                flat_args = fields.clone();
                &flat_args
            }
            _ => args,
        };

        // Commutative calling: any component of the callee's input
        // product may pipe in on the left, so the receiver is not
        // necessarily the first slot (`Sep(124) -> Tail(text)` and
        // `text -> Tail(Sep(124))` are one call). Bind the inputs by
        // type and compile them in parameter order. This must run
        // *before* the receiver is compiled — once it is on the stack it
        // occupies slot 0, and a call entered from a later component
        // pushes its operands against the wrong slots.
        if !is_builtin_op {
            if let Some(recv_name) = self.infer_ctor_arg_type_name(receiver) {
                let hit = self
                    .dispatch_candidates(&recv_name)
                    .into_iter()
                    .find_map(|c| self.func_table.get(&(Some(c), method.to_string())).cloned());
                if let Some(info) = hit {
                    let mut inputs = vec![receiver.clone()];
                    inputs.extend(args.iter().cloned());
                    if let Some(ordered) = self.commutative_order(&info.input_types, &inputs) {
                        let _ = self.compile_expr(&ordered[0], scope, f);
                        return self.emit_func_table_call(&info, &ordered[1..], scope, f);
                    }
                }
            }
        }

        let recv_ty = self.compile_expr(receiver, scope, f);

        // `String(Byte)` — the one-byte-string memory primitive — must
        // win over the stdlib `String` constructor family: a `Byte`
        // receiver erases to `Ty::I64`, and its alias chain passes
        // through `Int`, whose family member is the decimal renderer.
        if method == "String" && args.is_empty() && self.expr_is_byte(receiver) {
            return self.emit_byte_to_str(scope, f);
        }

        // Check user func table first: look up by Canon type name. Scalars
        // (`Int`, `Float`, `Bool`, `String`) don't carry their name on the
        // `Ty` enum, so we map them back to a canonical Canon type name here
        // — this lets `extern Wasm` declarations with scalar receivers (e.g.
        // `min = (Int * …)`) resolve from a call site like `5.min(…)`.
        //
        // Capability receivers (`Random`, `Stdout`, `Clock`, …) leave nothing
        // on the stack and have type `Ty::Unit`. We recover their type name
        // from the AST identifier so calls like `Random.randomInt` resolve.
        let type_name = recv_ty
            .canon_name()
            .map(|s| s.to_string())
            .or_else(|| match &recv_ty {
                Ty::I64 => Some("Int".to_string()),
                Ty::F64 => Some("Float".to_string()),
                Ty::I32 => Some("Bool".to_string()),
                Ty::Str => Some("String".to_string()),
                Ty::Unit => match receiver {
                    Expr::Ident(id) => Some(id.name.clone()),
                    _ => None,
                },
                _ => None,
            });
        // Try the receiver's own type name first, then every name in
        // its newtype alias chain — `Foo("x") -> Encoded` with `Foo =
        // String` must find the `(String)` family member. A method
        // resolves to a user/stdlib function under its written name; a
        // miss falls through to the builtin below.
        // A scalar newtype erases to its underlying primitive, so a piped
        // construction like `3000 -> Port` leaves `Ty::I64` on the stack
        // and `type_name` recovers only "Int" — losing "Port", which the
        // next step (`Port -> HttpServer`) dispatches on. Recover the
        // *static* type from the receiver's syntactic shape (see
        // `syntactic_type_name`). Tried first so newtype-typed shapes
        // still resolve.
        // A message application carries its receiver's type, not the
        // message's: after `limbs -> LimbsShift(1)` the value is still
        // a `Limbs`, and reading the chain as a `LimbsShift` (an `Int`
        // newtype) would send the next lookup down `Int`'s alias chain.
        let message_recv_type = match receiver {
            Expr::MethodCall {
                receiver: inner,
                method: message,
                ..
            } => self
                .infer_ctor_arg_type_name(inner)
                .and_then(|r| self.message_target(&r, &message.name))
                .map(|(target, _)| target),
            _ => None,
        };
        let static_recv_type: Option<String> = message_recv_type.or_else(|| {
            syntactic_type_name(receiver)
                .filter(|name| self.type_defs.contains_key(*name))
                .map(str::to_string)
        });
        let mut candidate_types: Vec<String> = Vec::new();
        if let Some(st) = &static_recv_type {
            // A name that is both a union variant and a newtype has two
            // runtime shapes: the union struct (`Text -> Name` with
            // `Name` a variant of `Token` builds one) or the bare
            // payload (a `* Name` arm binds one). Only the payload can
            // satisfy a function typed on the erased type, so the alias
            // chain is consulted only when the compiled receiver isn't
            // the union struct — otherwise `Text -> Name -> Token`
            // walks `Name` to `Text` and finds the very `Text => Token`
            // family member it sits inside, handing it a union pointer
            // where a string goes.
            let is_union_struct = matches!(
                (&recv_ty, self.variant_parent.get(st)),
                (Ty::NamedPtr(n), Some(parent)) if n == parent
            );
            if is_union_struct {
                candidate_types.push(st.clone());
            } else {
                candidate_types.extend(self.collect_alias_chain(st));
            }
        }
        if let Some(name) = &type_name {
            for a in self.collect_alias_chain(name) {
                if !candidate_types.contains(&a) {
                    candidate_types.push(a);
                }
            }
        }
        // A member is the call only when the receiver and the arguments
        // fill its inputs: `list -> Todos` beside `Wire => Todos` is the
        // documented zero-argument relabel, not that call short an
        // input (which emitted with the wrong stack shape).
        let fits = |info: &FuncInfo| info.input_types.len() == 1 + args.len();
        for alias in candidate_types {
            let key = (Some(alias), method.to_string());
            if let Some(info) = self.func_table.get(&key).filter(|i| fits(i)).cloned() {
                return self.emit_func_table_call(&info, args, scope, f);
            }
        }
        let free_key = (None, method.to_string());
        if let Some(info) = self.func_table.get(&free_key).filter(|i| fits(i)).cloned() {
            return self.emit_func_table_call(&info, args, scope, f);
        }

        // No user/stdlib function matched — normalize the types-only
        // vocabulary (`Print`/`Sum`/`Joined`/…) to its canonical builtin
        // name so the `print`/`String`/builtin paths below recognize it.
        let method = crate::ast::builtin_method_alias(method).unwrap_or(method);

        // Conversion is construction (the language spec, docs/src/spec/):
        // a String-alias receiver (`Path("/x").String()`) is the
        // identity conversion — the value already is one. The scalar
        // renderings (`Int.String()`, `Float.String()`, `Bool.String()`)
        // resolved through the func-table lookups above into the
        // `canon/string.can` constructor family; `Byte.String()`
        // took the one-byte-string primitive before them.
        if method == "String" && args.is_empty() && recv_ty.is_str_like() {
            return Ty::Str;
        }

        // Primitive construction via pipe: `1 -> Int`, `2.5 -> Float`,
        // `b -> Bool`. The receiver is already on the stack; widen /
        // convert / pass through, mirroring `compile_constructor`'s
        // primitive arm. (`"5" -> Int` parses via a `(String) -> Int`
        // func-table member above, so only the non-string cases land
        // here.)
        if args.is_empty() {
            match (method, &recv_ty) {
                ("Int", Ty::I64) => return Ty::I64,
                ("Int", Ty::I32) => {
                    f.instruction(&Instruction::I64ExtendI32S);
                    return Ty::I64;
                }
                ("Int", Ty::F64) => {
                    f.instruction(&Instruction::I64TruncF64S);
                    return Ty::I64;
                }
                ("Float", Ty::F64) => return Ty::F64,
                ("Float", Ty::I64) => {
                    f.instruction(&Instruction::F64ConvertI64S);
                    return Ty::F64;
                }
                ("Bool", Ty::I32) => return Ty::I32,
                _ => {}
            }
        }

        // Newtype wrap via pipe: `"hi" -> Greeting` with `Greeting =
        // String` is the identity — the receiver already carries the
        // underlying representation, so relabel it to the newtype. Only
        // fires when the newtype's repr matches the receiver's, so a
        // *conversion* (`5 -> Json`, resolved above or a type error)
        // never silently becomes an identity wrap.
        if args.is_empty() {
            if let Some(TypeExpr::Named { .. }) = self.type_defs.get(method) {
                let wrapped = self.resolve_repr(method);
                let compatible = std::mem::discriminant(&wrapped)
                    == std::mem::discriminant(&recv_ty)
                    || (wrapped.is_str_like() && recv_ty.is_str_like());
                if compatible {
                    return wrapped;
                }
            }
        }

        // Union injection via pipe: `Nil() -> Texts` with `Texts = Cons
        // + Nil` is the identity — a variant constructor already built
        // the union struct (tag + payload), so the receiver on the stack
        // *is* the union value. Only an injection a constructor family
        // doesn't own reaches here: the family registers one commutative
        // key per input component, so `has_func_body` is true for the
        // union's name and sends every `X -> Union` pipe down this path,
        // including the ones no member accepts. Those resolve nowhere,
        // and without the relabel the value is dropped — the union
        // arrives untagged and dispatch reads whatever memory held.
        if args.is_empty() {
            if let Some(TypeExpr::Union { .. }) = self.type_defs.get(method) {
                if let Ty::NamedPtr(recv_name) = &recv_ty {
                    if self
                        .collect_alias_chain(recv_name)
                        .iter()
                        .any(|a| a == method)
                    {
                        return Ty::NamedPtr(method.to_string());
                    }
                }
            }
        }

        // `.print()` is a universal zero-arg method that delegates to the
        // type-aware `emit_print` helper.
        if method == "print" && args.is_empty() {
            self.emit_print(recv_ty, scope, f);
            return Ty::Unit;
        }

        // Built-in methods
        self.compile_builtin_method(receiver, recv_ty, method, args, scope, f)
    }

    /// Emits a call to a function registered in `func_table`. Handles the
    /// indirect-return convention for `extern Wasm` functions whose result
    /// doesn't fit in a flat WASM value (`string`, `result<string, string>`).
    ///
    /// For a direct-return function the WASM stack on entry already has the
    /// receiver, so we just compile the remaining args and emit `Call(idx)`.
    /// For an indirect-return function we additionally allocate a return
    /// area, push its pointer as the trailing core arg, and after the call
    /// decode the result according to `info.indirect_return`.
    pub(super) fn emit_func_table_call(
        &mut self,
        info: &FuncInfo,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // Narrow-width conversions (WIT-informed lowering). Canon's
        // `Int` is i64 everywhere; a `wasi:*` extern whose WIT declares
        // u8/u16/u32/s8/s16/s32 has a core i32 slot instead. The
        // receiver (when present) is already on the stack — component
        // param 0 with everything else still unpushed, so its wrap must
        // happen before the args compile.
        let recv_count = info.narrow_params.len().saturating_sub(args.len());
        let narrow_at = |i: usize| info.narrow_params.get(i).copied().unwrap_or(false);
        if recv_count == 1 && narrow_at(0) {
            f.instruction(&Instruction::I32WrapI64);
        }
        for (i, a) in args.iter().enumerate() {
            let _ = self.compile_expr(a, scope, f);
            if narrow_at(recv_count + i) {
                f.instruction(&Instruction::I32WrapI64);
            }
        }
        match &info.indirect_return {
            Some(IndirectReturnShape::HttpSend { ok_name, err_name }) => {
                let (ok, err) = (ok_name.clone(), err_name.clone());
                return self.emit_http_send(info.func_idx, ok, err, scope, f);
            }
            Some(IndirectReturnShape::FileRead { ok_name, err_name }) => {
                let (ok, err) = (ok_name.clone(), err_name.clone());
                return self.emit_file_read(info.func_idx, ok, err, scope, f);
            }
            Some(IndirectReturnShape::FileWrite { ok_name, err_name }) => {
                let (ok, err) = (ok_name.clone(), err_name.clone());
                return self.emit_file_write(info.func_idx, ok, err, scope, f);
            }
            Some(IndirectReturnShape::StreamWrite { ok_name, err_name }) => {
                let (ok, err) = (ok_name.clone(), err_name.clone());
                return self.emit_stream_write(ok, err, scope, f);
            }
            _ => {}
        }
        if info.is_async {
            return self.emit_async_call(info, scope, f);
        }
        let Some(shape) = info.indirect_return.clone() else {
            f.instruction(&Instruction::Call(info.func_idx));
            if info.bare_result {
                // WIT bare `result;` return: the single i32 on the stack
                // is the canonical discriminant (0=ok, 1=err). Flip it
                // into Canon's alphabetical tag convention (Err=0, Ok=1)
                // and box it as an ordinary Canon Result struct so
                // dispatch and `?` treat it like any user-constructed
                // Result. The payload slot stays zero — both arms are
                // Unit, so nothing ever reads it.
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Xor);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                return Ty::NamedPtr("Result".to_string());
            }
            // Widen a narrow scalar result back to Canon's i64 `Int`,
            // zero- or sign-extending per the WIT signedness.
            match info.narrow_result_signed {
                Some(true) => {
                    f.instruction(&Instruction::I64ExtendI32S);
                }
                Some(false) => {
                    f.instruction(&Instruction::I64ExtendI32U);
                }
                None => {}
            }
            return info.result_ty.clone();
        };

        // Allocate the return area, stash its pointer, and call.
        f.instruction(&Instruction::I32Const(shape.return_area_size() as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        f.instruction(&Instruction::Call(info.func_idx));

        // Decode the result.
        match shape {
            // Fused before the generic call above.
            IndirectReturnShape::HttpSend { .. }
            | IndirectReturnShape::FileRead { .. }
            | IndirectReturnShape::FileWrite { .. }
            | IndirectReturnShape::StreamWrite { .. } => unreachable!("fused above"),
            IndirectReturnShape::String => {
                // (i32 ptr at +0, i32 len at +4) — push both as a string
                // pair. Use `info.result_ty` so the alias name is
                // preserved (set up by `assign_func_indices`).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            IndirectReturnShape::OptionString => {
                // Re-shape the canonical `option<string>` ret area
                // (disc byte at +0, ptr/len at +4/+8) into a fresh
                // Canon Option struct (i32 tag at +0, payload at
                // +4/+8). `$alloc` doesn't touch the `alloc_ptr`
                // *local*, which still points at the ret area.
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                for off in [4u64, 8] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            IndirectReturnShape::OptionScalar { prim } => {
                // Re-shape the canonical `option<T>` ret area (disc byte
                // at +0, scalar payload at +align(T)) into a fresh Canon
                // Option struct (i32 tag at +0, 8-byte payload at +4),
                // widening the payload to Canon's i64/f64 value repr so
                // `(None, Some<Int>)` dispatch and `?` read it like any
                // user-constructed Option.
                use wasm_encoder::PrimitiveValType as P;
                let payload_off = prim_size_align(prim).1 as u64;
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                // Tag: the WIT discriminant (0=none, 1=some) already
                // matches Canon's alphabetical (None=0, Some=1)
                // convention; store the byte as a full i32 so the
                // host's padding bytes become zero.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // Payload. On `none` this copies undefined ret-area
                // bytes — harmless: the area is sized for the payload
                // and a tag-0 Option's payload slot is never read.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                emit_prim_load_widen(f, prim, payload_off);
                match prim {
                    P::F32 | P::F64 => {
                        f.instruction(&Instruction::F64Store(MemArg {
                            offset: 4,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    _ => {
                        f.instruction(&Instruction::I64Store(MemArg {
                            offset: 4,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            IndirectReturnShape::ListString => {
                // (i32 list ptr at +0, i32 count at +4). The canonical
                // element layout matches Canon's `List<String>` exactly
                // — push the pair as-is.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                Ty::List
            }
            IndirectReturnShape::ListScalar { prim } => {
                use wasm_encoder::PrimitiveValType as P;
                let (elem_size, _) = prim_size_align(prim);
                if elem_size == 8 {
                    // u64 / s64 / f64: the canonical 8-byte element
                    // stride is byte-identical to Canon's list layout —
                    // push the (ptr, count) pair as-is, like ListString.
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: 0,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: 4,
                        align: 2,
                        memory_index: 0,
                    }));
                    return Ty::List;
                }
                // Narrow elements: re-pack the byte-packed canonical
                // buffer into a fresh 8-byte-stride Canon list, widening
                // each element per the WIT signedness. No user code
                // compiles inside this sequence, so the ordinary scratch
                // locals are safe to use.
                // src elem ptr → rptr, count → rlen
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                // dst = $alloc(count * 8) → rbool
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                // i = 0
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::Block(BlockType::Empty));
                f.instruction(&Instruction::Loop(BlockType::Empty));
                // if i >= count: break
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                f.instruction(&Instruction::I32GeS);
                f.instruction(&Instruction::BrIf(1));
                // dst + i*8
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                // src + i*elem_size
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(elem_size as i32));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                // dst[i] = widen(src[i])
                emit_prim_load_widen(f, prim, 0);
                match prim {
                    P::F32 => {
                        f.instruction(&Instruction::F64Store(MemArg {
                            offset: 0,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    _ => {
                        f.instruction(&Instruction::I64Store(MemArg {
                            offset: 0,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                }
                // i++
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::Br(0));
                f.instruction(&Instruction::End); // end loop
                f.instruction(&Instruction::End); // end block
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                Ty::List
            }
            IndirectReturnShape::ScalarRecord {
                product, fields, ..
            } => {
                // Copy each canonical field into a fresh Canon product
                // struct, widening narrow ints to i64. The ret area is
                // still in the `alloc_ptr` local ($alloc the function
                // doesn't touch codegen locals).
                let layout = self.product_field_layout(&product);
                let total: u32 = layout
                    .iter()
                    .map(|(n, _, _)| self.field_byte_size(n))
                    .sum::<u32>()
                    .max(4);
                f.instruction(&Instruction::I32Const(total as i32));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                for field in &fields {
                    let Some((_, repr, canon_off)) =
                        layout.iter().find(|(n, _, _)| n == &field.canon_name)
                    else {
                        continue;
                    };
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    emit_prim_load_widen(f, field.prim, field.offset as u64);
                    match repr {
                        Ty::F64 => {
                            f.instruction(&Instruction::F64Store(MemArg {
                                offset: *canon_off as u64,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        _ => {
                            f.instruction(&Instruction::I64Store(MemArg {
                                offset: *canon_off as u64,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                    }
                }
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr(product)
            }
            IndirectReturnShape::ByteStream {
                ok_name, err_name, ..
            } => {
                // invariant: `collect_extern_imports` allocates the three
                // builtins for every `ByteStream` extern.
                let stage = stream::Stage::Host {
                    read_fn: info
                        .stream_read_fn
                        .expect("byte-stream extern has a stream-read builtin"),
                    drop_stream_fn: info
                        .stream_drop_readable_fn
                        .expect("byte-stream extern has a stream-drop-readable builtin"),
                    drop_future_fn: info
                        .future_drop_readable_fn
                        .expect("byte-stream extern has a future-drop-readable builtin"),
                    third: stream::Third::Nothing,
                };
                // The stream at +0 and its future at +4 become a `Host`
                // stage, the `Ok` of the same `Result` struct a
                // `result<string, string>` return produces.
                let mem32 = |offset: u64| MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                };
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(mem32(0)));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(mem32(4)));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                self.emit_host_stream(stage, scope.tmp_i32(), scope.tmp_i32_b(), None, scope, f);
                self.build_result_ok(Ty::NamedPtr("Stream".to_string()), scope, f);
                Ty::NamedPtrOf("Result".to_string(), ok_name, err_name)
            }
            IndirectReturnShape::ResultStringString { ok_name, err_name } => {
                // Flip the WIT discriminant (byte 0) into Canon's tag
                // convention by XOR-ing with 1, and store back as a full
                // i32 so bytes 1–3 (which were undefined padding from the
                // host) become zero.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Xor);
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                // Push area pointer as the Result handle.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                Ty::NamedPtrOf("Result".to_string(), ok_name, err_name)
            }
        }
    }

    // ── Scratch save/load helpers ──────────────────────────────────────────────

    pub(super) fn save_to_scratch(&mut self, ty: Ty, scope: &LocalScope, f: &mut Function) {
        self.save_ty_to_scratch(&ty, scope, f);
    }

    pub(super) fn save_ty_to_scratch(&self, ty: &Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            }
            Ty::Str | Ty::NamedStr(_) => {
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
            }
            Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
            }
            Ty::Unit => {}
        }
    }

    pub(super) fn load_from_scratch(&self, ty: &Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
            }
            Ty::Unit => {}
        }
    }

    /// Store a single payload value into the struct at `address + offset`,
    /// where `address` is taken from the operand stack (NOT from
    /// `scope.alloc_ptr()` — that local is clobbered whenever the value
    /// expression contains a nested constructor).
    ///
    /// Stack contract on entry depends on `payload_ty`:
    ///   * Scalars (`Ty::I64`/`F64`/`I32`/`Ptr`/`NamedPtr`/`NamedPtrOf`):
    ///     `[address, value]` — one i32/i64 `store` consumes both.
    ///   * Strings (`Ty::Str`/`NamedStr`): `[address, ptr, len]` — two
    ///     i32 stores: `ptr` at `offset` and `len` at `offset + 4`,
    ///     both against the on-stack address.
    ///   * `Ty::Unit`: just drops the address. There's no payload.
    pub(super) fn store_payload_at_offset(
        &self,
        offset: u32,
        payload_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        match payload_ty {
            Ty::I64 => {
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::F64 => {
                f.instruction(&Instruction::F64Store(MemArg {
                    offset: offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                // Stack: [addr, ptr, len]. Stash ptr+len in `tmp_i32`/
                // `tmp_i32_b` (`rptr`/`rlen` may still hold the value
                // the caller pushed via `load_from_scratch`), stash the
                // on-stack addr in `addr_scratch`, then emit the two
                // stores against it. No re-load of `alloc_ptr`: the
                // on-stack address is the only one guaranteed to point
                // at the struct being built when the payload expression
                // contained nested allocations. A list is the same
                // `(ptr, len)` pair.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                // Store ptr at +offset
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                // Store len at +offset+4
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: (offset + 4) as u64,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Unit => {
                // No value to store — the address is still on the stack; drop it.
                f.instruction(&Instruction::Drop);
            }
        }
    }

    pub(super) fn store_value_at_offset(
        &self,
        offset: u32,
        repr: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        self.store_payload_at_offset(offset, repr, scope, f);
    }

    /// Push the value stored at `offset` past the address in local
    /// `base` in the shape `repr` occupies: an `i64` / `f64` verbatim,
    /// one `i32` for a pointer or `Bool`, and a `(ptr, len)` pair (two
    /// `i32`s at `offset` and `offset + 4`) for a string or list. The
    /// inverse of `store_payload_at_offset`.
    pub(super) fn load_payload_at(&self, base: u32, offset: u32, repr: &Ty, f: &mut Function) {
        let offset = offset as u64;
        match repr {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::F64Load(MemArg {
                    offset,
                    align: 3,
                    memory_index: 0,
                }));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: offset + 4,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Unit => {}
        }
    }

    /// The type name a value's repr carries, for labelling the payload of
    /// a container built around it (`Some(todo)` → `Option<Todo>`).
    /// `None` when the repr names nothing (`Unit`, a bare pointer).
    pub(super) fn payload_type_name(ty: &Ty) -> Option<String> {
        match ty {
            Ty::NamedPtr(n) | Ty::NamedStr(n) | Ty::NamedPtrOf(n, _, _) => Some(n.clone()),
            Ty::Str => Some("String".to_string()),
            Ty::I64 => Some("Int".to_string()),
            Ty::F64 => Some("Float".to_string()),
            Ty::I32 => Some("Bool".to_string()),
            Ty::List => Some("List".to_string()),
            Ty::Ptr | Ty::Unit => None,
        }
    }

    // ── Local variable helpers ─────────────────────────────────────────────────

    pub(super) fn push_local(&self, idx: u32, repr: &Ty, f: &mut Function) {
        match repr {
            Ty::I64 | Ty::F64 => {
                f.instruction(&Instruction::LocalGet(idx));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalGet(idx));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(idx));
                f.instruction(&Instruction::LocalGet(idx + 1));
            }
            Ty::Unit => {}
        }
    }

    // ── Print helpers ──────────────────────────────────────────────────────────

    pub(super) fn emit_print(&mut self, ty: Ty, scope: &LocalScope, f: &mut Function) {
        // Scalars render through the stdlib `String` constructor family
        // (`canon/string.can`) and then print as the string they
        // became — one print path, one newline convention.
        let ty = match ty {
            Ty::I64 => self.emit_render_to_str("Int", scope, f),
            Ty::F64 => self.emit_render_to_str("Float", scope, f),
            Ty::I32 => self.emit_render_to_str("Bool", scope, f),
            other => other,
        };
        match ty {
            Ty::Str | Ty::NamedStr(_) => {
                // print_str writes raw bytes — we always append a single `\n`
                // (the byte at `MEM_NEWLINE`) so `.print` produces one
                // line of output whether the receiver is a literal or a
                // host-returned string.
                f.instruction(&Instruction::Call(self.fn_print_str));
                f.instruction(&Instruction::I32Const(MEM_NEWLINE as i32));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::Call(self.fn_print_str));
            }
            Ty::I64
            | Ty::F64
            | Ty::I32
            | Ty::NamedPtr(_)
            | Ty::NamedPtrOf(_, _, _)
            | Ty::Ptr
            | Ty::List => {
                self.drop_value(ty, f); // unknown print — drop
            }
            Ty::Unit => {}
        }
    }

    pub(super) fn drop_value(&self, ty: Ty, f: &mut Function) {
        match ty {
            Ty::Unit => {}
            Ty::I64 | Ty::F64 | Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::Drop);
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::Drop);
                f.instruction(&Instruction::Drop);
            }
        }
    }

    // ── Main compile entry ─────────────────────────────────────────────────────

    pub(super) fn compile(&mut self) -> Vec<u8> {
        // Pre-passes
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();

        // Register the one waitable signature that isn't already covered
        // by the fixed TY_* slots: `(i32, i32) -> i32` for
        // `waitable-set.wait`. The other four intrinsics reuse existing
        // types (`waitable-set.new` = TY_RUN, `waitable.join` =
        // TY_PRINT_STR, `waitable-set.drop` and `subtask.drop` =
        // TY_PRINT_BOOL).
        let ty_waitable_set_wait =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
        // `subtask.cancel` has signature `(i32) -> (i32)` — takes a
        // subtask handle, returns the new CallState. Used by `race`'s
        // loser-cancel path.
        let ty_subtask_cancel = self.get_or_add_wasm_type(&[ValType::I32], &[ValType::I32]);
        // Reserve the wasm type for the list-to-json-array helper:
        // `(i32, i32) -> (i32, i32)`. Must be registered *before* the
        // type section is emitted below; the function section uses the
        // returned absolute index.
        let list_to_json_array_ty =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
        // String compare: `(ptr1, len1, ptr2, len2) -> i32`.
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        // Map + list-growth helper shapes.
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        // Reserve the loop block type used by `compile_list_map` /
        // `compile_list_filter` (see `list_loop_trio_ty`):
        // `(src, dst, remaining) -> (src, dst, remaining)`, all i32.
        // Block types must exist in the type section, which is emitted
        // before any user function body is compiled.
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );
        let ty_cabi_realloc = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        // A fused sequence's imports carry their own core signatures
        // (`fused_imports`); register them before the type section is
        // emitted.
        let fused_shapes: Vec<IndirectReturnShape> = self
            .extern_imports
            .iter()
            .filter_map(|e| e.indirect_return.clone())
            .collect();
        for shape in &fused_shapes {
            for import in fused_imports(shape) {
                self.get_or_add_wasm_type(import.params, import.results);
            }
        }
        // The stream stage type, and `$stream_next`'s index: after the
        // user functions and `cabi_realloc`. Both are needed by the
        // bodies, which compile next.
        let stage_ty = self.get_or_add_wasm_type(&[ValType::I32], &[ValType::I32; 3]);
        self.fn_stream_next = self.fn_user_start + self.compiled_user_funcs.len() as u32 + 1;

        // ── Code section ─────────────────────────────────────────────────────────────
        // Built before the module's sections are written: a body
        // registers the stream stages it pulls through, and the function
        // section counts them.
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_alloc());
        codes.function(&self.build_start());
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
        // User functions — one body per `compiled_user_funcs` entry, in
        // func-index order (matches the function section below exactly).
        let ordered_funcs: Vec<FunctionDef> = self
            .compiled_user_funcs
            .iter()
            .map(|(_, _, func)| func.clone())
            .collect();
        for func in ordered_funcs {
            let compiled = self.build_user_function(&func);
            codes.function(&compiled);
        }
        codes.function(&self.build_cabi_realloc());
        codes.function(&self.build_stream_next());
        for stage in self.build_stream_bodies() {
            codes.function(&stage);
        }

        let mut m = Module::new();

        // ── Type section ───────────────────────────────────────────────
        // Indices here must match the TY_* constants above.
        let mut types = TypeSection::new();
        // 0: print_str    (i32, i32) -> ()
        types.ty().function([ValType::I32, ValType::I32], []);
        // 1: print_bool   (i32) -> ()  — also used by waitable-set.drop,
        //                                  subtask.drop, task.return,
        //                                  stream.drop-writable,
        //                                  future.drop-readable
        types.ty().function([ValType::I32], []);
        // 2: run          () -> ()   (async-stackful lift; result via task.return)
        types.ty().function([], []);
        // 3: alloc        (i32) -> (i32)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 4: stdout write-via-stream  (i32 readable) -> (i32 future)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 5: stdout stream-new        () -> (i64 packed handles)
        types.ty().function([], [ValType::I64]);
        // 6: stdout stream-write      (i32 writable, i32 ptr, i32 len) -> (i32 status)
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
        // 7: handle return             () -> (i32)   — waitable-set.new
        types.ty().function([], [ValType::I32]);
        // User function types
        let user_sigs: Vec<_> = self.user_type_sigs.clone();
        for (params, results) in &user_sigs {
            types
                .ty()
                .function(params.iter().cloned(), results.iter().cloned());
        }
        m.section(&types);

        // ── Import section ───────────────────────────────────────────────────
        // Named the way `wit-component` reads a core module against its
        // world (`component::wrap_cli`): an interface's functions live
        // under `<iface>@<version>`, the canonical builtins a function's
        // streams and futures need are `[stream-new-N]<fn>` and kin
        // beside it, an async import is `[async-lower]<fn>`, and the
        // task intrinsics sit under `$root`.
        //   - wasi:cli/stdout: `write-via-stream` and the four builtins
        //         `print_str` stitches into the native WASI P3 stdout
        //         sequence.
        //   - one slot group per user `extern Wasm` declaration (sorted)
        //   - the 7 waitable/task intrinsics
        let mut imports = ImportSection::new();
        imports.import(
            STDOUT_MODULE,
            "write-via-stream",
            EntityType::Function(TY_STDOUT_WRITE_VIA_STREAM),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-new-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-write-0]write-via-stream",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            STDOUT_MODULE,
            "[stream-drop-writable-0]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            STDOUT_MODULE,
            "[future-drop-readable-1]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            STDOUT_MODULE,
            "[future-read-1]write-via-stream",
            EntityType::Function(ty_waitable_set_wait), // (i32, i32) -> i32
        );
        for ext in &self.extern_imports.clone() {
            if matches!(
                ext.indirect_return,
                Some(IndirectReturnShape::StreamWrite { .. })
            ) {
                continue;
            }
            let type_idx = *self
                .user_type_map
                .get(&(ext.params.clone(), ext.results.clone()))
                // invariant: `assign_func_indices` registers every extern
                // import's (params, results) signature in `user_type_map`.
                .expect("extern import type was added during assign_func_indices");
            let name = if ext.is_async {
                format!("[async-lower]{}", ext.fn_name)
            } else {
                ext.fn_name.clone()
            };
            imports.import(
                &ext.component_namespace,
                &name,
                EntityType::Function(type_idx),
            );
            if ext.stream_read_fn.is_some() {
                imports.import(
                    &ext.component_namespace,
                    &format!("[stream-read-0]{}", ext.fn_name),
                    EntityType::Function(TY_STDOUT_STREAM_WRITE), // (i32, i32, i32) -> (i32)
                );
                imports.import(
                    &ext.component_namespace,
                    &format!("[stream-drop-readable-0]{}", ext.fn_name),
                    EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
                );
                imports.import(
                    &ext.component_namespace,
                    &format!("[future-drop-readable-1]{}", ext.fn_name),
                    EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
                );
            }
            if let Some(shape) = &ext.indirect_return {
                for import in fused_imports(shape) {
                    let type_idx = self.get_or_add_wasm_type(import.params, import.results);
                    imports.import(import.module, import.name, EntityType::Function(type_idx));
                }
            }
        }
        // Waitable intrinsics — see field doc on `fn_waitable_*`.
        imports.import(
            "$root",
            "[waitable-set-new]",
            EntityType::Function(TY_HANDLE_RETURN), // () -> i32
        );
        imports.import(
            "$root",
            "[waitable-join]",
            EntityType::Function(TY_PRINT_STR), // (i32, i32) -> ()
        );
        imports.import(
            "$root",
            "[waitable-set-wait]",
            EntityType::Function(ty_waitable_set_wait), // (i32, i32) -> i32
        );
        imports.import(
            "$root",
            "[waitable-set-drop]",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "$root",
            "[subtask-drop]",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        // `task.return` for the async-stackful `run` lift: the bare
        // `result` discriminant.
        imports.import(
            &format!("[export]{}", component::WASI_CLI_RUN),
            "[task-return]run",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "$root",
            "[subtask-cancel]",
            EntityType::Function(ty_subtask_cancel), // (i32) -> (i32)
        );
        m.section(&imports);

        // ── Function section ─────────────────────────────────────────────────────────
        // Defined functions in the order they appear in the function index
        // space (right after the import block).
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_ALLOC);
        funcs.function(TY_RUN); // exported run() -> i32
        funcs.function(list_to_json_array_ty); // list → json array helper
        funcs.function(str_cmp_ty); // string compare -> -1/0/1
        funcs.function(list_append_ty); // list append
        funcs.function(list_concat_ty); // list concat
                                        // User-compiled functions only — extern imports are already declared
                                        // in the import section and must NOT get a defined-function slot.
                                        // `compiled_user_funcs` is the single source of truth shared
                                        // with the code section below: one entry per compiled body, in
                                        // func-index order, immune to `func_table` key collisions
                                        // (constructor families register several bodies per name).
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
        funcs.function(ty_cabi_realloc); // cabi_realloc
        funcs.function(stage_ty); // $stream_next, then the stages
        for _ in &self.stream_stages {
            funcs.function(stage_ty);
        }
        m.section(&funcs);
        m.section(&self.stream_table_section());

        // ── Memory / globals: self-contained ─────────────────────────
        // Sized to fit the static string pool — see `heap_layout`.
        let (heap_start, min_pages) = heap_layout(self.strings.data.len());
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: min_pages as u64,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        m.section(&memories);
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(heap_start as i32),
        );
        m.section(&globals);

        // ── Export section ─────────────────────────────────────────────────────────────
        // `wit-component` lifts the entry as `wasi:cli/run.run`,
        // async-stackful: the core signature is `() -> ()` and the
        // result travels through `task.return`.
        let cabi_realloc_idx = self.fn_user_start + self.compiled_user_funcs.len() as u32;
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("cabi_realloc", ExportKind::Func, cabi_realloc_idx);
        exports.export(
            &format!("[async-lift-stackful]{}#run", component::WASI_CLI_RUN),
            ExportKind::Func,
            self.fn_start,
        );
        m.section(&exports);
        m.section(&self.stream_element_section());
        m.section(&codes);

        // ── Data section ──────────────────────────────────────────────────────
        let mut data = DataSection::new();
        // '\n' at offset MEM_NEWLINE
        data.active(0, &ConstExpr::i32_const(MEM_NEWLINE as i32), *b"\n");
        if !self.strings.data.is_empty() {
            data.active(
                0,
                &ConstExpr::i32_const(MEM_STR_START as i32),
                self.strings.data.clone(),
            );
        }
        m.section(&data);

        m.finish()
    }
}

/// The declared type name an expression carries on its face: a `Foo(x)`
/// constructor, a `Foo`-named identifier or product field (values are
/// named after their types — there are no other value names), or a
/// piped construction `x -> Foo`. Scalar-newtype erasure drops the name
/// from the compiled value, so this is how a receiver's static type
/// survives to method lookup (`static_recv_type`) and the
/// `String(Byte)` conversion (`expr_is_byte`).
pub(super) fn syntactic_type_name(e: &Expr) -> Option<&str> {
    match e {
        Expr::Constructor { name, .. } => Some(&name.name),
        Expr::Ident(id) => Some(&id.name),
        // A repetition component (`Limbs.1`) is one `Limbs`.
        Expr::FieldAccess {
            receiver, field, ..
        } => match (field.name.parse::<u64>().is_ok(), receiver.as_ref()) {
            (true, Expr::Ident(id)) => Some(&id.name),
            _ => Some(&field.name),
        },
        Expr::MethodCall {
            method,
            piped: true,
            ..
        } => Some(&method.name),
        _ => None,
    }
}
