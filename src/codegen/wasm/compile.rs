//! Expression, statement, and dispatch compilation, plus the intrinsic
//! function builders (print helpers, alloc, list ops) and the CLI-world
//! module assembler. This is the heart of codegen: it walks the AST and
//! emits core WASM for every Canon construct.
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
    /// (a private module-import name the component wrapper backs with a
    /// synthetic core instance — see `component::wrap`).
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

    pub(super) fn build_print_int(&self) -> Function {
        let mut f = Function::new([
            (1, ValType::I32), // local 1: ptr
            (1, ValType::I32), // local 2: is_neg
            (1, ValType::I32), // local 3: digit
        ]);
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64LtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'-' as i32));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32 + 1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        f
    }

    /// Build the `fn_int_to_str` helper: `(i64) -> (i32, i32)`.
    ///
    /// Renders the value's decimal digits into the shared int buffer
    /// (same backward digit loop as `build_print_int`, minus the
    /// trailing-newline convention), then copies them into a fresh
    /// `$alloc` block so the result survives later renders. Returns
    /// the `(ptr, len)` pair of the copy.
    pub(super) fn build_int_to_str(&self) -> Function {
        let store8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        let load8 = store8;
        // Locals: 0 = value (param, i64); 1 = ptr; 2 = is_neg;
        // 3 = digit; 4 = len; 5 = dst; 6 = out_ptr.
        let mut f = Function::new([(6, ValType::I32)]);
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(1));
        // Negative? Remember the sign, work on the magnitude. (i64::MIN
        // survives this: `0 - i64::MIN` wraps back to itself and the
        // unsigned digit loop below reads it as the correct magnitude.)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64LtS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::End);
        // Zero short-circuits to a single '0' digit.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Leading '-' for negatives.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(b'-' as i32));
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::End);
        // len = MEM_INT_BUF_END - ptr (the '\n' at END is print_int's
        // convention, not part of the rendered value).
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(4));
        // out_ptr = alloc(len); dst = out_ptr.
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(6));
        f.instruction(&Instruction::LocalSet(5));
        // Copy loop: while ptr < MEM_INT_BUF_END { *dst++ = *ptr++ }.
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Load8U(load8));
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        // Return (out_ptr, len).
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(4));
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

    pub(super) fn build_print_bool(&self) -> Function {
        // invariant: `collect_all_strings` unconditionally interns the
        // "False"/"True" literals before any function body is built.
        let (fp, fl) = self.strings.get("False").expect("False interned");
        let (tp, tl) = self.strings.get("True").expect("True interned");
        let mut f = Function::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(tp as i32));
        f.instruction(&Instruction::I32Const(tl as i32));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::I32Const(fp as i32));
        f.instruction(&Instruction::I32Const(fl as i32));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        // Trailing newline (the shared '\n' byte), same as every other
        // `.print` path — one call, one line.
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Call(self.fn_print_str));
        f.instruction(&Instruction::End);
        f
    }

    /// Formats an `f64` into the shared int buffer and prints it.
    ///
    /// Output format: fixed-point with up to 6 fraction digits,
    /// trailing zeros trimmed; whole values print with no fraction
    /// (`2.0` → `2`); specials print as `NaN`, `Inf`, `-Inf`. Values
    /// whose integer part exceeds `u64::MAX` saturate (the buffer is
    /// sized for 20 integer digits). This is a pragmatic decimal
    /// rendering, not shortest-round-trip dtoa — good enough until a
    /// proper Grisu/Ryū port becomes worth the code size.
    ///
    /// Locals: 0 = value (param, f64), 1 = int_part (i64),
    /// 2 = frac (i64), 3 = ptr, 4 = digit, 5 = neg, 6 = started,
    /// 7 = counter (i32).
    pub(super) fn build_print_float(&self) -> Function {
        const PTR: u32 = 3;
        let mut f = Function::new([(2, ValType::I64), (5, ValType::I32)]);
        let store8 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        // Helper closure: ptr -= 1; mem[ptr] = byte.
        let push_byte = |f: &mut Function, byte: u8| {
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(1));
            f.instruction(&Instruction::I32Sub);
            f.instruction(&Instruction::LocalSet(PTR));
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(byte as i32));
            f.instruction(&Instruction::I32Store8(store8));
        };
        // Helper closure: print buffer [ptr, MEM_INT_BUF_END] (the byte
        // at MEM_INT_BUF_END is the shared '\n').
        let flush = |f: &mut Function, print_str: u32| {
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32 + 1));
            f.instruction(&Instruction::LocalGet(PTR));
            f.instruction(&Instruction::I32Sub);
            f.instruction(&Instruction::Call(print_str));
        };

        // ptr = MEM_INT_BUF_END
        f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
        f.instruction(&Instruction::LocalSet(PTR));

        // NaN: value != value.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'N');
        push_byte(&mut f, b'a');
        push_byte(&mut f, b'N');
        flush(&mut f, self.fn_print_str);
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // neg = value < 0; value = |value|
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(0.0.into()));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Neg);
        f.instruction(&Instruction::LocalSet(0));
        f.instruction(&Instruction::End);

        // Inf.
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(f64::INFINITY.into()));
        f.instruction(&Instruction::F64Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'f');
        push_byte(&mut f, b'n');
        push_byte(&mut f, b'I');
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'-');
        f.instruction(&Instruction::End);
        flush(&mut f, self.fn_print_str);
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // int_part = trunc_sat(value)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64TruncSatF64U);
        f.instruction(&Instruction::LocalSet(1));
        // frac = trunc_sat((value - int_part) * 1e6 + 0.5)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64ConvertI64U);
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::F64Const(1e6.into()));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Const(0.5.into()));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::I64TruncSatF64U);
        f.instruction(&Instruction::LocalSet(2));
        // Rounding carry: frac == 1_000_000 → bump int_part.
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(1_000_000));
        f.instruction(&Instruction::I64GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(1));
        f.instruction(&Instruction::I64Add);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::End);

        // Fraction digits, least-significant first, trailing zeros
        // skipped until the first significant digit (`started`).
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(2));
        // Skip while nothing started and digit is zero.
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::Br(1)); // continue the loop
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(PTR));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Decimal point (only when fraction digits were written).
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'.');
        f.instruction(&Instruction::End);

        // Integer digits (same shape as `build_print_int`).
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'0');
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(PTR));
        f.instruction(&Instruction::LocalGet(PTR));
        f.instruction(&Instruction::I32Const(b'0' as i32));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(store8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Sign.
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::If(BlockType::Empty));
        push_byte(&mut f, b'-');
        f.instruction(&Instruction::End);

        flush(&mut f, self.fn_print_str);
        f.instruction(&Instruction::End);
        f
    }

    /// Emit an in-place byte-copy loop reading from `scope.rbool()`
    /// (src) and writing to `scope.rptr()` (dst) for `scope.rlen()`
    /// (n) bytes. All three locals are modified by the loop (dst++,
    /// src++, n--), so they must be set up by the caller and not
    /// relied on after the call returns.
    ///
    /// This exists as a stand-in for `memory.copy` (bulk-memory
    /// proposal) because the component wrapper in `component::wrap`
    /// currently doesn't propagate the bulk-memory feature through
    /// to the synthesised core instance, so emitting `MemoryCopy`
    /// directly fails component validation. A future PR can swap
    /// this for `memory.copy` once the wrapper signs off on the
    /// feature — the call sites in `concat` won't need to change.
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
        // Locate the entry (`main`) and detect the canonical CLI shape
        // `Args => Exit`, whose single `Args` param is the argument
        // vector (`Unit => Program` and friends have no param).
        let main_func: Option<FunctionDef> = self.ast.items.iter().find_map(|item| {
            if let Item::Function(func) = item {
                if func.name.name == "main" && func.receiver.is_none() {
                    return Some(func.clone());
                }
            }
            None
        });
        let has_args = main_func
            .as_ref()
            .is_some_and(|func| crate::ast::is_args_entry_param(&func.params));

        // `Args` is `List<String>` — two i32 locals (ptr, len) laid down
        // before the scratch block. Shifting `param_count` keeps the
        // scratch-local accessors (`rptr()` = `param_count`, …) aligned
        // with the two prepended slots.
        let mut locals = extra_locals_decl();
        let mut scope = LocalScope::empty();
        if has_args {
            locals.insert(0, (2, ValType::I32)); // Args: (ptr, len)
            scope.param_count = 2;
        }
        let mut f = Function::new(locals);

        if has_args {
            // Populate the `Args` local by invoking the `Args` nullary
            // constructor (`Unit => Args { getArguments() }` in
            // `canon/std`), which reads argv via
            // `wasi:cli/environment#get-arguments` and leaves the decoded
            // `List<String>` (ptr, len) on the stack. Compiled before
            // `Args` is registered in `scope` so the constructor call
            // can't alias the local it fills.
            let call = Expr::Constructor {
                name: crate::ast::Ident {
                    name: "Args".to_string(),
                    span: crate::error::Span::default(),
                },
                args: Vec::new(),
                span: crate::error::Span::default(),
            };
            let ty = self.compile_expr(&call, &scope, &mut f);
            debug_assert!(matches!(ty, Ty::List), "Args() must produce a list");
            f.instruction(&Instruction::LocalSet(1)); // len (top of stack)
            f.instruction(&Instruction::LocalSet(0)); // ptr
            scope.vars.insert("Args".to_string(), (0, Ty::List));
            // `List` is the underlying-type alias, so a body that pipes
            // the argv through a list builtin (`Args -> At(1)`) resolves.
            scope.vars.insert("List".to_string(), (0, Ty::List));
        }

        let result_ty = main_func
            .as_ref()
            .map(|func| self.compile_block_return(&func.body, &scope, &mut f));
        // Deliver the run `result` discriminant to the component-level
        // caller via `task.return` (0 = ok, 1 = err). This must precede
        // `End` and is how the async-stackful lift signals completion.
        match result_ty {
            // `Args => Exit`: the returned `Exit` (`= Int`) is the exit
            // status. WASI `run` returns a bare `result`, which can only
            // encode success/failure — so `Exit(0)` maps to ok (exit 0)
            // and any nonzero code to err (exit 1). An exact nonzero code
            // needs the hard `Exited(n)` (`exit-with-code`) escape hatch.
            Some(Ty::I64) => {
                f.instruction(&Instruction::I64Const(0));
                f.instruction(&Instruction::I64Ne); // i32: 1 when nonzero
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
            // `Unit => Program` and other Unit-world entries: nothing to
            // report — always ok.
            Some(other) => {
                self.drop_value(other, &mut f);
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
            None => {
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::Call(self.fn_task_return));
            }
        }
        f.instruction(&Instruction::End);
        f
    }

    pub(super) fn build_user_function(&mut self, func: &FunctionDef) -> Function {
        let (params, scope) = self.build_local_scope(func);
        let _ = params; // params are implicit in the function type
        let mut f = Function::new(extra_locals_decl());
        let body = func.body.clone();
        // `?` may early-return the whole Result/Option value, but only
        // when the enclosing function itself returns the same shape
        // (one i32 pointer at the core level). Record which kind for
        // the duration of this body.
        let ret = self.resolve_return_ty(func);
        self.cur_fn_early_return = match &ret {
            Ty::NamedPtrStr(n, _, _) | Ty::NamedPtr(n) if n == "Result" => Some("Result"),
            Ty::NamedPtr(n) if n == "Option" => Some("Option"),
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
    /// returns `["String"]`. Bounded by `resolve_repr_depth`'s 20-step
    /// guard so a malformed cycle can't infinite-loop.
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
                if !generics.is_empty() {
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
            Expr::HexLit { value, .. } => {
                f.instruction(&Instruction::I64Const(*value as i64));
                Ty::I64
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
                let recv_ty = self.compile_expr(receiver, scope, f);
                if let Some(unwrapped) = newtype_unwrap_ty(&recv_ty, &field.name) {
                    return unwrapped;
                }
                // Product field access: the receiver is a heap pointer
                // to a struct laid out by `build_product_value`. Read
                // back from the matching byte offset.
                if let Ty::NamedPtr(product_name) = &recv_ty {
                    if self
                        .type_defs
                        .get(product_name)
                        .is_some_and(|t| matches!(t, TypeExpr::Product { .. }))
                    {
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
                //   - `Ty::NamedPtrStr(_, _, _)` → `(i32 ptr, i32 len)` at offsets 4 and 8.
                //   - `Ty::NamedPtr("Result"|"Option")` → `i64` at offset 4 (legacy).
                match &inner_ty {
                    Ty::NamedPtrStr(container, ok_name, _) => {
                        let container = container.clone();
                        let ok_name = ok_name.clone();
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        if self.cur_fn_early_return == Some(container.as_str()) {
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
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I32Load(MemArg {
                            offset: 4,
                            align: 2,
                            memory_index: 0,
                        }));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::I32Load(MemArg {
                            offset: 8,
                            align: 2,
                            memory_index: 0,
                        }));
                        // Preserve the Canon-level type of the Ok payload
                        // so subsequent method calls dispatch correctly
                        // (e.g. `.read()` on `File` after
                        // `Path(…).File()?`). `Ty::Str` for a bare
                        // `Result<String, String>`; `Ty::NamedStr(name)`
                        // for any aliased payload type.
                        if ok_name == "String" {
                            Ty::Str
                        } else {
                            Ty::NamedStr(ok_name)
                        }
                    }
                    Ty::NamedPtr(n) if n == "Result" || n == "Option" => {
                        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                        if self.cur_fn_early_return == Some(n.as_str()) {
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
            // (Static fragments) and `MethodCall { method: "ToJson" }`
            // (Interp expressions), then compile that. This reuses the
            // existing `concat` builtin and the existing `ToJson` trait
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
            // `.ToHtml()` calls (escaping for `String`/`Int` via the
            // stdlib's `text()`, identity for `Html` — see
            // `packages/canon/std/src/web/html.can`).
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

    // ── Constructor compilation ────────────────────────────────────────────────

    /// Static byte-ness test for the `String(Byte)` conversion. `Byte`
    /// erases to i64 at the value level (same repr as `Int`), so the
    /// two Int→String conversions — decimal rendering vs. single-byte
    /// string — are told apart by the *declared* type at the call
    /// site: a `Byte(…)` constructor, an identifier bound under a
    /// name whose alias chain passes through `Byte`, or a field
    /// access unwrapping to `Byte`. A method chain that returns
    /// `Byte` erases before it gets here — wrap it
    /// (`Byte(x).String()`) to pick the byte reading; needing the
    /// wrap to mean the other thing is exactly why the newtype
    /// exists.
    pub(super) fn expr_is_byte(&self, e: &Expr) -> bool {
        let name = match e {
            Expr::Constructor { name, .. } => &name.name,
            Expr::Ident(id) => &id.name,
            Expr::FieldAccess { field, .. } => &field.name,
            // Piped construction: `65 -> Byte` builds a `Byte` just like
            // `Byte(65)`, but scalar-newtype erasure drops the name from
            // the value, so recover it from the constructor's spelling.
            Expr::MethodCall {
                method,
                piped: true,
                ..
            } => &method.name,
            _ => return false,
        };
        self.collect_alias_chain(name).iter().any(|n| n == "Byte")
    }

    /// Converts the i64 byte value on the stack into a fresh one-byte
    /// string — the value half of `String(Byte)`. The value is masked
    /// to its low 8 bits.
    pub(super) fn emit_byte_to_str(&mut self, scope: &LocalScope, f: &mut Function) -> Ty {
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(0xFF));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1));
        Ty::Str
    }

    pub(super) fn compile_constructor(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        match name {
            // Bool variants
            "True" => {
                f.instruction(&Instruction::I32Const(1));
                Ty::I32
            }
            "False" => {
                f.instruction(&Instruction::I32Const(0));
                Ty::I32
            }
            // Unit
            "Unit" => Ty::Unit,
            // ── HTTP-mode constructors ────────────────────────────────
            // `Headers()` and `Response(Headers * Status)` compile to
            // real `wasi:http/types` calls (see `compile_http`). The
            // stdlib's binding declarations for these names exist only
            // for the checker; codegen owns the calling convention.
            "Headers" if self.http_mode => {
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
                Ty::NamedPtr("Headers".to_string())
            }
            "Response" if self.http_mode => self.build_http_response(args, scope, f),
            // Primitive constructors. Identity when the argument
            // already has the target representation (`Int(1)`,
            // `String("x")`) — compiling it IS the construction —
            // and *conversion* when it doesn't (`String(42)` renders
            // decimal, `String(Byte(65))` is the one-byte string
            // "A"): conversion is construction, see the language spec
            // (docs/src/spec/). The zero-arg forms produce the type's
            // zero value.
            "Int" | "Float" | "String" => {
                if let Some(a) = args.first() {
                    let is_byte = name == "String" && self.expr_is_byte(a);
                    let ty = self.compile_expr(a, scope, f);
                    match (name, &ty) {
                        // Tolerate `Int(bool)` / `Float(int)` shape
                        // drift by widening rather than corrupting the
                        // stack.
                        ("Int", Ty::I32) => {
                            f.instruction(&Instruction::I64ExtendI32S);
                            Ty::I64
                        }
                        ("Float", Ty::I64) => {
                            f.instruction(&Instruction::F64ConvertI64S);
                            Ty::F64
                        }
                        ("String", Ty::I64) => {
                            if is_byte {
                                self.emit_byte_to_str(scope, f)
                            } else {
                                f.instruction(&Instruction::Call(self.fn_int_to_str));
                                Ty::Str
                            }
                        }
                        ("Int", ty) if ty.is_str_like() => {
                            // `Int("42")` — the fallible parse constructor
                            // from `canon/std/Int`. The compiled string is
                            // already on the stack, exactly where
                            // `emit_func_table_call` expects the receiver.
                            if let Some(info) = self
                                .func_table
                                .get(&(Some("String".to_string()), "Int".to_string()))
                                .cloned()
                            {
                                return self.emit_func_table_call(&info, &[], scope, f);
                            }
                            // Parser not in scope — the checker rejects
                            // this; keep the stack shape sane regardless.
                            self.drop_value(Ty::Str, f);
                            f.instruction(&Instruction::I64Const(0));
                            Ty::I64
                        }
                        _ => ty,
                    }
                } else {
                    match name {
                        "Int" => {
                            f.instruction(&Instruction::I64Const(0));
                            Ty::I64
                        }
                        "Float" => {
                            f.instruction(&Instruction::F64Const(0.0.into()));
                            Ty::F64
                        }
                        _ => {
                            let (ptr, len) = self.strings.intern("");
                            f.instruction(&Instruction::I32Const(ptr as i32));
                            f.instruction(&Instruction::I32Const(len as i32));
                            Ty::Str
                        }
                    }
                }
            }
            // Option built-ins
            "None" => self.build_option_none(f),
            "Some" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_option_some(payload_ty, scope, f)
            }
            // Result built-ins
            "Ok" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_result_ok(payload_ty, scope, f)
            }
            "Err" => {
                let payload_ty = if !args.is_empty() {
                    self.compile_expr(&args[0], scope, f)
                } else {
                    Ty::Unit
                };
                self.build_result_err(payload_ty, scope, f)
            }
            // List constructor: List(e1, e2, e3, ...)
            "List" => self.build_list_literal(args, scope, f),
            // NOTE: `Map()` / `Set()` are NOT built in — they are the
            // pure-Canon `canon/std/Map` / `canon/std/Set` recursive
            // unions, whose zero-arg `Self` constructors resolve
            // through the ordinary user-defined path below.
            // NOTE: the concurrency combinators (`parallel` / `race`) are
            // *methods* — `a.parallel(b)` — handled at the top of
            // `compile_method_call`. The checker rejects the bare call
            // form, so no Constructor arm exists for them here.
            // User-defined types
            _ => {
                // 1. Union variant constructor (e.g. `Branch(...)`, `Leaf`).
                if let Some(parent) = self.variant_parent.get(name).cloned() {
                    let tag = self.variant_tag[name];
                    let total = self.union_total_size(&parent);
                    return self.build_union_value(&parent, name, tag, total, args, scope, f);
                }

                // 2. Free function with this name (no receiver). Lets the
                //    user write zero-arg constructors like `Now()` or
                //    `RandomInt()` that the stdlib declares as
                //    `Name = () -> Name` via `extern Wasm`.
                if args.is_empty() {
                    if let Some(info) = self.func_table.get(&(None, name.to_string())).cloned() {
                        return self.emit_func_table_call(&info, &[], scope, f);
                    }
                    // `Name = () -> Name` is normalised by the parser into
                    // a `Self`-named method with receiver `Name` (see
                    // `resolve_new_syntax`). Dispatch a bare `Name()` call
                    // through that key.
                    if let Some(info) = self
                        .func_table
                        .get(&(Some(name.to_string()), "Self".to_string()))
                        .cloned()
                    {
                        return self.emit_func_table_call(&info, &[], scope, f);
                    }
                }

                // 3. Constructor declared as a method on the first arg's
                //    type — lets `Url("http://…")` dispatch to
                //    `Url = (String) -> Result<…>`, and selects the right
                //    member of a constructor *family* (`Json = (Bool) ->
                //    Json` vs `Json = (Int) -> Json`) by the argument's
                //    type. Both call shapes reach here: `Value(map, k)`
                //    (positional) and `Value(map * k)` (product value) —
                //    the product form is flattened so its first field
                //    drives the lookup and the rest ride as ordinary
                //    trailing args.
                if !args.is_empty() {
                    let flat: Vec<Expr> = if args.len() == 1 {
                        if let Expr::ProductValue { fields, .. } = &args[0] {
                            fields.clone()
                        } else {
                            args.to_vec()
                        }
                    } else {
                        args.to_vec()
                    };
                    if let Some(first_ty) = self.infer_ctor_arg_type_name(&flat[0]) {
                        // The declared param type may sit anywhere on the
                        // arg's widening chain: the exact name, the
                        // variant's parent union (`True()` fills a `Bool`
                        // param), or a newtype's underlying type.
                        let mut candidates: Vec<String> = vec![first_ty.clone()];
                        if let Some(parent) = self.variant_parent.get(&first_ty) {
                            candidates.push(parent.clone());
                        }
                        for link in self.collect_alias_chain(&first_ty) {
                            if !candidates.contains(&link) {
                                candidates.push(link);
                            }
                        }
                        for cand in candidates {
                            let key = (Some(cand), name.to_string());
                            if let Some(info) = self.func_table.get(&key).cloned() {
                                // Compile the first arg (this becomes the
                                // receiver) and dispatch with the rest.
                                let _ = self.compile_expr(&flat[0], scope, f);
                                return self.emit_func_table_call(&info, &flat[1..], scope, f);
                            }
                        }
                    }
                }

                // 4. Type-def newtype / product constructor.
                if self.type_defs.contains_key(name) {
                    let body = self.type_defs.get(name).cloned().unwrap();
                    return match &body {
                        TypeExpr::Product { .. } => {
                            // Product type. Two surface shapes reach here:
                            //   * `Name(a * b * c)` — one arg, an
                            //     `Expr::ProductValue` whose fields are
                            //     the positional field values.
                            //   * `Name(a, b, c)` — N comma-separated args
                            //     in declaration (alphabetical) order.
                            // Both route through `build_product_value`,
                            // which allocates the struct, lays each field
                            // out at its byte offset, and returns the
                            // pointer typed as `Ty::NamedPtr(name)`.
                            // Anything else (mismatched arity, an empty
                            // call) falls through to the legacy
                            // side-effect-only path so we don't regress
                            // existing programs.
                            let layout = self.product_field_layout(name);
                            if args.len() == 1 {
                                if let Expr::ProductValue { fields, .. } = &args[0].clone() {
                                    if fields.len() == layout.len() {
                                        return self.build_product_value(name, fields, scope, f);
                                    }
                                }
                            }
                            if !layout.is_empty() && args.len() == layout.len() {
                                return self.build_product_value(name, args, scope, f);
                            }
                            for a in args {
                                let ty = self.compile_expr(a, scope, f);
                                self.drop_value(ty, f);
                            }
                            Ty::Unit
                        }
                        _ => {
                            // Newtype alias: transparent — compile the arg and re-wrap.
                            let repr = self.resolve_repr(name);
                            if !args.is_empty() {
                                let arg_ty = self.compile_expr(&args[0], scope, f);
                                match &repr {
                                    Ty::NamedStr(_) => {
                                        let _ = arg_ty;
                                        Ty::NamedStr(name.to_string())
                                    }
                                    Ty::NamedPtr(_) => {
                                        let _ = arg_ty;
                                        Ty::NamedPtr(name.to_string())
                                    }
                                    _ => {
                                        let _ = arg_ty;
                                        repr
                                    }
                                }
                            } else {
                                Ty::Unit
                            }
                        }
                    };
                }

                // 5. Unknown: compile args for side effects.
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                Ty::Unit
            }
        }
    }

    /// `infer_static_type_name` extended for constructor-argument routing:
    /// also resolves bare identifiers. In Canon an identifier in expression
    /// position *is* a type name — parameters and dispatch-arm payloads are
    /// referenced by the type they bind (there are no local variables) — so
    /// the name itself is the best static type available. Kept separate from
    /// `infer_static_type_name` so the method-call and async-classification
    /// call sites keep their conservative behavior.
    /// Static result type of a builtin-vocabulary method. Comparisons
    /// yield `Bool`; the numeric operations preserve their receiver's
    /// type (`Int` stays `Int`, `Float` stays `Float`); the string and
    /// index operations yield `String` / `Int`. Returns `None` for
    /// anything not in the builtin vocabulary, so a user shape of the
    /// same name (resolved earlier via `func_table`) always wins.
    pub(super) fn builtin_result_type(&self, method: &str, receiver: &Expr) -> Option<String> {
        match method {
            "Eq" | "Ne" | "Lt" | "Le" | "Gt" | "Ge" | "And" | "Or" | "Not" => {
                Some("Bool".to_string())
            }
            "Length" | "ByteAt" => Some("Int".to_string()),
            "Joined" | "Substring" => Some("String".to_string()),
            // List transforms preserve list-ness (`map`/`append`).
            "Mapped" | "Appended" => Some("List".to_string()),
            "Sum" | "Difference" | "Product" | "Quotient" | "Remainder" | "Minimum" | "Maximum"
            | "Negated" => self.infer_ctor_arg_type_name(receiver),
            _ => None,
        }
    }

    pub(super) fn infer_ctor_arg_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.name.clone()),
            // Newtype unwrap (`x.String`) — a PascalCase field names the
            // component's type, which *is* the value's type.
            Expr::FieldAccess { field, .. }
                if field.name.chars().next().is_some_and(char::is_uppercase) =>
            {
                Some(field.name.clone())
            }
            // A method chain's static type comes from the callee's
            // registered result type — this is what lets a pipe hang off
            // a chain (`Map().Inserted("a", "1") -> Keys`). Builtin
            // methods aren't in `func_table`, so chains ending in them
            // still return `None` and the call falls through to the
            // pre-pipe routing paths.
            Expr::MethodCall {
                receiver, method, ..
            } => {
                let recv = self.infer_ctor_arg_type_name(receiver)?;
                let mut cands: Vec<String> = vec![recv.clone()];
                if let Some(p) = self.variant_parent.get(&recv) {
                    cands.push(p.clone());
                }
                for link in self.collect_alias_chain(&recv) {
                    if !cands.contains(&link) {
                        cands.push(link);
                    }
                }
                for c in cands {
                    if let Some(info) = self.func_table.get(&(Some(c), method.name.clone())) {
                        return match &info.result_ty {
                            Ty::NamedPtr(n) | Ty::NamedStr(n) | Ty::NamedPtrStr(n, _, _) => {
                                Some(n.clone())
                            }
                            Ty::Str => Some("String".to_string()),
                            Ty::I64 => Some("Int".to_string()),
                            Ty::F64 => Some("Float".to_string()),
                            Ty::I32 => Some("Bool".to_string()),
                            Ty::List => Some("List".to_string()),
                            _ => None,
                        };
                    }
                }
                // Builtin vocabulary isn't in `func_table`; infer its
                // result type so a constructor family keyed on that type
                // still resolves through a builtin-terminated chain
                // (`Eq(5) -> TestResult`, `Sum(1) -> Digits`).
                if let Some(t) = self.builtin_result_type(&method.name, receiver) {
                    return Some(t);
                }
                // Piped construction: `X -> Foo` builds a `Foo` (a
                // variant widens to its union), so `7 -> Value` inside a
                // product binds to the `Value` field by type.
                if let Some(parent) = self.variant_parent.get(&method.name) {
                    return Some(parent.clone());
                }
                if self.type_defs.contains_key(&method.name) {
                    return Some(method.name.clone());
                }
                None
            }
            _ => self.infer_static_type_name(expr),
        }
    }

    /// Quick static inference of an expression's Canon-level type *name*,
    /// used to look up methods/constructors before compiling. Returns
    /// `Some("String")` for string literals, `Some("Int")` for ints, etc.;
    /// `None` when the static shape isn't obvious without full type checking.
    pub(super) fn infer_static_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StringLit { .. }
            | Expr::JsonLit { .. }
            | Expr::HtmlLit { .. }
            | Expr::FormatLit { .. } => Some("String".to_string()),
            Expr::IntLit { .. } | Expr::HexLit { .. } => Some("Int".to_string()),
            Expr::FloatLit { .. } => Some("Float".to_string()),
            Expr::Constructor { name, .. } => {
                // Use the constructor's name as a hint — sufficient for the
                // common case `Path("…").File()` where `File` is a method on
                // `Path`.
                Some(name.name.clone())
            }
            _ => None,
        }
    }

    pub(super) fn build_option_none(&self, f: &mut Function) -> Ty {
        // Alloc 12 bytes, tag=0
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        // dup on stack not easy; use store + reload pattern
        // Actually store tag and return ptr
        // f: [ptr]
        // We need to store tag=0 at [ptr+0] then return ptr
        // But we already consumed ptr to alloc, so we need a local.
        // ... this requires a local. Since we're in a context without a scope,
        // let's just emit the allocation inline and hope the caller has scratch space.
        // Simplification: don't set tag (it defaults to 0 in zeroed memory) and return ptr.
        Ty::NamedPtr("Option".to_string())
    }

    pub(super) fn build_option_some(
        &mut self,
        payload_ty: Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // payload is on stack; save in tmp
        self.save_to_scratch(payload_ty.clone(), scope, f);
        // alloc 12 bytes
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        // store tag=1
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        // store payload at offset 4
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Option".to_string())
    }

    pub(super) fn build_result_ok(
        &mut self,
        payload_ty: Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        self.save_to_scratch(payload_ty.clone(), scope, f);
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(1)); // Ok = tag 1
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Result".to_string())
    }

    pub(super) fn build_result_err(
        &mut self,
        payload_ty: Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        self.save_to_scratch(payload_ty.clone(), scope, f);
        f.instruction(&Instruction::I32Const(12));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(0)); // Err = tag 0
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        self.load_from_scratch(&payload_ty, scope, f);
        self.store_payload_at_offset(4, &payload_ty, scope, f);
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr("Result".to_string())
    }

    /// Build a union value (tag + payload). Returns Ty::NamedPtr(union_name).
    ///
    /// IMPORTANT: all field expressions are compiled BEFORE the union struct is
    /// allocated, so nested constructors (e.g. Branch containing Leaf()) can each
    /// use `scope.alloc_ptr()` without clobbering each other.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_union_value(
        &mut self,
        union_name: &str,
        variant_name: &str,
        tag: u32,
        total_size: u32,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let payload_start = 4u32;

        // ── Step 1: Compile all field values BEFORE allocating the union struct ──
        // This prevents nested constructors from overwriting scope.alloc_ptr().
        // We save up to 2 i32 fields and 1 i64 field to scratch locals.

        let layout = if !args.is_empty() {
            self.product_field_layout(variant_name)
        } else {
            vec![]
        };

        // ── Auto-boxed product payloads (the language spec, docs/src/spec/) ──
        //
        // A variant whose typedef is a multi-field product (`Link =
        // Label * Next` inside `Chain = Link + Stop`) stores ONE
        // pointer to a standalone product struct, not inline fields.
        // `build_product_value` already handles any field count and
        // arbitrarily nested constructors (including recursive
        // same-union values) via its operand-stack discipline, and the
        // indirection is exactly what makes recursive types finite.
        // The arm side reads the pointer back in `bind_arm_payload`'s
        // `NamedPtr` case, so field access on the bound name goes
        // through the ordinary `product_field_layout` offsets.
        if layout.len() >= 2 {
            let fields: Vec<Expr> = match args {
                [Expr::ProductValue { fields, .. }] => fields.clone(),
                _ => args.to_vec(),
            };
            self.build_product_value(variant_name, &fields, scope, f);
            // [product_ptr] — park it while the union struct allocates
            // (nothing below compiles user code, so tmp_i32 is safe).
            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            f.instruction(&Instruction::I32Const(total_size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::I32Const(tag as i32));
            f.instruction(&Instruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
            f.instruction(&Instruction::I32Store(MemArg {
                offset: payload_start as u64,
                align: 2,
                memory_index: 0,
            }));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            return Ty::NamedPtr(union_name.to_string());
        }

        // Encoded field types for the store pass below.
        //
        // `Str0` stashes a single string-shaped payload into `tmp_i32`
        // (ptr) and `tmp_i32_b` (len). It pairs with the dispatch-side
        // extraction in `compile_arm_body`, which reads back (ptr, len)
        // from offsets 4 and 8 of the union struct. Only one string
        // payload is supported per variant, which matches the
        // single-arg shape of newtype variants like `Fail = String`.
        #[derive(Clone, Copy)]
        enum SavedField {
            Ptr0,
            Ptr1,
            I64_0,
            F64_0,
            Str0,
            Dropped,
        }
        let mut saved: Vec<SavedField> = Vec::new();

        if !args.is_empty() {
            if !layout.is_empty() && args.len() == 1 {
                if let Expr::ProductValue { fields, .. } = &args[0].clone() {
                    let fields = fields.clone();
                    let mut ptr_count = 0usize;
                    let mut i64_count = 0usize;
                    for (i, _) in layout.iter().enumerate() {
                        if let Some(field_expr) = fields.get(i) {
                            let ty = self.compile_expr(field_expr, scope, f);
                            match &ty {
                                Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                                    if ptr_count == 0 {
                                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                                        saved.push(SavedField::Ptr0);
                                    } else {
                                        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                                        saved.push(SavedField::Ptr1);
                                    }
                                    ptr_count += 1;
                                }
                                Ty::I64 => {
                                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                                    saved.push(SavedField::I64_0);
                                    i64_count += 1;
                                }
                                Ty::F64 => {
                                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                                    saved.push(SavedField::F64_0);
                                    i64_count += 1;
                                }
                                _ => {
                                    self.drop_value(ty, f);
                                    saved.push(SavedField::Dropped);
                                }
                            }
                        }
                    }
                    let _ = (ptr_count, i64_count);
                } else {
                    // Single non-product arg
                    let arg = args[0].clone();
                    let ty = self.compile_expr(&arg, scope, f);
                    match &ty {
                        Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                            saved.push(SavedField::Ptr0);
                        }
                        Ty::I64 => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                            saved.push(SavedField::I64_0);
                        }
                        Ty::F64 => {
                            f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                            saved.push(SavedField::F64_0);
                        }
                        _ => {
                            self.drop_value(ty, f);
                            saved.push(SavedField::Dropped);
                        }
                    }
                }
            } else {
                // Direct single arg (non-layout case)
                let arg = args[0].clone();
                let ty = self.compile_expr(&arg, scope, f);
                match &ty {
                    Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        saved.push(SavedField::Ptr0);
                    }
                    Ty::I64 => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                        saved.push(SavedField::I64_0);
                    }
                    Ty::F64 => {
                        f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                        saved.push(SavedField::F64_0);
                    }
                    Ty::Str | Ty::NamedStr(_) => {
                        // Stack: [ptr, len]. Pop len first (top), then ptr.
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        saved.push(SavedField::Str0);
                    }
                    _ => {
                        self.drop_value(ty, f);
                        saved.push(SavedField::Dropped);
                    }
                }
            }
        }

        // ── Step 2: Allocate the union struct ────────────────────────────────────
        f.instruction(&Instruction::I32Const(total_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        // ── Step 3: Store the tag ─────────────────────────────────────────────────
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(tag as i32));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));

        // ── Step 4: Store field values from scratch locals ───────────────────────
        if !saved.is_empty() {
            if !layout.is_empty() {
                for (idx, sf) in saved.iter().enumerate() {
                    if let Some((_, field_repr, field_offset)) = layout.get(idx) {
                        let abs_offset = payload_start + field_offset;
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        match sf {
                            SavedField::Ptr0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::Ptr1 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::I64_0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::F64_0 => {
                                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                                self.store_value_at_offset(abs_offset, field_repr, scope, f);
                            }
                            SavedField::Str0 => {
                                // Forward-declared variant for string-typed
                                // union payloads (`Fail = String` style).
                                // The producer side isn't pushing this yet;
                                // when it does, the store will use
                                // `(tmp_i32, tmp_i32_b)` for `(ptr, len)`.
                                // For now, treat as Dropped to keep the
                                // match exhaustive without claiming we
                                // support it.
                                f.instruction(&Instruction::Drop); // drop the addr
                            }
                            SavedField::Dropped => {
                                f.instruction(&Instruction::Drop); // drop the addr
                            }
                        }
                    }
                }
            } else if let Some(sf) = saved.first() {
                // Single non-layout field
                match sf {
                    SavedField::Ptr0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: payload_start as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                    }
                    SavedField::I64_0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64Store(MemArg {
                            offset: payload_start as u64,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    SavedField::F64_0 => {
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                        f.instruction(&Instruction::F64Store(MemArg {
                            offset: payload_start as u64,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    SavedField::Str0 => {
                        // Store ptr at offset 4 (payload_start) and len at
                        // offset 8 (payload_start + 4). Layout matches
                        // what `compile_arm_body` and `?` expect.
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: payload_start as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                        f.instruction(&Instruction::I32Store(MemArg {
                            offset: (payload_start + 4) as u64,
                            align: 2,
                            memory_index: 0,
                        }));
                    }
                    _ => {}
                }
            }
        }

        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr(union_name.to_string())
    }

    /// Build a value-level product (`Foo(a * b * c)` or `Foo(a, b, c)`).
    ///
    /// Allocates one heap block sized to the product's field layout,
    /// then for each field: pushes the struct base, compiles the field
    /// expression, and stores the result at the field's byte offset.
    /// Returns the struct pointer typed as `Ty::NamedPtr(product_name)`,
    /// which downstream `Expr::FieldAccess` reads back from in
    /// `compile_expr` (matching offset via `product_field_layout`).
    ///
    /// Field expressions are assumed to be positional (same order as
    /// the type-level field declaration, which the parser preserves
    /// and the alphabetical-ordering rule pins).
    /// Every type `name` widens to, most-specific first: itself, its
    /// newtype-alias targets (`Value` → `String`), and — if it names a
    /// union variant — its parent union and that union's aliases
    /// (`Empty` → `Map`). This is the set a value of type `name` can
    /// satisfy, used to bind product values to fields by type rather
    /// than by position.
    pub(super) fn widening_chain(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for link in self.collect_alias_chain(name) {
            if !out.contains(&link) {
                out.push(link);
            }
        }
        if let Some(parent) = self.variant_parent.get(name) {
            for link in self.collect_alias_chain(parent) {
                if !out.contains(&link) {
                    out.push(link);
                }
            }
        }
        out
    }

    /// How well a value of type `value_name` fits a field of type
    /// `field_ty`: `2` when the field is the value's own type, or the
    /// value is a union member and the field is (an ancestor of) that
    /// union (`True` value → `Bool` field); `1` when they merely widen
    /// to a shared base type (`String` value → `Key` field, or `Value`
    /// value → `String` field, both erasing to `String`); `0` when
    /// unrelated. Newtypes are distinct types even along an alias chain
    /// (`A = B` doesn't make an `A` value an exact `B`), so only a
    /// value's own name or its union ancestry counts as exact — the
    /// erasure walked by `widening_chain` is shared-base only.
    pub(super) fn field_match_score(&self, value_name: &str, field_ty: &str) -> u8 {
        if value_name == field_ty {
            return 2;
        }
        if let Some(parent) = self.variant_parent.get(value_name) {
            if self
                .collect_alias_chain(parent)
                .iter()
                .any(|n| n == field_ty)
            {
                return 2;
            }
        }
        let value_chain = self.widening_chain(value_name);
        let field_chain = self.widening_chain(field_ty);
        if value_chain.iter().any(|n| field_chain.contains(n)) {
            return 1;
        }
        0
    }

    pub(super) fn build_product_value(
        &mut self,
        product_name: &str,
        field_exprs: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let layout = self.product_field_layout(product_name);
        let total_size: u32 = layout
            .iter()
            .map(|(name, _, _)| self.field_byte_size(name))
            .sum::<u32>()
            .max(4); // `alloc` expects a non-zero size.

        // ── Bind values to fields by type, not by position ────────────
        // Fields are alphabetical and construction is positionless
        // (`Node(String * Empty() * Value)` and `Node(Empty() * Value *
        // String)` build the same struct). Each value is routed to the
        // field whose type it best matches: an exact newtype match
        // (`Value` → the `Value` field) wins over a shared-base match
        // (a bare `String` → the `Key` field), and any leftovers fall
        // back to declaration order. Same-typed fields (map's `Key` and
        // `Value`, both `String`) are why newtypes matter — tag a value
        // `Value(x)` and it lands in the `Value` slot regardless of
        // where it was written.
        let n_fields = layout.len().min(field_exprs.len());
        let value_names: Vec<Option<String>> = field_exprs
            .iter()
            .map(|e| self.infer_ctor_arg_type_name(e))
            .collect();
        let mut used = vec![false; field_exprs.len()];
        let mut slot_val: Vec<Option<usize>> = vec![None; n_fields];
        // Pass 1 (exact) then pass 2 (shared-base): a slot claims the
        // first unused value that scores at the current threshold.
        for threshold in [2u8, 1u8] {
            for (si, (field_name, _, _)) in layout.iter().take(n_fields).enumerate() {
                if slot_val[si].is_some() {
                    continue;
                }
                if let Some(vi) = (0..field_exprs.len()).find(|&vi| {
                    !used[vi]
                        && value_names[vi]
                            .as_ref()
                            .is_some_and(|nm| self.field_match_score(nm, field_name) == threshold)
                }) {
                    slot_val[si] = Some(vi);
                    used[vi] = true;
                }
            }
        }
        // Pass 3 (positional): unresolved values fill remaining slots in
        // order — the pre-typed-construction behaviour, kept as a floor.
        for slot in slot_val.iter_mut().take(n_fields) {
            if slot.is_some() {
                continue;
            }
            if let Some(vi) = (0..field_exprs.len()).find(|&vi| !used[vi]) {
                *slot = Some(vi);
                used[vi] = true;
            }
        }

        // ── Allocate ──────────────────────────────────────────────────
        f.instruction(&Instruction::I32Const(total_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        // ── Pre-push base copies on the operand stack ─────────────────
        // A nested constructor inside any field expression (`Some("hi")`,
        // an inner product, …) reassigns `scope.alloc_ptr()`, so the
        // local can't be trusted after the first `compile_expr`. Values
        // already on the operand stack, however, sit safely below a
        // nested expression's own stack activity. So: one copy per
        // stored field (consumed bottom-up by the stores below) plus
        // one at the very bottom that survives as the result.
        for _ in 0..=n_fields {
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        }

        // ── Lay out each field ────────────────────────────────────────
        // The store helper accepts `[addr, value]` (scalar) or
        // `[addr, ptr, len]` (string) and consumes the address copy
        // pre-pushed above.
        for (i, (_field_name, field_repr, field_offset)) in layout.iter().take(n_fields).enumerate()
        {
            let vi = slot_val[i].unwrap_or(i);
            let _val_ty = self.compile_expr(&field_exprs[vi], scope, f);
            self.store_payload_at_offset(*field_offset, field_repr, scope, f);
        }

        // ── Result ────────────────────────────────────────────────────
        // The bottom-most base copy is still on the stack.
        Ty::NamedPtr(product_name.to_string())
    }

    /// Load a single field from a heap-allocated product struct.
    ///
    /// Stack contract: enters with `[ptr_to_struct]` on top, exits with
    /// the field value laid out per `field_repr` (one i32/i64 for
    /// scalars / named pointers, two i32s `[ptr, len]` for strings).
    /// Returns the field's wasm repr so the caller can thread it
    /// through subsequent method dispatch.
    ///
    /// Returns `None` if `field_name` is not a known field of
    /// `product_name` (the caller is responsible for the fallback).
    pub(super) fn load_product_field(
        &self,
        product_name: &str,
        field_name: &str,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Option<Ty> {
        let layout = self.product_field_layout(product_name);
        let (_, field_repr, field_offset) =
            layout.iter().find(|(n, _, _)| n == field_name).cloned()?;
        match &field_repr {
            Ty::I64 => {
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: field_offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::F64 => {
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: field_offset as u64,
                    align: 3,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: field_offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::Str | Ty::NamedStr(_) => {
                // Stack: [base]. Stash base, then re-load it twice to
                // emit the (ptr, len) pair as two i32 loads.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: field_offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: (field_offset + 4) as u64,
                    align: 2,
                    memory_index: 0,
                }));
                Some(field_repr)
            }
            Ty::List | Ty::Unit => {
                f.instruction(&Instruction::Drop);
                None
            }
        }
    }

    /// Build a list value from positional element expressions.
    ///
    /// Each slot is fixed at 8 bytes regardless of element type. The
    /// layout per slot is:
    ///
    ///   * `Ty::I64`        → one i64 at offset 0.
    ///   * `Ty::I32`        → one i32 at offset 0, upper 4 bytes unused.
    ///   * `Ty::Str`/`NamedStr` → i32 ptr at offset 0, i32 len at offset 4.
    ///   * anything else    → dropped + zeroed (legacy fallback).
    ///
    /// The fixed 8-byte stride lets the same `(ptr, len)` representation
    /// describe lists of any of the above types; downstream methods
    /// dispatch on a `Ty::List` receiver and read back according to the
    /// expected element shape (see `compile_builtin_method`).
    pub(super) fn build_list_literal(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // `List(a * b * c)` — the elements arrive as one product now that
        // comma argument lists are gone; flatten it to the element list.
        // A single non-product element (`List("x")`) stays one element.
        let flat: Vec<Expr>;
        let args: &[Expr] = match args {
            [Expr::ProductValue { fields, .. }] => {
                flat = fields.clone();
                &flat
            }
            _ => args,
        };
        let n = args.len() as u32;
        let byte_size = n * 8;
        f.instruction(&Instruction::I32Const(byte_size as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        for (i, arg) in args.iter().enumerate() {
            let ty = self.compile_expr(arg, scope, f);
            let slot_offset = (i as u64) * 8;
            match ty {
                Ty::I64 => {
                    // Stack: [value]. Store at slot.
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::F64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                    f.instruction(&Instruction::F64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::I32 => {
                    // Promote i32 to i64 so all numeric lists share the
                    // same wire format. Upper 4 bytes carry the
                    // sign-extension; callers reading back as i32 simply
                    // load the low 4 bytes.
                    f.instruction(&Instruction::I64ExtendI32S);
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                Ty::Str | Ty::NamedStr(_) => {
                    // Stack: [ptr, len]. Stash len, then ptr, then store
                    // them at offset+0 and offset+4 of the slot.
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                                                                            // Store ptr at offset+0
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset,
                        align: 2,
                        memory_index: 0,
                    }));
                    // Store len at offset+4
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset + 4,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                other => {
                    self.drop_value(other, f);
                    // Zero the slot so a later read doesn't see
                    // uninitialised heap bytes.
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I64Const(0));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_offset,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
        }

        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(n as i32));
        Ty::List
    }

    // ── Method call dispatch ────────────────────────────────────────────────────

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
        // Builtins (`Sum`, `Ge`, `Joined`, …) and pure operations are
        // not type names, so they fall through to the method paths
        // below. Runs before the receiver is compiled, so
        // `compile_constructor` owns every input — no double emit.
        // A name in the builtin vocabulary (`Length`, `Sum`, `Mapped`,
        // `Eq`, …) is never construction even when it also names a type
        // (`Length = Int`, `Mapped<U> = List<U>`): the method paths below
        // own it as a builtin on the receiver (list length / map) or a
        // stdlib shape. Excluding it keeps `list -> Length` a length, not
        // a `Length(list)` newtype wrap.
        let is_builtin_op = crate::ast::builtin_method_alias(method).is_some();
        // A name with a func-table body is a shape / constructor family
        // (`Route`, `Served`, `TestResult`, `Greeting`'s Int member, …).
        // Those resolve on the method path below, keyed on the receiver's
        // *compiled* type — routing them through `compile_constructor`
        // would rebuild the receiver and lose handle/repr threading. Only
        // *pure* construction (a product / newtype / variant / primitive
        // with no func body) needs the construction route.
        let has_func_body = self.func_table.keys().any(|(_, m)| m == method);
        let is_ctor_name = (!is_builtin_op
            && !has_func_body
            && method.chars().next().is_some_and(char::is_uppercase)
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

        let recv_ty = self.compile_expr(receiver, scope, f);

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
        // its newtype alias chain — `Foo("x").ToJson()` with `Foo =
        // String` must find a `ToJson` declared on `String`.
        // A method resolves to a user/stdlib function under its written
        // name or — for the types-only vocabulary — under its camelCase
        // alias. `stream -> Mapped(f)` finds the `map` binding on
        // `Stream` (a camelCase FFI function) before the `List` builtin
        // `Mapped` claims it; `list -> Mapped(f)` misses both bindings
        // and falls through to the builtin below.
        let method_names: Vec<String> = match crate::ast::builtin_method_alias(method) {
            Some(canonical) => vec![method.to_string(), canonical.to_string()],
            None => vec![method.to_string()],
        };
        // A scalar newtype erases to its underlying primitive, so a piped
        // construction like `3000 -> Port` leaves `Ty::I64` on the stack
        // and `type_name` recovers only "Int" — losing "Port", which the
        // next step (`Port -> HttpServer`) dispatches on. Recover the
        // *static* type from the receiver's syntactic shape: `Foo(x)` or
        // `x -> Foo` constructs a `Foo` when `Foo` names a type. Tried
        // first so newtype-typed shapes still resolve.
        let static_recv_type: Option<String> = match receiver {
            Expr::Constructor { name, .. } if self.type_defs.contains_key(&name.name) => {
                Some(name.name.clone())
            }
            Expr::MethodCall {
                method: m,
                piped: true,
                ..
            } if self.type_defs.contains_key(&m.name) => Some(m.name.clone()),
            _ => None,
        };
        let mut candidate_types: Vec<String> = Vec::new();
        if let Some(st) = &static_recv_type {
            candidate_types.extend(self.collect_alias_chain(st));
        }
        if let Some(name) = &type_name {
            for a in self.collect_alias_chain(name) {
                if !candidate_types.contains(&a) {
                    candidate_types.push(a);
                }
            }
        }
        for alias in candidate_types {
            for m in &method_names {
                let key = (Some(alias.clone()), m.clone());
                if let Some(info) = self.func_table.get(&key).cloned() {
                    return self.emit_func_table_call(&info, args, scope, f);
                }
            }
        }
        if type_name.is_none() {
            for m in &method_names {
                if let Some(info) = self.func_table.get(&(None, m.clone())).cloned() {
                    return self.emit_func_table_call(&info, args, scope, f);
                }
            }
        }

        // Also try without type name (free functions used as methods)
        let free_key = (None, method.to_string());
        if type_name.is_some() {
            if let Some(info) = self.func_table.get(&free_key).cloned() {
                return self.emit_func_table_call(&info, args, scope, f);
            }
        }

        // No user/stdlib function matched — normalize the types-only
        // vocabulary (`Print`/`Sum`/`Joined`/…) to its canonical builtin
        // name so the `print`/`String`/builtin paths below recognize it.
        let method = crate::ast::builtin_method_alias(method).unwrap_or(method);

        // Conversion is construction (the language spec, docs/src/spec/):
        // `Int.String()` / `Byte.String()` are the method spellings of
        // the `String(Int)` / `String(Byte)` constructors. Placed after
        // the func-table lookups so a user-declared `String` method on
        // some other receiver type still wins.
        if method == "String" && args.is_empty() {
            match &recv_ty {
                Ty::I64 => {
                    return if self.expr_is_byte(receiver) {
                        self.emit_byte_to_str(scope, f)
                    } else {
                        f.instruction(&Instruction::Call(self.fn_int_to_str));
                        Ty::Str
                    };
                }
                // A String-alias receiver (`Path("/x").String()`) is
                // the identity conversion — the value already is one.
                ty if ty.is_str_like() => return Ty::Str,
                _ => {}
            }
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

        // `.print()` is a universal method that delegates to the type-aware
        // `emit_print` helper. It accepts 0 args (`x.print()`) — a single
        // arg form for the legacy `Stdout` convention can be added later.
        if method == "print" && args.is_empty() {
            self.emit_print(recv_ty, scope, f);
            return Ty::Unit;
        }

        // Built-in methods
        self.compile_builtin_method(recv_ty, method, args, scope, f)
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
        if info.is_async {
            return self.emit_async_call(info, scope, f);
        }
        let Some(shape) = info.indirect_return.clone() else {
            f.instruction(&Instruction::Call(info.func_idx));
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
            IndirectReturnShape::ScalarRecord {
                product, fields, ..
            } => {
                // Copy each canonical field into a fresh Canon product
                // struct, widening narrow ints to i64. The ret area is
                // still in the `alloc_ptr` local ($alloc the function
                // doesn't touch codegen locals).
                use wasm_encoder::PrimitiveValType as P;
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
                    let off = field.offset as u64;
                    match field.prim {
                        P::U64 | P::S64 => {
                            f.instruction(&Instruction::I64Load(MemArg {
                                offset: off,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        P::F64 => {
                            f.instruction(&Instruction::F64Load(MemArg {
                                offset: off,
                                align: 3,
                                memory_index: 0,
                            }));
                        }
                        P::U32 | P::S32 | P::U16 | P::S16 | P::U8 | P::S8 | P::Bool | P::Char => {
                            match field.prim {
                                P::U16 | P::S16 => {
                                    f.instruction(&Instruction::I32Load16U(MemArg {
                                        offset: off,
                                        align: 1,
                                        memory_index: 0,
                                    }));
                                }
                                P::U8 | P::S8 | P::Bool => {
                                    f.instruction(&Instruction::I32Load8U(MemArg {
                                        offset: off,
                                        align: 0,
                                        memory_index: 0,
                                    }));
                                }
                                _ => {
                                    f.instruction(&Instruction::I32Load(MemArg {
                                        offset: off,
                                        align: 2,
                                        memory_index: 0,
                                    }));
                                }
                            }
                            if matches!(field.prim, P::S8 | P::S16 | P::S32) {
                                f.instruction(&Instruction::I64ExtendI32S);
                            } else {
                                f.instruction(&Instruction::I64ExtendI32U);
                            }
                        }
                        _ => {
                            f.instruction(&Instruction::I64Const(0));
                        }
                    }
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
                Ty::NamedPtrStr("Result".to_string(), ok_name, err_name)
            }
        }
    }

    /// Emits the guest-side sequence for calling an `extern Wasm.async`
    /// function under the component-model async-lower ABI.
    ///
    /// At entry: args are already on the stack in their flat representation
    /// (just like a sync call), having been compiled by
    /// `emit_func_table_call` before the dispatch on `is_async`.
    ///
    /// Sequence:
    ///
    /// 1. **Ret-area** (only when the WIT-level function has a result).
    ///    Allocate `ret_area_size_for(&info.result_ty)` bytes via `$alloc`,
    ///    stash the pointer in `alloc_ptr`, and push it as the trailing
    ///    core-arg.
    /// 2. **Call** the async-lowered import. Its core signature is
    ///    `(flat_params …, ret_ptr?) -> i32` where the i32 result is a
    ///    *packed status word*:
    ///    - low 4 bits = `CallState` (0 Starting, 1 Started,
    ///      2 Returned, 3 StartCancelled, 4 ReturnCancelled)
    ///    - high 28 bits = subtask waitable handle (or 0 when Returned)
    /// 3. **Status check**. Save the status to `tmp_i32`, then mask the
    ///    low 4 bits and compare against `2 = Returned`. On the
    ///    sync-completion fast path we skip the wait block. Otherwise we
    ///    enter the **wait sequence**: extract the subtask handle from
    ///    the high 28 bits of the status, create a fresh waitable-set,
    ///    join the subtask into it, block on `waitable-set.wait`, and
    ///    drop both the set and the subtask after the wait returns. By
    ///    that point the host has written the actual result into our
    ///    ret-area.
    /// 4. **Decode result** from the ret-area according to
    ///    `info.result_ty`.
    pub(super) fn emit_async_call(
        &mut self,
        info: &FuncInfo,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            // Allocate ret-area, save its ptr, and push it as the last arg.
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(scope.alloc_ptr()));
        }
        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));
        // Save the packed status so we can (a) check the low 4 bits and
        // (b) recover the subtask handle from the high 28 bits if we
        // need to wait.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        // Check `status & 0xF != 2` (i.e. *not* `Returned`).
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(0xF));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        // ── Async-suspend path ─────────────────────────────────────────
        // The subtask has been started but not yet finished. Extract its
        // handle (high 28 bits of the packed status), wrap it in a
        // single-element waitable-set, and block on `waitable-set.wait`.
        // The host signals subtask completion through the waitable; when
        // wait returns, the result has been written to our ret-area.
        //
        // We re-use scratch locals from the surrounding function's
        // extra-locals pool:
        //   tmp_i32_b → subtask handle
        //   rbool     → waitable-set handle
        //   rptr      → event-area pointer (8 bytes, written by wait)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        // set = waitable-set.new()
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        // waitable.join(subtask, set)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        // event_area = $alloc(8) — wait writes the 8-byte event payload here.
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        // event_code = waitable-set.wait(set, event_area); we don't need
        // to inspect the event payload since the only thing in the set
        // is our subtask — wait returning means it reached a terminal
        // state. Drop the returned event code.
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);
        // Drop the subtask BEFORE the waitable-set: the subtask is
        // joined to the set as a child, so dropping the set while the
        // subtask is still registered trips wasmtime's
        // `ResourceTableError::HasChildren` check (see
        // `wasmtime::runtime::component::concurrent::waitable_set_drop`).
        // Dropping the subtask removes it from the set's child list;
        // the set then drops cleanly.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));
        f.instruction(&Instruction::End);
        // Read the result out of the ret-area (still in `alloc_ptr`).
        if !has_result {
            return Ty::Unit;
        }
        match &info.result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // String result: (ptr i32 at +0, len i32 at +4).
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
            Ty::I64 | Ty::F64 => {
                // 8-byte scalar at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                if matches!(info.result_ty, Ty::I64) {
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    f.instruction(&Instruction::F64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                info.result_ty.clone()
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) => {
                // 4-byte scalar / handle at +0.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                info.result_ty.clone()
            }
            // List / NamedPtrStr / Unit fall here. The current codegen
            // doesn't synthesise async externs returning these shapes —
            // they'd need their own ret-area decoders. Trap so the gap is
            // visible if we ever do.
            _ => {
                f.instruction(&Instruction::Unreachable);
                info.result_ty.clone()
            }
        }
    }

    // ── Concurrency combinators ─────────────────────────────────────
    //
    // `parallel(a, b)` and `race(a, b)` are guest-side combinators: the
    // codegen emits a non-blocking async call for each arg (capturing
    // subtask handle + ret-area into named locals), then runs the
    // canonical-ABI multi-subtask wait sequence in the same function.
    // No host bridge is involved — the `canon:async/waitable` canon
    // intrinsics (`set-new`, `join`, `set-wait`, `set-drop`,
    // `subtask-drop`, `subtask-cancel`) handle everything.

    /// Compile a single `parallel`/`race` argument as a non-blocking
    /// async call. The arg must be a `MethodCall` or `Constructor` that
    /// resolves to an `extern Wasm.async` function in `func_table`.
    ///
    /// On exit:
    ///   - The arg's sub-args are evaluated.
    ///   - The arg's ret-area is allocated into `retarea_local`.
    ///   - The import is called; the packed status is consumed.
    ///   - The subtask handle (status >> 4) is stored in `subtask_local`.
    ///
    /// Returns the callee's declared `result_ty` so the caller knows how
    /// to decode the ret-area later.
    ///
    /// Today this is conservative: if the arg shape doesn't match a known
    /// async extern, the codegen traps via `unreachable`. The checker
    /// can't surface a friendlier error yet because the surface is brand
    /// new; clean up once user pain reports.
    pub(super) fn emit_arg_as_nonblocking(
        &mut self,
        arg: &Expr,
        scope: &LocalScope,
        f: &mut Function,
        subtask_local: u32,
        retarea_local: u32,
    ) -> Ty {
        // Resolve the callee FuncInfo and identify the receiver / args.
        let resolved: Option<(FuncInfo, Option<Box<Expr>>, Vec<Expr>)> = match arg {
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let recv_ty_name = self.infer_static_type_name(receiver);
                let key = recv_ty_name.map(|n| (Some(n), method.name.clone()));
                let info = key
                    .and_then(|k| self.func_table.get(&k).cloned())
                    .or_else(|| self.func_table.get(&(None, method.name.clone())).cloned());
                info.map(|i| (i, Some(receiver.clone()), args.clone()))
            }
            Expr::Constructor { name, args, .. } => {
                // Try free-function key first.
                let mut info = self.func_table.get(&(None, name.name.clone())).cloned();
                // Then try Self-renamed constructor.
                if info.is_none() {
                    info = self
                        .func_table
                        .get(&(Some(name.name.clone()), "Self".to_string()))
                        .cloned();
                }
                // Then try capability-receiver: first arg's type as receiver.
                if info.is_none() {
                    if let Some(first) = args.first() {
                        if let Some(tname) = self.infer_static_type_name(first) {
                            info = self
                                .func_table
                                .get(&(Some(tname), name.name.clone()))
                                .cloned();
                        }
                    }
                }
                info.map(|i| (i, None, args.clone()))
            }
            _ => None,
        };

        let Some((info, receiver_opt, args_to_push)) = resolved else {
            // Couldn't resolve the call; trap. Callers should ensure the
            // arg points to a real async extern.
            f.instruction(&Instruction::Unreachable);
            return Ty::Unit;
        };

        if !info.is_async {
            // Only async calls make sense here — a sync call would
            // complete immediately and there'd be no subtask to wait on.
            f.instruction(&Instruction::Unreachable);
            return info.result_ty.clone();
        }

        // Push the receiver expression first (for MethodCall form). The
        // receiver becomes the first param of the import call.
        if let Some(rcv) = receiver_opt {
            let _ = self.compile_expr(&rcv, scope, f);
        }
        // Then the explicit args.
        for a in args_to_push {
            let _ = self.compile_expr(&a, scope, f);
        }

        // Allocate the ret-area and tee into `retarea_local` (leaving the
        // ptr on the stack as the last param to the import).
        let has_result = !matches!(info.result_ty, Ty::Unit);
        if has_result {
            let size = ret_area_size_for(&info.result_ty);
            f.instruction(&Instruction::I32Const(size as i32));
            f.instruction(&Instruction::Call(self.fn_alloc));
            f.instruction(&Instruction::LocalTee(retarea_local));
        } else {
            f.instruction(&Instruction::I32Const(0));
            f.instruction(&Instruction::LocalSet(retarea_local));
        }

        // Call the async-lowered import. Stack on return: i32 packed status.
        f.instruction(&Instruction::Call(info.func_idx));

        // Extract subtask handle = status >> 4. The low 4 bits encode the
        // CallState; the high 28 bits are the subtask waitable handle.
        f.instruction(&Instruction::I32Const(4));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::LocalSet(subtask_local));

        info.result_ty.clone()
    }

    /// Emit `parallel(a, b)`: start both async calls non-blocking, join
    /// their subtasks to a fresh waitable-set, loop until both events
    /// fire, then build a `List<T>` with the two results in arg-order.
    ///
    /// Both args must call async externs returning the same payload type.
    /// The result type is `Ty::List`. Today only `Ty::Str` / `Ty::NamedStr`
    /// element shapes are decoded; other shapes trap.
    pub(super) fn compile_parallel(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        if args.len() != 2 {
            // Surface error: parallel expects exactly two args. The
            // checker doesn't yet validate arity for synthetic combinators.
            f.instruction(&Instruction::Unreachable);
            return Ty::List;
        }

        // ── Start both calls non-blocking ─────────────────────────
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let ty_b = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        // Both arms must agree on element type.
        let _ = ty_b;
        let elem_ty = ty_a;

        // ── Build waitable-set, join both ──────────────────────────
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // ── Event area + seen flags ─────────────────────────────
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));

        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));

        // ── Wait loop until both seen ───────────────────────────
        //
        // Structure:
        //   block $break
        //     loop $continue
        //       wait; drop event_code
        //       handle = load i32 at par_event_ptr+0
        //       handle == subtask_a ? seen_a = 1
        //       handle == subtask_b ? seen_b = 1
        //       (seen_a & seen_b) ? br $break (depth=1)
        //       br $continue (depth=0)
        //     end
        //   end
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));

        // waitable-set.wait(set, event_area) → event_code; drop event_code
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);

        // event_handle = load i32 at par_event_ptr+0 → tmp_i32
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));

        // if event_handle == subtask_a: seen_a = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
        f.instruction(&Instruction::End);

        // if event_handle == subtask_b: seen_b = 1
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
        f.instruction(&Instruction::End);

        // if (seen_a & seen_b): br $break (depth 1 — the block above the loop)
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::LocalGet(scope.par_seen_b()));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::BrIf(1));

        // br $continue (depth 0 — the loop itself)
        f.instruction(&Instruction::Br(0));

        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block

        // ── Cleanup: drop subtasks before the set ────────────────────
        // Subtasks are children of the set; the set's drop requires no
        // children (see wasmtime's `ResourceTableError::HasChildren`).
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // ── Build List<T> with the two results ──────────────────────
        // List layout per `build_list_literal`: N*8 bytes, each slot is
        // (ptr i32, len i32) for Str / (8 bytes for I64/F64) at offsets
        // i*8. Total size = 16 for 2 elements.
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        match &elem_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // slot 0 ← (ptr,len) at par_retarea_a +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 0, scope.par_retarea_a(), 0);
                // slot 1 ← (ptr,len) at par_retarea_b +0/+4
                self.copy_str_pair(f, scope.alloc_ptr(), 8, scope.par_retarea_b(), 0);
            }
            Ty::I64 | Ty::F64 => {
                // Each slot is one i64. Source ret-area holds the value at +0.
                for (slot_off, retarea) in
                    [(0u64, scope.par_retarea_a()), (8u64, scope.par_retarea_b())]
                {
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(retarea));
                    f.instruction(&Instruction::I64Load(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::I64Store(MemArg {
                        offset: slot_off,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
            _ => {
                // Other element shapes not yet supported. Trap so the gap
                // is visible (we'd silently corrupt the list otherwise).
                f.instruction(&Instruction::Unreachable);
            }
        }

        // Push (list_ptr, len=2) — the standard `Ty::List` representation.
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(2));
        Ty::List
    }

    /// Emit `race(a, b)`: start both async calls non-blocking, wait for
    /// the *first* event, cancel the loser, drop everything, and return
    /// the winner's result decoded from its ret-area.
    ///
    /// Today only `Ty::Str` / `Ty::NamedStr` element shapes are decoded;
    /// other shapes trap.
    pub(super) fn compile_race(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        if args.len() != 2 {
            f.instruction(&Instruction::Unreachable);
            return Ty::Str;
        }

        // Start both calls non-blocking.
        let ty_a = self.emit_arg_as_nonblocking(
            &args[0],
            scope,
            f,
            scope.par_subtask_a(),
            scope.par_retarea_a(),
        );
        let _ = self.emit_arg_as_nonblocking(
            &args[1],
            scope,
            f,
            scope.par_subtask_b(),
            scope.par_retarea_b(),
        );
        let elem_ty = ty_a;

        // Build waitable-set, join both.
        f.instruction(&Instruction::Call(self.fn_waitable_set_new));
        f.instruction(&Instruction::LocalSet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_join));

        // Event area + flags. Re-using par_seen_a as "winner is a?".
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // One wait, then identify the winner.
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_wait));
        f.instruction(&Instruction::Drop);

        // Read event handle into tmp_i32, set seen_a = (handle == subtask_a).
        f.instruction(&Instruction::LocalGet(scope.par_event_ptr()));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::I32Eq);
        f.instruction(&Instruction::LocalSet(scope.par_seen_a()));

        // Cancel the loser. `subtask.cancel` takes a subtask handle and
        // returns a state code (which we drop). The runtime guarantees
        // teardown of any transitive subtasks.
        //
        // The cancel call returns an i32 status code, even when issued
        // with async semantics. We drop it; the caller only cares that
        // the loser is no longer producing observable side effects.
        f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
        f.instruction(&Instruction::If(BlockType::Empty));
        // a won → cancel b
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Else);
        // b won → cancel a
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_cancel));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);

        // Drop both subtasks before the set.
        f.instruction(&Instruction::LocalGet(scope.par_subtask_a()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_subtask_b()));
        f.instruction(&Instruction::Call(self.fn_subtask_drop));
        f.instruction(&Instruction::LocalGet(scope.par_set()));
        f.instruction(&Instruction::Call(self.fn_waitable_set_drop));

        // Decode the winner's ret-area onto the stack.
        match &elem_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // if seen_a: push (par_retarea_a +0, +4) else (par_retarea_b +0, +4)
                // WASM `if` with result type doesn't natively allow pushing two
                // values — use a Select-style approach via a winner_retarea local.
                // Compute winner_retarea via Select.
                f.instruction(&Instruction::LocalGet(scope.par_retarea_a()));
                f.instruction(&Instruction::LocalGet(scope.par_retarea_b()));
                f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));

                // Push ptr, then len.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                elem_ty
            }
            _ => {
                f.instruction(&Instruction::Unreachable);
                elem_ty
            }
        }
    }

    /// Copy a `(ptr i32, len i32)` pair from `src_local + src_off` to
    /// `dst_local + dst_off`. Small helper used by the list-building tail
    /// of `compile_parallel`.
    pub(super) fn copy_str_pair(
        &self,
        f: &mut Function,
        dst_local: u32,
        dst_off: u64,
        src_local: u32,
        src_off: u64,
    ) {
        // ptr
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off,
            align: 2,
            memory_index: 0,
        }));
        // len
        f.instruction(&Instruction::LocalGet(dst_local));
        f.instruction(&Instruction::LocalGet(src_local));
        f.instruction(&Instruction::I32Load(MemArg {
            offset: src_off + 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Store(MemArg {
            offset: dst_off + 4,
            align: 2,
            memory_index: 0,
        }));
    }

    /// Compile `list.map(lambda)` as an inlined element-wise loop.
    ///
    /// Entry stack: `[src_ptr, len]` (the `Ty::List` pair). Exit stack:
    /// `[dst_ptr, len]` of a freshly allocated result list. `elem_name`
    /// is the lambda parameter's type name (Canon lambda bodies refer
    /// to the parameter by its type name), `elem_repr` its resolved
    /// representation — only `Ty::I64` and string-shaped elements are
    /// supported by the caller's gate.
    ///
    /// Loop state (`src`, `dst`, `remaining`) is carried on the wasm
    /// operand stack through multi-value block/loop params, NOT in
    /// locals — the lambda body is arbitrary user code and may clobber
    /// every scratch local. The only locals live across the body are
    /// the element binding itself (`map_elem_i64` / `map_elem_ptr`),
    /// which is exactly what the body is supposed to read.
    pub(super) fn compile_list_map(
        &mut self,
        elem_name: &str,
        elem_repr: &Ty,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self
            .user_type_map
            .get(&(
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32, ValType::I32, ValType::I32],
            ))
            .copied()
            // invariant: `compile()` reserves this (i32,i32,i32)->(i32,i32,i32)
            // loop type in `user_type_map` before any list-map is compiled.
            .expect("list-map loop type reserved in compile()");
        let mem64 = MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        };
        let mem32 = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        let mem32_4 = MemArg {
            offset: 4,
            align: 2,
            memory_index: 0,
        };

        // ── Setup. Stack: [src, len] ─────────────────────────────────
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
                                                                     // dst_base = alloc(len*8 + 8) — the +8 keeps a zero-length list
                                                                     // from handing $alloc a zero size.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst_base
                                                                  // Bottom-of-stack survivors: result (len, dst_base) …
        f.instruction(&Instruction::LocalGet(scope.tmp_i32())); // n
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b())); // dst_base
                                                                  // … and the loop-carried trio.
        f.instruction(&Instruction::LocalGet(scope.addr_scratch())); // src
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b())); // dst
        f.instruction(&Instruction::LocalGet(scope.tmp_i32())); // rem

        f.instruction(&Instruction::Block(BlockType::FunctionType(trio)));
        f.instruction(&Instruction::Loop(BlockType::FunctionType(trio)));
        // [src, dst, rem] — exit when rem == 0.
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // Peel the trio (no user code between here and the re-push, so
        // scratch locals are safe).
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // rem
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
                                                                     // Bind the current element.
        match elem_repr {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::LocalSet(scope.map_elem_i64()));
            }
            _ => {
                // String-shaped: (ptr, len) at +0/+4.
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(mem32_4));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
            }
        }
        // Park the next iteration's state (and the current dst for the
        // post-body store) on the operand stack where the body can't
        // touch it: [new_src, dst_cur, new_dst, new_rem].
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);

        // ── The lambda body ──────────────────────────────────────────
        let elem_local = match elem_repr {
            Ty::I64 => scope.map_elem_i64(),
            _ => scope.map_elem_ptr(),
        };
        let mut inner = LocalScope {
            vars: scope.vars.clone(),
            param_count: scope.param_count,
        };
        for alias in self.collect_alias_chain(elem_name) {
            inner.vars.insert(alias, (elem_local, elem_repr.clone()));
        }
        let out_ty = self.compile_block_return(body, &inner, f);

        // ── Store the result, restore the trio ───────────────────────
        // Stash the body's result (the element locals are free again;
        // trio juggling below uses the i32 scratch, so i32-shaped
        // results go through the element pair instead).
        match &out_ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
            }
            Ty::Unit => {}
        }
        // [new_src, dst_cur, new_dst, new_rem] → locals.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // new_rem
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // new_dst
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // dst_cur
                                                                     // Store the stashed result at dst_cur.
        match &out_ty {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I64Store(mem64));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::F64Store(mem64));
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::I32Store(mem32));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr() + 1));
                f.instruction(&Instruction::I32Store(mem32_4));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::I32Store(mem32));
            }
            Ty::Unit => {}
        }
        // Rebuild the trio and continue: [new_src] + new_dst + new_rem.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // loop
        f.instruction(&Instruction::End); // block

        // [n, dst_base, src_f, dst_f, rem_f] → [dst_base, n].
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // dst_base
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // n
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        Ty::List
    }

    pub(super) fn compile_builtin_method(
        &mut self,
        recv_ty: Ty,
        method: &str,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // Types-only vocabulary: `-> Print` / `-> Sum(2)` / `-> Joined(s)`
        // resolve to the same codegen as `print` / `add` / `concat`. Only
        // reached after the func_table lookup missed, so a user/stdlib
        // function of the same name always wins first.
        let method = crate::ast::builtin_method_alias(method).unwrap_or(method);
        match (method, &recv_ty) {
            // ── Int arithmetic ────────────────────────────────────────────────
            ("add", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Add);
                Ty::I64
            }
            ("sub", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Sub);
                Ty::I64
            }
            ("mul", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Mul);
                Ty::I64
            }
            ("div", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64DivS);
                Ty::I64
            }
            ("mod", Ty::I64) | ("rem", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64RemS);
                Ty::I64
            }
            ("lt", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64LtS);
                Ty::I32
            }
            ("le", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64LeS);
                Ty::I32
            }
            ("gt", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64GtS);
                Ty::I32
            }
            ("ge", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64GeS);
                Ty::I32
            }
            ("eq", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Eq);
                Ty::I32
            }
            ("ne", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Ne);
                Ty::I32
            }
            // ── Bool composition ─────────────────────────────────────────────
            // Bools are i32 0/1. `and`/`or` are non-short-circuiting
            // (both sides evaluate) — acceptable because Canon
            // expressions are effect-free apart from capabilities, and
            // it matches the eager `.eq(..)` chains they compose with.
            // ── Float arithmetic ──────────────────────────────────────────────
            ("add", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Add);
                Ty::F64
            }
            ("sub", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Sub);
                Ty::F64
            }
            ("mul", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Mul);
                Ty::F64
            }
            ("div", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Div);
                Ty::F64
            }
            // wasm has no f64 remainder instruction; compute
            // `a - trunc(a/b) * b` (sign follows the dividend, matching
            // Rust's `%` on floats). Both operands are needed twice and
            // wasm has no stack dup, so they round-trip through the
            // f64 scratch pair.
            ("mod" | "rem", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::LocalSet(scope.tmp_f64_b())); // b
                f.instruction(&Instruction::LocalSet(scope.tmp_f64())); // a
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_f64_b()));
                f.instruction(&Instruction::F64Div);
                f.instruction(&Instruction::F64Trunc);
                f.instruction(&Instruction::LocalGet(scope.tmp_f64_b()));
                f.instruction(&Instruction::F64Mul);
                f.instruction(&Instruction::F64Sub);
                Ty::F64
            }
            ("lt", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Lt);
                Ty::I32
            }
            ("le", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Le);
                Ty::I32
            }
            ("gt", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Gt);
                Ty::I32
            }
            ("ge", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Ge);
                Ty::I32
            }
            ("eq", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Eq);
                Ty::I32
            }
            ("ne", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Ne);
                Ty::I32
            }
            // ── String concat ────────────────────────────────────────────────────
            //
            // Allocates a fresh buffer of size `len1 + len2`, copies the
            // receiver bytes followed by the argument bytes, and returns
            // a new `(ptr, len)` pair. Uses `memory.copy` (bulk-memory
            // proposal) which wasm-encoder + wasmtime both accept.
            ("concat", _) if recv_ty.is_str_like() => {
                // Receiver is on the stack as (ptr1, len1). Compile the
                // argument so we end with (ptr1, len1, ptr2, len2).
                let mut arg_pushed = false;
                if let Some(a) = args.first() {
                    let arg_ty = self.compile_expr(a, scope, f);
                    if arg_ty.is_str_like() {
                        arg_pushed = true;
                    } else {
                        self.drop_value(arg_ty, f);
                    }
                }
                if !arg_pushed {
                    // No string arg — treat as concat with empty string.
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }

                // Stash inputs into locals (top of stack first):
                //   str_scratch_ptr+1 = len2
                //   str_scratch_ptr   = ptr2
                //   tmp_i32_b         = len1 (kept immutable; used both as
                //                              n for copy 1 and as offset
                //                              into result for copy 2)
                //   rbool             = ptr1 (used as src in copy 1; the
                //                              copy loop modifies it)
                //
                // NOTE: deliberately uses `str_scratch_ptr` (not
                // `arm_payload_ptr`) so a `concat` call inside a
                // dispatch arm body doesn't corrupt the arm's bound
                // payload — see the gap fix in CLAUDE.md.
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));

                // total_len = len1 + len2, kept in tmp_i32 for the return.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr() + 1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));

                // result_ptr = alloc(total_len), stash in alloc_ptr.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

                // Copy 1: dst = result_ptr, src = ptr1, n = len1.
                // Loop locals: dst → rptr, src → rbool (in-place), n → rlen.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);

                // Copy 2: dst = result_ptr + len1, src = ptr2, n = len2.
                // Reuse rptr/rbool/rlen as before.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);

                // Push (result_ptr, total_len) as the concat's return value.
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::Str
            }
            // ── String length ───────────────────────────────────────
            //
            // Stack: [ptr, len] → [len_i64]. Drops the pointer; the
            // length is the i32 byte-count promoted to i64 (Canon `Int`).
            ("length", _) if recv_ty.is_str_like() => {
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::Drop); // drop ptr
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I64ExtendI32S);
                Ty::I64
            }
            // ── String byteAt ──────────────────────────────────────
            //
            // `s.byteAt(i)` returns the unsigned byte at index `i`
            // (0..=255) as an `Int`. Out-of-bounds access traps via the
            // raw `i32.load8_u` (wasmtime translates an OOB load into a
            // memory-out-of-bounds trap, which surfaces as a Rust panic
            // through wasmtime's runtime). For a string-as-bytes view of
            // a String — this is the primitive that makes Canon-side
            // string parsing possible.
            ("byteAt", _) if recv_ty.is_str_like() => {
                // Receiver on stack: [ptr, len]. Compile index arg next.
                let mut arg_pushed = false;
                if let Some(a) = args.first() {
                    let arg_ty = self.compile_expr(a, scope, f);
                    if matches!(arg_ty, Ty::I64) {
                        arg_pushed = true;
                    } else {
                        self.drop_value(arg_ty, f);
                    }
                }
                if !arg_pushed {
                    f.instruction(&Instruction::I64Const(1));
                }
                // Canon indexing is 1-based (like positional product
                // access `byte.1`): byteAt(1) is the first byte.
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                // Stack: [ptr, len, index_i64]. Want: load byte at ptr+index.
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // index_i32
                f.instruction(&Instruction::Drop); // drop len
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add); // ptr + index
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I64ExtendI32U);
                Ty::I64
            }
            // ── String substring ────────────────────────────────────
            //
            // `s.substring(start, end)` returns the 1-based, inclusive
            // slice `[start, end]` as a fresh String — `substring(1, 4)`
            // is the first four bytes, pairing with 1-based `byteAt`.
            // Internally start is shifted down once and the old
            // half-open arithmetic does the rest (`len = end - (start-1)`).
            // Allocates a new buffer and copies the bytes — the result
            // is independent of the receiver's lifetime (heap is
            // bump-allocated, so neither outlives the other; copying
            // makes mutation safe if it ever lands).
            ("substring", _) if recv_ty.is_str_like() && substring_bounds(args).is_some() => {
                // The bounds arrive either as a `From * To` product (the
                // canonical, positionless form — alphabetical order puts
                // `From` first) or, during migration, as two positional
                // args. Either way: `start`, then `end` (both `Int`).
                let (start_e, end_e) = substring_bounds(args).unwrap();
                let ty0 = self.compile_expr(start_e, scope, f);
                if !matches!(ty0, Ty::I64) {
                    self.drop_value(ty0, f);
                    f.instruction(&Instruction::I64Const(1));
                }
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                let ty1 = self.compile_expr(end_e, scope, f);
                if !matches!(ty1, Ty::I64) {
                    self.drop_value(ty1, f);
                    f.instruction(&Instruction::I64Const(0));
                }
                // Stack: [ptr, len, start_i64, end_i64].
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // end_i32
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // start_i32
                f.instruction(&Instruction::Drop); // drop len
                                                   // src = ptr + start
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rbool())); // src
                                                                      // new_len = end - start (preserved in str_scratch_ptr for
                                                                      // the final return push; the copy loop will clobber rlen).
                                                                      // Uses `str_scratch_ptr` (not `arm_payload_ptr`) so a
                                                                      // `substring` call inside a dispatch arm body doesn't
                                                                      // corrupt the bound payload.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::LocalSet(scope.str_scratch_ptr()));
                // result_ptr = alloc(new_len)
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
                // Copy loop locals: dst → rptr, src → rbool (already set),
                // n → rlen (decremented to 0 by the loop).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                f.instruction(&Instruction::LocalSet(scope.rlen()));
                self.emit_byte_copy_loop(scope, f);
                // Return (result_ptr, new_len).
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.str_scratch_ptr()));
                Ty::Str
            }
            // ── String eq ───────────────────────────────────────────────────────────────
            //
            // `s1.eq(s2)` returns `True` if both strings have the same
            // length and byte-for-byte content. Length-mismatch is the
            // fast-fail path; equal-length walks a byte-by-byte compare
            // loop. Pairs with `byteAt` to unblock parser-style code.
            ("eq", _) if recv_ty.is_str_like() && args.len() == 1 => {
                // Compile the other string. Stack ends as [ptr1, len1, ptr2, len2].
                let arg_ty = self.compile_expr(&args[0], scope, f);
                if !arg_ty.is_str_like() {
                    // Mismatched arg type — drop everything and return false.
                    self.drop_value(arg_ty, f);
                    self.drop_value(recv_ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    return Ty::I32;
                }
                self.emit_str_eq(scope, f);
                Ty::I32
            }
            // ── String ordering ─────────────────────────────────────
            //
            // Byte-wise lexicographic comparison via `fn_str_cmp`
            // (-1/0/1), mirroring `Int`'s comparison surface. This is
            // the primitive behind user-side alphabetical ordering —
            // the same order the language enforces on declarations.
            ("lt" | "le" | "gt" | "ge" | "ne", _) if recv_ty.is_str_like() && args.len() == 1 => {
                let arg_ty = self.compile_expr(&args[0], scope, f);
                if !arg_ty.is_str_like() {
                    // Mismatched arg type — drop everything, return false.
                    self.drop_value(arg_ty, f);
                    self.drop_value(recv_ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    return Ty::I32;
                }
                f.instruction(&Instruction::Call(self.fn_str_cmp));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&match method {
                    "lt" => Instruction::I32LtS,
                    "le" => Instruction::I32LeS,
                    "gt" => Instruction::I32GtS,
                    "ge" => Instruction::I32GeS,
                    _ => Instruction::I32Ne,
                });
                Ty::I32
            }
            // ── List methods ───────────────────────────────────────────────────
            ("length", Ty::List) | ("length", Ty::NamedPtr(_)) => {
                // Stack: (ptr: i32, len: i32) for List, or just i32 for NamedPtr
                match &recv_ty {
                    Ty::List => {
                        // Stack: [ptr, len]. Drop ptr, extend len to i64.
                        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                        f.instruction(&Instruction::Drop); // drop ptr
                        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                        f.instruction(&Instruction::I64ExtendI32S);
                    }
                    _ => {
                        // Not a list — drop and return 0
                        self.drop_value(recv_ty, f);
                        f.instruction(&Instruction::I64Const(0));
                    }
                }
                Ty::I64
            }
            ("map", Ty::List) => {
                // Real element-wise map when the argument is an inline
                // lambda with a supported element type. Canon lambdas
                // are non-capturing (the language has no local
                // variables), so the body is inlined straight into the
                // loop with the parameter's type name bound to the
                // current-element local. Anything else falls back to
                // the historical identity behaviour.
                if let Some(Expr::Lambda { params, body, .. }) = args.first() {
                    if params.len() == 1 {
                        if let TypeExpr::Named { name, .. } = &params[0].ty {
                            let name = name.clone();
                            let body = body.clone();
                            let elem = self.resolve_repr(&name);
                            if matches!(elem, Ty::I64 | Ty::Str | Ty::NamedStr(_)) {
                                return self.compile_list_map(&name, &elem, &body, scope, f);
                            }
                        }
                    }
                }
                // Identity fallback (unsupported element shapes).
                // Stack: [ptr, len]
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // save ptr
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::List
            }
            ("get", Ty::List) => {
                // list.get(i) -> Option — mirrors `first` but reads at
                // `list_ptr + i*8` after an unsigned bounds check
                // (negative indices wrap to huge u64s and fail it).
                //
                // Compile the index argument first — it is arbitrary
                // user code and may clobber every scratch local; the
                // receiver's (ptr, len) stays safe on the stack below
                // it.
                let idx_ty = self.compile_expr(&args[0], scope, f);
                if !matches!(idx_ty, Ty::I64) {
                    self.drop_value(idx_ty, f);
                    f.instruction(&Instruction::I64Const(1));
                }
                // 1-based: get(1) is the first element. `get(0)` shifts
                // to -1, wraps to a huge u64, and fails the unsigned
                // bounds check below — a clean `None`.
                f.instruction(&Instruction::I64Const(1));
                f.instruction(&Instruction::I64Sub);
                // Stack: [ptr, len, idx]. All user code is done; peel.
                f.instruction(&Instruction::LocalSet(scope.tmp_i64())); // idx
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // ptr
                                                                          // Allocate the Option struct.
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                // idx < len (unsigned, in i64 space)?
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I64ExtendI32U);
                f.instruction(&Instruction::I64LtU);
                f.instruction(&Instruction::If(BlockType::Empty));
                // Some: tag=1, payload = i64 at ptr + idx*8.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::Else);
                // None: tag=0.
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            // NOTE: `Map` / `Set` methods are NOT built in — they are
            // pure Canon (`canon/std/Map`, `canon/std/Set`) and resolve
            // through `func_table` in `compile_method_call` before the
            // builtin fallback ever fires.
            // ── List growth ──────────────────────────────────────────
            ("append", Ty::List) => {
                // Compile the element, then pack it into the 8-byte
                // slot the same way `build_list_literal` stores it:
                // i64 verbatim, strings as `ptr | len << 32`.
                let elem_ty = self.compile_expr(&args[0], scope, f);
                match elem_ty {
                    Ty::I64 => {}
                    ref t if t.is_str_like() => {
                        f.instruction(&Instruction::I64ExtendI32U); // len
                        f.instruction(&Instruction::I64Const(32));
                        f.instruction(&Instruction::I64Shl);
                        f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64ExtendI32U); // ptr
                        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                        f.instruction(&Instruction::I64Or);
                    }
                    Ty::F64 => {
                        f.instruction(&Instruction::I64ReinterpretF64);
                    }
                    Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                        f.instruction(&Instruction::I64ExtendI32U);
                    }
                    other => {
                        self.drop_value(other, f);
                        f.instruction(&Instruction::I64Const(0));
                    }
                }
                f.instruction(&Instruction::Call(self.fn_list_append));
                Ty::List
            }
            ("concat", Ty::List) => {
                let ty = self.compile_expr(&args[0], scope, f);
                if !matches!(ty, Ty::List) {
                    // Non-list arg — concat with the empty list.
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                f.instruction(&Instruction::Call(self.fn_list_concat));
                Ty::List
            }
            // `list.Json()` — conversion-is-construction spelling
            // (the language spec, docs/src/spec/) of "encode this list of
            // pre-rendered JSON values as a JSON array".
            ("Json", Ty::List) => {
                // Stack: [list_ptr, list_len]. Call the helper which
                // returns `(out_ptr, out_len)` of a freshly-allocated
                // string `[elem0,elem1,…,elemN]`. Each slot in the list
                // is read as `(i32 ptr, i32 len)` at offsets 0/4 — the
                // storage layout of `build_list_literal` for string
                // elements. Lists of `Int` / `Float` slots are
                // misinterpreted (their first 4 bytes would be read as
                // a ptr); we document that and rely on user code to
                // only call this on `List<String>`-shaped lists.
                f.instruction(&Instruction::Call(self.fn_list_to_json_array));
                Ty::Str
            }
            ("first", Ty::List) => {
                // Stack: [ptr, len] → Option<Int>
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // save len
                f.instruction(&Instruction::LocalSet(scope.alloc_ptr())); // save ptr
                                                                          // alloc 12 bytes for Option
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool())); // save option ptr
                                                                      // if len == 0 → None (tag=0, already zeroed)
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::If(BlockType::Empty));
                // Some: tag=1, payload = first i64 element at [list_ptr+0]
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::I64Store(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::Else);
                // None: tag=0 (already zeroed by alloc initialization? No, heap may be dirty.)
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Option".to_string())
            }
            // ── HTTP mode: request introspection ─────────────────────────────
            // `request.path()` — `[method]request.get-path-with-query`
            // returns `option<string>` through an indirect ret-area
            // (disc byte at +0, ptr/len at +4/+8). Re-shaped into a
            // Canon `Option` struct (i32 tag at +0, payload at +4/+8)
            // so the ordinary `(None, Some<String>)` dispatch works.
            ("path", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                // Stack: [request]. Methods take a borrow — passing our
                // own handle index is the standard convention.
                f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
                f.instruction(&Instruction::Call(FN_HTTP_GET_PATH));
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
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
                    f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
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
            // `request.method()` — `[method]request.get-method` returns
            // the WIT `method` variant through a 12-byte ret area (disc
            // byte at +0; the `other(string)` payload at +4/+8). Canon
            // surfaces it as a plain `String` ("GET", "POST", …) so
            // routing is the same literal dispatch used for paths and
            // web-app messages — no 10-arm union dispatch at every call
            // site. Static cases map to interned strings; `other`
            // passes its payload through verbatim.
            ("method", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                // Stack: [request].
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalTee(scope.rbool()));
                f.instruction(&Instruction::Call(FN_HTTP_GET_METHOD));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                // Defaults to the `other` payload (valid when disc = 9,
                // overwritten below for every static discriminant).
                for (off, local) in [(4u64, scope.map_elem_ptr()), (8, scope.addr_scratch())] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::I32Load(MemArg {
                        offset: off,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::LocalSet(local));
                }
                // WIT declaration order (wit-vendor/wasi/http.wit).
                const METHOD_NAMES: [&str; 9] = [
                    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
                ];
                for (disc, name) in METHOD_NAMES.iter().enumerate() {
                    let (ptr, len) = self.strings.intern(name);
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                    f.instruction(&Instruction::I32Const(disc as i32));
                    f.instruction(&Instruction::I32Eq);
                    f.instruction(&Instruction::If(BlockType::Empty));
                    f.instruction(&Instruction::I32Const(ptr as i32));
                    f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                    f.instruction(&Instruction::I32Const(len as i32));
                    f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                    f.instruction(&Instruction::End);
                }
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                Ty::Str
            }
            // `headers.set(name, value)` — `[method]fields.append`. The
            // stdlib binds `set` to `append`: on a freshly-constructed
            // `fields` every `set` is the first write for its name, so
            // append gives set semantics with the simpler single-value
            // WIT shape. The `result<_, header-error>` lands in a fresh
            // 20-byte ret area (disc at +0, `other(option<string>)`
            // payload from +4) and is deliberately ignored — a rejected
            // name/value degrades to "header absent", the same posture
            // as `set-status-code`.
            ("set", Ty::NamedPtr(ref n)) if n == "Headers" && self.http_mode => {
                // Stack: [hdrs]. The two args are arbitrary user code —
                // park both strings on the operand stack before touching
                // any scratch local.
                for a in args.iter().take(2) {
                    let ty = self.compile_expr(a, scope, f);
                    if !ty.is_str_like() {
                        self.drop_value(ty, f);
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                for _ in args.len()..2 {
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                // Peel [hdrs, nptr, nlen, vptr, vlen] into locals — no
                // user code runs from here on.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // vlen
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // vptr
                f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // nlen
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // nptr
                f.instruction(&Instruction::LocalSet(scope.rbool())); // hdrs
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(20));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_APPEND));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::NamedPtr("Headers".to_string())
            }
            // ── Fallback: drop receiver + args, return Unit ────────────────────
            _ => {
                self.drop_value(recv_ty, f);
                for a in args {
                    let ty = self.compile_expr(a, scope, f);
                    self.drop_value(ty, f);
                }
                Ty::Unit
            }
        }
    }

    pub(super) fn compile_i64_arg(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) {
        if let Some(a) = args.first() {
            let ty = self.compile_expr(a, scope, f);
            if ty == Ty::I32 {
                f.instruction(&Instruction::I64ExtendI32S);
            }
        } else {
            f.instruction(&Instruction::I64Const(0));
        }
    }

    pub(super) fn compile_f64_arg(&mut self, args: &[Expr], scope: &LocalScope, f: &mut Function) {
        if let Some(a) = args.first() {
            let ty = self.compile_expr(a, scope, f);
            if ty == Ty::I64 {
                f.instruction(&Instruction::F64ConvertI64S);
            }
        } else {
            f.instruction(&Instruction::F64Const(0.0.into()));
        }
    }

    // ── Match / dispatch ────────────────────────────────────────────────────────

    /// Byte-wise string equality. Expects `[ptr1, len1, ptr2, len2]`
    /// (four i32s) on the operand stack; leaves a single i32 (0/1).
    /// Length mismatch is the fast-fail path; equal lengths walk a
    /// byte-by-byte compare loop. Clobbers `rptr`, `rlen`, `rbool`,
    /// `tmp_i32`, and `tmp_i32_b`. Shared by the `String.eq` builtin
    /// and string literal-dispatch compare chains.
    pub(super) fn emit_str_eq(&self, scope: &LocalScope, f: &mut Function) {
        // Save into locals.
        f.instruction(&Instruction::LocalSet(scope.rlen())); // len2
        f.instruction(&Instruction::LocalSet(scope.rbool())); // ptr2
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len1
        f.instruction(&Instruction::LocalSet(scope.rptr())); // ptr1
                                                             // If len1 != len2, push 0 and skip. Otherwise compare bytes.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.rlen()));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Else);
        // Equal-length compare. Use tmp_i32_b as the running
        // result (1 = still-equal). Walk bytes; on mismatch,
        // set result=0 and break out.
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        // if len == 0: break
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        // if load8(p1) != load8(p2): result=0, break
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Br(2)); // break outer block
        f.instruction(&Instruction::End);
        // p1++, p2++, len--
        f.instruction(&Instruction::LocalGet(scope.rptr()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rptr()));
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.rbool()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::Br(0)); // continue
        f.instruction(&Instruction::End); // end loop
        f.instruction(&Instruction::End); // end block
                                          // Push result.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::End); // end outer if
    }

    pub(super) fn compile_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let scrut_ty = self.compile_expr(scrutinee, scope, f);

        // Determine the return type from arm annotations
        let arm_result_ty: Ty = arms
            .first()
            .map(|a| self.resolve_type_expr_repr(&a.return_ty))
            .unwrap_or(Ty::Unit);

        // Literal-pattern dispatch on a String / Int scrutinee: an
        // equality-compare chain instead of a discriminant switch.
        if arms.iter().any(|a| a.literal.is_some()) {
            return self.emit_literal_dispatch(scrut_ty, arms, &arm_result_ty, scope, f);
        }

        // Bool dispatch (i32 on stack, 0=False, 1=True)
        if scrut_ty == Ty::I32 {
            let true_arm = arms.iter().find(|a| arm_tag(a) == Some(1));
            let false_arm = arms.iter().find(|a| arm_tag(a) == Some(0));
            if true_arm.is_some() || false_arm.is_some() {
                return self.emit_bool_dispatch(true_arm, false_arm, &arm_result_ty, scope, f);
            }
        }

        // Union dispatch (i32 heap ptr on stack).
        // `NamedPtr` and `NamedPtrStr` share an in-memory layout, so both
        // dispatch the same way — the only difference is that
        // `NamedPtrStr` carries enough type info for arms to extract the
        // string payload (handled in `compile_arm_body`).
        let union_name = match &scrut_ty {
            Ty::NamedPtr(n) => Some(n.clone()),
            Ty::NamedPtrStr(n, _, _) => Some(n.clone()),
            _ => None,
        };
        if let Some(union_name) = union_name {
            // Save the union pointer so arm bodies can re-load it to extract
            // a payload, then load and push the tag for the dispatch logic.
            // Per-arm payload extraction happens inside `compile_arm_body`
            // based on each arm's pattern type — there's no single
            // "payload shape" for the whole dispatch, because variants
            // can carry different payload types (e.g. `Fail = String`
            // alongside `Pass = Unit` in `TestResult = Fail + Pass`).
            f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));
            f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
            f.instruction(&Instruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            return self.emit_union_dispatch(&union_name, arms, &arm_result_ty, scope, f);
        }

        // Fallback: drop scrutinee
        self.drop_value(scrut_ty, f);
        Ty::Unit
    }

    pub(super) fn emit_bool_dispatch(
        &mut self,
        true_arm: Option<&MatchArm>,
        false_arm: Option<&MatchArm>,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // tag is on stack (i32): 0=False, 1=True
        f.instruction(&Instruction::If(BlockType::Empty));
        // if-branch: True (tag == 1)
        if let Some(arm) = true_arm {
            self.compile_arm_body(arm, result_ty, scope, f);
        }
        f.instruction(&Instruction::Else);
        // else-branch: False (tag == 0)
        if let Some(arm) = false_arm {
            self.compile_arm_body(arm, result_ty, scope, f);
        }
        f.instruction(&Instruction::End);
        self.load_result(result_ty, scope, f)
    }

    pub(super) fn emit_union_dispatch(
        &mut self,
        union_name: &str,
        arms: &[MatchArm],
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        // tag i32 is on stack, alloc_ptr holds the union address
        // Use if/else for 2-variant unions, br_table for more
        let variants = self
            .union_variants
            .get(union_name)
            .cloned()
            .unwrap_or_default();

        if variants.len() <= 2 {
            // Simple if/else: if tag != 0 → variant[1], else → variant[0]
            let arm_1 = if variants.len() > 1 {
                arms.iter().find(|a| {
                    arm_type_name(a).is_some_and(|n| {
                        n == variants[1] || n == "Some" || n == "Ok" || n == "True"
                    })
                })
            } else {
                None
            };
            let arm_0 = arms.iter().find(|a| {
                arm_type_name(a).is_some_and(|n| {
                    n == variants.first().map(|s| s.as_str()).unwrap_or("")
                        || n == "None"
                        || n == "Err"
                        || n == "False"
                })
            });

            f.instruction(&Instruction::If(BlockType::Empty));
            if let Some(arm) = arm_1 {
                self.compile_arm_body(arm, result_ty, scope, f);
            }
            f.instruction(&Instruction::Else);
            if let Some(arm) = arm_0 {
                self.compile_arm_body(arm, result_ty, scope, f);
            }
            f.instruction(&Instruction::End);
        } else {
            // N-variant dispatch (N ≥ 3). The tag is on the stack; stash
            // it in `tmp_i32` so we can compare against each variant in
            // turn. We emit a chain of `local.get tag; i32.const i;
            // i32.eq; if ... else { ... }` nested to depth N-1, with the
            // final `else` arm handling the last variant. This is the
            // straightforward shape — a `br_table` would be more compact
            // but harder to thread through wasm-encoder's structured
            // control instructions, and the if/else version matches the
            // 2-variant code above so any future control-flow change
            // touches one place.
            f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
            let last_idx = variants.len() - 1;
            // Open `if` blocks for variants 0..last (inclusive lower bound).
            for (tag, variant) in variants.iter().enumerate().take(last_idx) {
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(tag as i32));
                f.instruction(&Instruction::I32Eq);
                f.instruction(&Instruction::If(BlockType::Empty));
                if let Some(arm) = arms.iter().find(|a| arm_matches_variant(a, variant)) {
                    self.compile_arm_body(arm, result_ty, scope, f);
                }
                f.instruction(&Instruction::Else);
            }
            // The else-most branch handles the last variant.
            if let Some(last_variant) = variants.last() {
                if let Some(arm) = arms.iter().find(|a| arm_matches_variant(a, last_variant)) {
                    self.compile_arm_body(arm, result_ty, scope, f);
                }
            }
            // Close all the `if/else` blocks opened above.
            for _ in 0..last_idx {
                f.instruction(&Instruction::End);
            }
        }
        self.load_result(result_ty, scope, f)
    }

    /// Compile a match arm body and SAVE the result to scope scratch locals.
    ///
    /// Before compiling the body, the arm's payload (if any) is extracted
    /// from the union struct (at offsets 4+ via `scope.alloc_ptr()`) and
    /// bound to a local under the arm's pattern name. So for
    ///
    /// ```text
    /// testResult.(
    ///     * (Fail) -> Unit { Fail.String.print() }
    ///     * (Pass) -> Unit { "ok".print() }
    /// )
    /// ```
    ///
    /// the `Fail` arm enters with the string payload already loaded into
    /// `scope.arm_payload_ptr()` / `+1`, and `scope.vars["Fail"]` mapped
    /// to that pair (typed `Ty::NamedStr("Fail")`). The arm body's
    /// `Fail.String.print()` then compiles like any other string
    /// expression — the newtype unwrap is a static-type retype
    /// (`newtype_unwrap_ty`), and `.print()` is the built-in.
    /// Compile a literal-pattern dispatch: the scrutinee is stashed in
    /// the dedicated `lit_scrut_*` locals, each literal arm becomes one
    /// link of an equality if/else chain (string compare via
    /// `emit_str_eq`, int compare via `i64.eq`), and the mandatory
    /// catch-all arm sits in the innermost `else`. Inside every arm
    /// body the scrutinee is bound under the catch-all's pattern name
    /// and the scrutinee's own type name. The bare primitive name
    /// (`String`) is bound only when the scrutinee *is* a bare string —
    /// a newtype-wrapped scrutinee (`Prefix(msg.substring(1, 4))`)
    /// binds `Prefix`, leaving the enclosing function's `String` param
    /// visible in arm bodies; distinguishing the two is exactly why
    /// the user wrapped it.
    pub(super) fn emit_literal_dispatch(
        &mut self,
        scrut_ty: Ty,
        arms: &[MatchArm],
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let catch_all = arms.iter().find(|a| a.literal.is_none());
        let lit_arms: Vec<&MatchArm> = arms.iter().filter(|a| a.literal.is_some()).collect();

        let mut bound_names: Vec<String> = Vec::new();
        if let Some(arm) = catch_all {
            if let Some(n) = arm_type_name(arm) {
                bound_names.push(n.to_string());
            }
        }
        if let Some(n) = scrut_ty.canon_name() {
            bound_names.push(n.to_string());
        }

        if scrut_ty.is_str_like() {
            if scrut_ty.canon_name().is_none() {
                bound_names.push("String".to_string());
            }
            // Stash the scrutinee (ptr, len) where neither the compare
            // scratch nor arm bodies' builtins will clobber it.
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_ptr() + 1));
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_ptr()));
            let mut arm_scope = scope.clone();
            for n in &bound_names {
                arm_scope
                    .vars
                    .insert(n.clone(), (scope.lit_scrut_ptr(), scrut_ty.clone()));
            }
            for arm in &lit_arms {
                match &arm.literal {
                    Some(ArmLiteral::Str(value)) => {
                        let (lptr, llen) = self.strings.intern(value);
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_ptr()));
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_ptr() + 1));
                        f.instruction(&Instruction::I32Const(lptr as i32));
                        f.instruction(&Instruction::I32Const(llen as i32));
                        self.emit_str_eq(scope, f);
                    }
                    // Kind mismatch is a checker error; emit a
                    // never-taken link so the chain stays well-formed.
                    _ => {
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                f.instruction(&Instruction::If(BlockType::Empty));
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
                f.instruction(&Instruction::Else);
            }
            if let Some(arm) = catch_all {
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
            }
            for _ in 0..lit_arms.len() {
                f.instruction(&Instruction::End);
            }
            return self.load_result(result_ty, scope, f);
        }

        if scrut_ty == Ty::I64 {
            bound_names.push("Int".to_string());
            f.instruction(&Instruction::LocalSet(scope.lit_scrut_i64()));
            let mut arm_scope = scope.clone();
            for n in &bound_names {
                arm_scope
                    .vars
                    .insert(n.clone(), (scope.lit_scrut_i64(), scrut_ty.clone()));
            }
            for arm in &lit_arms {
                match &arm.literal {
                    Some(ArmLiteral::Int(v)) => {
                        f.instruction(&Instruction::LocalGet(scope.lit_scrut_i64()));
                        f.instruction(&Instruction::I64Const(*v));
                        f.instruction(&Instruction::I64Eq);
                    }
                    _ => {
                        f.instruction(&Instruction::I32Const(0));
                    }
                }
                f.instruction(&Instruction::If(BlockType::Empty));
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
                f.instruction(&Instruction::Else);
            }
            if let Some(arm) = catch_all {
                self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
            }
            for _ in 0..lit_arms.len() {
                f.instruction(&Instruction::End);
            }
            return self.load_result(result_ty, scope, f);
        }

        // Unsupported scrutinee shape — the checker has already
        // reported it; keep the stack balanced.
        self.drop_value(scrut_ty, f);
        Ty::Unit
    }

    pub(super) fn compile_arm_body(
        &mut self,
        arm: &MatchArm,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) {
        let arm_scope = self.bind_arm_payload(&arm.param_ty, scope, f);
        self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
    }

    /// Body of `compile_arm_body` after payload binding: compile the
    /// arm's block in an already-prepared scope and save the result to
    /// the shared scratch locals. Literal dispatch calls this directly —
    /// its scrutinee binding replaces the union payload extraction.
    pub(super) fn compile_arm_body_prebound(
        &mut self,
        arm: &MatchArm,
        result_ty: &Ty,
        arm_scope: &LocalScope,
        f: &mut Function,
    ) {
        let scope = arm_scope;
        let body = arm.body.clone();
        let ty = self.compile_block_return(&body, scope, f);
        // Save result to scratch locals so we can reload after if/else
        match result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // ty should push (ptr, len)
                match ty {
                    Ty::Str | Ty::NamedStr(_) => {
                        f.instruction(&Instruction::LocalSet(scope.rlen()));
                        f.instruction(&Instruction::LocalSet(scope.rptr()));
                    }
                    _ => {
                        self.drop_value(ty, f);
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::LocalSet(scope.rptr()));
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::LocalSet(scope.rlen()));
                    }
                }
            }
            Ty::I64 => match ty {
                Ty::I64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I64Const(0));
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                }
            },
            Ty::I32 => match ty {
                Ty::I32 => {
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
            },
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => match ty {
                Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr => {
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rbool()));
                }
            },
            Ty::F64 => match ty {
                Ty::F64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::F64Const(0.0.into()));
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                }
            },
            // A List result is a (ptr, count) pair — same two-i32 shape
            // as a string, parked in the same rptr/rlen scratch pair.
            Ty::List => match ty {
                Ty::List => {
                    f.instruction(&Instruction::LocalSet(scope.rlen()));
                    f.instruction(&Instruction::LocalSet(scope.rptr()));
                }
                _ => {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rptr()));
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::LocalSet(scope.rlen()));
                }
            },
            _ => {
                self.drop_value(ty, f);
            }
        }
    }

    /// Extract a dispatch-arm payload from the union struct and return
    /// an extended scope that binds the arm's pattern name to the
    /// extracted value(s).
    ///
    /// The union struct lives at `scope.alloc_ptr()` (set by
    /// `compile_match` before the if/else). The layout matches what
    /// `build_union_value` writes:
    ///
    ///   * offset 0   — discriminant tag (i32)
    ///   * offset 4+  — payload, encoded by variant
    ///
    /// String payloads (`A = String`) live as `(ptr i32, len i32)` at
    /// offsets 4 and 8. We read both into `arm_payload_ptr()` and
    /// `arm_payload_ptr() + 1` so the arm body sees an ordinary
    /// string-shaped local pair.
    ///
    /// Numeric (`Int`-payload) and product-payload variants aren't
    /// extracted here yet — they remain a codegen gap. Zero-data
    /// variants (like `Pass = Unit` or stdlib `None`) have nothing to
    /// extract: the scope is returned unchanged.
    pub(super) fn bind_arm_payload(
        &self,
        param_ty: &TypeExpr,
        base_scope: &LocalScope,
        f: &mut Function,
    ) -> LocalScope {
        let mut scope = base_scope.clone();
        let (bound_name, payload_ty) = self.arm_payload_binding(param_ty);
        if bound_name.is_empty() {
            return scope;
        }
        match &payload_ty {
            Ty::Str | Ty::NamedStr(_) => {
                // Load ptr at +4 into arm_payload_ptr
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr()));
                // Load len at +8 into arm_payload_ptr + 1
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 8,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr() + 1));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_ptr(), payload_ty));
            }
            Ty::I64 => {
                // Load i64 at +4 into tmp_i64 and bind the arm's name
                // to that local. Variant payloads of `Int` user-newtype
                // (or the primitive directly) use the same 8-byte slot
                // at offset 4 of the union struct — see
                // `build_union_value` and `store_value_at_offset`.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.tmp_i64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.tmp_i64(), payload_ty));
            }
            Ty::F64 => {
                // Same slot as I64, but through the f64-typed scratch —
                // wasm locals are monomorphic, so a `Float` payload
                // can't be bound to `tmp_i64`.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.tmp_f64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.tmp_f64(), payload_ty));
            }
            Ty::I32 => {
                // Bool / discriminant-style payload at +4.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.rbool()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.rbool(), payload_ty));
            }
            Ty::Ptr | Ty::NamedPtr(_) => {
                // Boxed product payload (auto-boxed by
                // `build_union_value` for multi-field product variants,
                // or a single pointer payload): the union stores one
                // pointer at +4. Bind it in the string pair's first
                // slot — dedicated, so arm-body builtins that use the
                // ordinary scratch locals can't clobber it — and field
                // access on the bound name (`Link.Label`) reads through
                // `product_field_layout` as usual.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_ptr(), payload_ty));
            }
            _ => {
                // List payloads not yet bound.
            }
        }
        scope
    }

    /// Given an arm's pattern `TypeExpr`, return `(bound_name, payload_ty)`:
    ///
    ///   * For a user variant like `(Fail)` where `Fail = String`, the
    ///     bound name is `"Fail"` and the payload type is
    ///     `Ty::NamedStr("Fail")` (the value retains its newtype identity).
    ///   * For a stdlib variant with a type argument like `(Some<String>)`,
    ///     the bound name is the type argument (`"String"`) and the payload
    ///     type is `Ty::Str`.
    ///   * For zero-data variants (like `(None)`, `(Pass)` where `Pass = Unit`),
    ///     returns `("", Ty::Unit)` — nothing to bind.
    pub(super) fn arm_payload_binding(&self, param_ty: &TypeExpr) -> (String, Ty) {
        let TypeExpr::Named { name, generics, .. } = param_ty else {
            return (String::new(), Ty::Unit);
        };
        // Stdlib variant with explicit type argument: bind under the
        // inner type's name (e.g. `Some<String>` binds `String`).
        if !generics.is_empty() {
            if let Some(TypeExpr::Named {
                name: inner_name, ..
            }) = generics.first()
            {
                let payload_ty = self.resolve_repr(inner_name);
                return (inner_name.clone(), payload_ty);
            }
            return (String::new(), Ty::Unit);
        }
        // Zero-data variants (`Stop`, `Empty` — a variant with no
        // typedef of its own) carry nothing to bind. Without this
        // guard their repr resolves to `NamedPtr(parent)` through the
        // `variant_parent` arm of `resolve_repr` and the pointer case
        // above would bind garbage read from offset 4.
        if !self.type_defs.contains_key(name) && self.variant_parent.contains_key(name) {
            return (String::new(), Ty::Unit);
        }
        // User variant: bind under the variant's own name. The payload
        // type is the variant's repr (which walks the alias chain), so
        // `Fail` with `Fail = String` gets `Ty::NamedStr("Fail")`.
        let payload_ty = self.resolve_repr(name);
        match &payload_ty {
            Ty::Unit => (String::new(), Ty::Unit),
            _ => (name.clone(), payload_ty),
        }
    }

    /// Reload match result from scratch locals.
    pub(super) fn load_result(&self, result_ty: &Ty, scope: &LocalScope, f: &mut Function) -> Ty {
        match result_ty {
            Ty::Str | Ty::NamedStr(_) => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                result_ty.clone()
            }
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                Ty::I64
            }
            Ty::I32 => {
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                Ty::I32
            }
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                result_ty.clone()
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                Ty::F64
            }
            Ty::List => {
                f.instruction(&Instruction::LocalGet(scope.rptr()));
                f.instruction(&Instruction::LocalGet(scope.rlen()));
                Ty::List
            }
            _ => Ty::Unit,
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
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
    ///   * Scalars (`Ty::I64`/`F64`/`I32`/`Ptr`/`NamedPtr`/`NamedPtrStr`):
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: offset as u64,
                    align: 2,
                    memory_index: 0,
                }));
            }
            Ty::Str | Ty::NamedStr(_) => {
                // Stack: [addr, ptr, len]. Stash ptr+len in `tmp_i32`/
                // `tmp_i32_b` (`rptr`/`rlen` may still hold the value
                // the caller pushed via `load_from_scratch`), stash the
                // on-stack addr in `addr_scratch`, then emit the two
                // stores against it. No re-load of `alloc_ptr`: the
                // on-stack address is the only one guaranteed to point
                // at the struct being built when the payload expression
                // contained nested allocations.
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
            _ => {
                // Unexpected — just drop the address.
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

    // ── Local variable helpers ─────────────────────────────────────────────────

    pub(super) fn push_local(&self, idx: u32, repr: &Ty, f: &mut Function) {
        match repr {
            Ty::I64 | Ty::F64 => {
                f.instruction(&Instruction::LocalGet(idx));
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
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

    pub(super) fn emit_print(&self, ty: Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => {
                f.instruction(&Instruction::Call(self.fn_print_int));
            }
            Ty::F64 => {
                f.instruction(&Instruction::Call(self.fn_print_float));
            }
            Ty::I32 => {
                f.instruction(&Instruction::Call(self.fn_print_bool));
            }
            Ty::Str | Ty::NamedStr(_) => {
                // print_str writes raw bytes — we always append a single `\n`
                // (the byte at `MEM_INT_BUF_END`) so `.print` produces one
                // line of output whether the receiver is a literal or a
                // host-returned string.
                f.instruction(&Instruction::Call(self.fn_print_str));
                f.instruction(&Instruction::I32Const(MEM_INT_BUF_END as i32));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::Call(self.fn_print_str));
            }
            Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) | Ty::Ptr | Ty::List => {
                self.drop_value(ty, f); // unknown print — drop
            }
            Ty::Unit => {}
        }
        let _ = scope;
    }

    pub(super) fn drop_value(&self, ty: Ty, f: &mut Function) {
        match ty {
            Ty::Unit => {}
            Ty::I64 | Ty::F64 | Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrStr(_, _, _) => {
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
        // Reserve the wasm type for the float printer: `(f64) -> ()`.
        let print_float_ty = self.get_or_add_wasm_type(&[ValType::F64], &[]);
        // Int→String renderer: `(i64) -> (i32, i32)`; string compare:
        // `(ptr1, len1, ptr2, len2) -> i32`.
        let int_to_str_ty =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        // Map + list-growth helper shapes.
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        // Reserve the loop block type used by `compile_list_map`:
        // `(src, dst, remaining) -> (src, dst, remaining)`, all i32.
        // Block types must exist in the type section, which is emitted
        // before any user function body is compiled.
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );

        let mut m = Module::new();

        // ── Type section ───────────────────────────────────────────────
        // Indices here must match the TY_* constants above.
        let mut types = TypeSection::new();
        // 0: print_str    (i32, i32) -> ()
        types.ty().function([ValType::I32, ValType::I32], []);
        // 1: print_int    (i64) -> ()
        types.ty().function([ValType::I64], []);
        // 2: print_bool   (i32) -> ()  — also used by waitable-set.drop,
        //                                  subtask.drop, task.return,
        //                                  stream.drop-writable,
        //                                  future.drop-readable
        types.ty().function([ValType::I32], []);
        // 3: run          () -> ()   (async-stackful lift; result via task.return)
        types.ty().function([], []);
        // 4: alloc        (i32) -> (i32)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 5: stdout write-via-stream  (i32 readable) -> (i32 future)
        types.ty().function([ValType::I32], [ValType::I32]);
        // 6: stdout stream-new        () -> (i64 packed handles)
        types.ty().function([], [ValType::I64]);
        // 7: stdout stream-write      (i32 writable, i32 ptr, i32 len) -> (i32 status)
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
        // 8: handle return             () -> (i32)   — waitable-set.new
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
        // The component wrapper provides:
        //   - wasi:cli/stdout.{write-via-stream, stream-new, stream-write,
        //         stream-drop-writable, future-drop-readable}: the five
        //         canonical-ABI builtins `print_str` stitches into the
        //         native WASI P3 stdout sequence.
        //   - one function per user `extern Wasm` declaration (sorted)
        //   - canon:async/waitable.*: 6 canonical async/task helpers
        //   - env.memory, env.bump_ptr: shared linear memory + bump
        //         pointer used by `$alloc` and the host's `cabi_realloc`.
        let mut imports = ImportSection::new();
        imports.import(
            "wasi:cli/stdout",
            "write-via-stream",
            EntityType::Function(TY_STDOUT_WRITE_VIA_STREAM),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-write",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            "wasi:cli/stdout",
            "stream-drop-writable",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "wasi:cli/stdout",
            "future-drop-readable",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        for ext in &self.extern_imports {
            let type_idx = *self
                .user_type_map
                .get(&(ext.params.clone(), ext.results.clone()))
                // invariant: `assign_func_indices` registers every extern
                // import's (params, results) signature in `user_type_map`.
                .expect("extern import type was added during assign_func_indices");
            imports.import(
                &ext.core_namespace,
                &ext.fn_name,
                EntityType::Function(type_idx),
            );
        }
        // Waitable intrinsics — see field doc on `fn_waitable_*`. The
        // synthetic core instance built in `component::wrap` (from a
        // canon section emitting `waitable-set.new`, `waitable.join`,
        // `waitable-set.wait`, `waitable-set.drop`, `subtask.drop`)
        // satisfies these. Names are kebab-case to match the canon
        // operator names.
        imports.import(
            "canon:async/waitable",
            "set-new",
            EntityType::Function(TY_HANDLE_RETURN), // () -> i32
        );
        imports.import(
            "canon:async/waitable",
            "join",
            EntityType::Function(TY_PRINT_STR), // (i32, i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "set-wait",
            EntityType::Function(ty_waitable_set_wait), // (i32, i32) -> i32
        );
        imports.import(
            "canon:async/waitable",
            "set-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "subtask-drop",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> ()
        );
        imports.import(
            "canon:async/waitable",
            "task-return",
            EntityType::Function(TY_PRINT_BOOL), // (i32) -> () — result<_,_> tag
        );
        imports.import(
            "canon:async/waitable",
            "subtask-cancel",
            EntityType::Function(ty_subtask_cancel), // (i32) -> (i32)
        );
        imports.import(
            "env",
            "memory",
            EntityType::Memory(MemoryType {
                minimum: 2,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            }),
        );
        // Shared bump pointer for $alloc and the host-side `cabi_realloc`.
        imports.import(
            "env",
            "bump_ptr",
            EntityType::Global(GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            }),
        );
        m.section(&imports);

        // ── Function section ─────────────────────────────────────────────────────────
        // Defined functions in the order they appear in the function index
        // space (right after the import block).
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_PRINT_INT);
        funcs.function(TY_PRINT_BOOL);
        funcs.function(TY_ALLOC);
        funcs.function(TY_RUN); // exported run() -> i32
        funcs.function(list_to_json_array_ty); // list → json array helper
        funcs.function(print_float_ty); // float printer (f64) -> ()
        funcs.function(int_to_str_ty); // Int→String renderer (i64) -> (i32, i32)
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
        m.section(&funcs);

        // ── Memory section ───────────────────────────────────────────────────────────────
        // We import the memory rather than declaring our own — the component
        // wrapper instantiates a tiny "memory provider" core module first so
        // that the canonical-ABI lowers (which need a memory option) can
        // reference it before this module is instantiated.

        // ── Global section ─────────────────────────────────────────────────────────────────
        // Empty — the bump_ptr global is imported, not defined here.

        // ── Export section ─────────────────────────────────────────────────────────────
        // The Component Model wrapper lifts `run` as `wasi:cli/run.run`.
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, self.fn_start);
        m.section(&exports);

        // ── Code section ─────────────────────────────────────────────────────────────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_print_int());
        codes.function(&self.build_print_bool());
        codes.function(&self.build_alloc());
        codes.function(&self.build_start());
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_print_float());
        codes.function(&self.build_int_to_str());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
        // User functions — one body per `compiled_user_funcs` entry, in
        // func-index order (matches the function section above exactly).
        let ordered_funcs: Vec<FunctionDef> = self
            .compiled_user_funcs
            .iter()
            .map(|(_, _, func)| func.clone())
            .collect();
        for func in ordered_funcs {
            let compiled = self.build_user_function(&func);
            codes.function(&compiled);
        }
        m.section(&codes);

        // ── Data section ──────────────────────────────────────────────────────
        let mut data = DataSection::new();
        // '\n' at offset MEM_INT_BUF_END
        data.active(0, &ConstExpr::i32_const(MEM_INT_BUF_END as i32), [b'\n']);
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
