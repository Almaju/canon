//! The builtin pipe vocabulary (`compile_builtin_method`): string, number, list, and host-introspection operations on a receiver.
use super::compile::*;
use super::*;

impl<'m> WasmGen<'m> {
    pub(super) fn compile_builtin_method(
        &mut self,
        // The receiver expression, needed only to recover a list's
        // element type — `Ty::List` does not carry one. See
        // `list_elem_is_string`.
        receiver: &Expr,
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
        // A stream's producers, consumers and transforms — see `stream`.
        // The lambdas inline like the list ones: `Mapped` and `Unfolded`
        // into their stage functions, `Folded` into the pull loop.
        if let ("unfold", [lambda]) = (method, args) {
            return self.compile_unfolded(recv_ty, lambda, scope, f);
        }
        if self.is_stream_ty(&recv_ty) {
            match method {
                "first" => return self.compile_stream_next(scope, f),
                "take" => return self.compile_stream_take(args, scope, f),
                "map" => {
                    if let Some(Expr::Lambda {
                        params, body, span, ..
                    }) = args.first()
                    {
                        if let [param] = params.as_slice() {
                            if let TypeExpr::Named { name, .. } = &param.ty {
                                return self.compile_stream_map(name, body, *span, scope, f);
                            }
                        }
                    }
                }
                "fold" => {
                    if let Some((init, acc_name, elem_name, body)) = self.inline_fold_lambda(args) {
                        return self
                            .compile_stream_fold(init, &acc_name, &elem_name, &body, scope, f);
                    }
                }
                _ => {}
            }
        }
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
            // Only the two base comparisons are builtins (wasm numerics);
            // the derived comparisons (`ne`/`le`/`gt`/`ge`) are pure Canon
            // dispatch over these in `canon/int.can`.
            ("lt", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64LtS);
                Ty::I32
            }
            ("eq", Ty::I64) => {
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I64Eq);
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
            // Base comparisons only, as for Int — the derived comparisons
            // live in `canon/float.can`. Their IEEE semantics survive
            // the port: `Gt` is `Lt` with swapped operands, `Le`/`Ge` are
            // `Lt`-or-`Eq`, and `Ne` is not-`Eq` — all exact under NaN
            // (every ordered comparison with a NaN operand is false).
            ("lt", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Lt);
                Ty::I32
            }
            ("eq", Ty::F64) => {
                self.compile_f64_arg(args, scope, f);
                f.instruction(&Instruction::F64Eq);
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
            // Like `Int`/`Float`, only `lt` (and `eq` above) is a
            // builtin; the derived comparisons dispatch over the two
            // in `canon/string.can`.
            ("lt", _) if recv_ty.is_str_like() && args.len() == 1 => {
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
                f.instruction(&Instruction::I32LtS);
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
                if let Some((name, elem, body)) = self.inline_unary_lambda(args) {
                    return self.compile_list_map(&name, &elem, &body, scope, f);
                }
                self.identity_list_fallback(args, scope, f)
            }
            ("filter", Ty::List) => {
                // Element-wise filter when the predicate is an inline
                // lambda — same inlining rule as `map` above (Canon
                // lambdas are non-capturing).
                if let Some((name, elem, body)) = self.inline_unary_lambda(args) {
                    return self.compile_list_filter(&name, &elem, &body, scope, f);
                }
                self.identity_list_fallback(args, scope, f)
            }
            ("take", Ty::List) => {
                // `take(n)` clamps the length — list slots are
                // contiguous, so the prefix IS the taken list. Negative
                // n clamps to 0; n past the end keeps everything.
                // Stack: [ptr, len].
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // n
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                                                                          // n0 = n < 0 ? 0 : n
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32LtS);
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                // new_len = n0 < len ? n0 : len
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32LtS);
                f.instruction(&Instruction::Select);
                Ty::List
            }
            ("skip", Ty::List) => {
                // `skip(n)` drops the first n elements: the slots are
                // contiguous, so the remainder IS the list n slots in
                // and n shorter. n is clamped to [0, len], so an empty
                // list stays empty and skipping past the end empties
                // it. Stack: [ptr, len].
                self.compile_i64_arg(args, scope, f);
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // n
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // ptr
                                                                             // n0 = n < 0 ? 0 : n
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32LtS);
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                // n1 = n0 < len ? n0 : len
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32LtS);
                f.instruction(&Instruction::Select);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                // [ptr + n1*8, len - n1]
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Sub);
                Ty::List
            }
            ("sort", Ty::List) => {
                // Insertion sort over a fresh copy of the slots (a list
                // is a value; the source may be shared), ordering by the
                // element's own `Lt`: `i64.lt_s` / `f64.lt` for scalars,
                // `fn_str_cmp` for strings. No user code runs, so the
                // scratch locals are safe: `tmp_i32` = n, `tmp_i32_b` =
                // src then j, `addr_scratch` = dst, `rbool` = i,
                // `tmp_i64` = the slot being moved. Stack: [ptr, len].
                let elem = self
                    .list_elem_name(receiver)
                    .map(|n| self.resolve_repr(&n))
                    .unwrap_or(Ty::I64);
                let mem64 = MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                };
                let mem32 = |offset: u64| MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                };
                // dst + j*8 - back*8
                let slot_j = |f: &mut Function, back: i32| {
                    f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                    f.instruction(&Instruction::I32Const(back));
                    f.instruction(&Instruction::I32Sub);
                    f.instruction(&Instruction::I32Const(8));
                    f.instruction(&Instruction::I32Mul);
                    f.instruction(&Instruction::I32Add);
                };
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // n
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // src
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // dst
                                                                             // Copy: dst[i] = src[i].
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Block(BlockType::Empty));
                f.instruction(&Instruction::Loop(BlockType::Empty));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32GeU);
                f.instruction(&Instruction::BrIf(1));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::I64Store(mem64));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Br(0));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::End);
                // for i in 1..n: j = i; while j > 0 && dst[j] < dst[j-1]: swap, j -= 1
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Block(BlockType::Empty));
                f.instruction(&Instruction::Loop(BlockType::Empty));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32GeU);
                f.instruction(&Instruction::BrIf(1));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // j = i
                f.instruction(&Instruction::Block(BlockType::Empty));
                f.instruction(&Instruction::Loop(BlockType::Empty));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::BrIf(1));
                // dst[j] < dst[j-1]?
                match &elem {
                    Ty::F64 => {
                        slot_j(f, 0);
                        f.instruction(&Instruction::F64Load(mem64));
                        slot_j(f, 1);
                        f.instruction(&Instruction::F64Load(mem64));
                        f.instruction(&Instruction::F64Lt);
                    }
                    Ty::Str | Ty::NamedStr(_) => {
                        slot_j(f, 0);
                        f.instruction(&Instruction::I32Load(mem32(0)));
                        slot_j(f, 0);
                        f.instruction(&Instruction::I32Load(mem32(4)));
                        slot_j(f, 1);
                        f.instruction(&Instruction::I32Load(mem32(0)));
                        slot_j(f, 1);
                        f.instruction(&Instruction::I32Load(mem32(4)));
                        f.instruction(&Instruction::Call(self.fn_str_cmp));
                        f.instruction(&Instruction::I32Const(0));
                        f.instruction(&Instruction::I32LtS);
                    }
                    _ => {
                        slot_j(f, 0);
                        f.instruction(&Instruction::I64Load(mem64));
                        slot_j(f, 1);
                        f.instruction(&Instruction::I64Load(mem64));
                        f.instruction(&Instruction::I64LtS);
                    }
                }
                f.instruction(&Instruction::I32Eqz);
                f.instruction(&Instruction::BrIf(1));
                // swap
                slot_j(f, 0);
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                slot_j(f, 0);
                slot_j(f, 1);
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::I64Store(mem64));
                slot_j(f, 1);
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I64Store(mem64));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                f.instruction(&Instruction::Br(0));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Br(0));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::List
            }
            ("reverse", Ty::List) => {
                // Copy the raw 8-byte slots into a fresh list back to
                // front. No user code runs, so the scratch locals are
                // safe: `tmp_i32` = len, `tmp_i32_b` = src,
                // `addr_scratch` = dst, `rbool` = i. Stack: [ptr, len].
                let mem64 = MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                };
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                // dst = alloc(len*8 + 8) — the +8 keeps a zero-length
                // list from handing $alloc a zero size.
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Block(BlockType::Empty));
                f.instruction(&Instruction::Loop(BlockType::Empty));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32GeU);
                f.instruction(&Instruction::BrIf(1));
                // dst[i] = src[len - 1 - i]
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Sub);
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::I32Mul);
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::I64Load(mem64));
                f.instruction(&Instruction::I64Store(mem64));
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Add);
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                f.instruction(&Instruction::Br(0));
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::End);
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                Ty::List
            }
            ("Stream", Ty::List) => self.compile_list_to_stream(scope, f),
            ("fold", Ty::List) => {
                // `list -> Folded(init * (Acc * Elem) => Acc { … })`:
                // the lambda's return type names the accumulator, the
                // product parameter's other component is the element.
                if let Some((init, acc_name, elem_name, body)) = self.inline_fold_lambda(args) {
                    return self.compile_list_fold(init, &acc_name, &elem_name, &body, scope, f);
                }
                self.identity_list_fallback(args, scope, f)
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
                self.option_ty_for_list(receiver)
            }
            // NOTE: `Map` / `Set` methods are NOT built in — they are
            // pure Canon (`canon/Map`, `canon/Set`) and resolve
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
                    Ty::Str | Ty::NamedStr(_) | Ty::List => {
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
                    Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                        f.instruction(&Instruction::I64ExtendI32U);
                    }
                    Ty::Unit => {
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
                self.option_ty_for_list(receiver)
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
            // `request.header(name)` — `[method]request.get-headers`
            // hands back an owned `fields`, and `[method]fields.get`
            // answers `list<field-value>` through a ret area (ptr/len at
            // +0/+4, each value a `list<u8>` pair). The first value is
            // the `Some` payload; no value is `None`. Same `Option`
            // struct as `path`.
            ("header", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                let mem32 = |offset: u64| MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                };
                // Stack: [request]. The name is arbitrary user code —
                // compile it before touching any scratch local.
                let ty = args
                    .first()
                    .map(|a| self.compile_expr(a, scope, f))
                    .unwrap_or(Ty::Unit);
                if !ty.is_str_like() {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(0));
                    f.instruction(&Instruction::I32Const(0));
                }
                f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // nlen
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // nptr
                f.instruction(&Instruction::Call(FN_HTTP_GET_HEADERS));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // fields
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // ret area
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_GET));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_DROP));
                f.instruction(&Instruction::I32Const(12));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.rbool()));
                // tag = (count != 0)
                f.instruction(&Instruction::LocalGet(scope.rbool()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(mem32(4)));
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Ne);
                f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
                f.instruction(&Instruction::I32Store(mem32(0)));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::If(BlockType::Empty));
                // values ptr → the first value's (ptr, len) pair
                f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                f.instruction(&Instruction::I32Load(mem32(0)));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                for off in [0u64, 4] {
                    f.instruction(&Instruction::LocalGet(scope.rbool()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                    f.instruction(&Instruction::I32Load(mem32(off)));
                    f.instruction(&Instruction::I32Store(mem32(off + 4)));
                }
                f.instruction(&Instruction::End);
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
            ("body", Ty::NamedPtr(ref n)) if n == "Request" && self.http_mode => {
                // Stack: [request]. `consume-body` moves the request —
                // the `handle` wrapper reads the flag and skips its
                // drop — and hands back the body stream and the
                // trailers future; the `res` future it takes is
                // resolved to `ok` when the stream ends.
                let mem32 = |offset: u64| MemArg {
                    offset,
                    align: 2,
                    memory_index: 0,
                };
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_CONSUMED as i32));
                f.instruction(&Instruction::I32Const(1));
                f.instruction(&Instruction::I32Store(mem32(0)));
                f.instruction(&Instruction::Call(FN_HTTP_RES_FUTURE_NEW));
                f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.par_seen_a()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                f.instruction(&Instruction::I64Const(32));
                f.instruction(&Instruction::I64ShrU);
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(scope.par_seen_b()));
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalSet(scope.par_set()));
                f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                f.instruction(&Instruction::LocalGet(scope.par_seen_a()));
                f.instruction(&Instruction::LocalGet(scope.par_set()));
                f.instruction(&Instruction::Call(FN_HTTP_CONSUME_BODY));
                f.instruction(&Instruction::LocalGet(scope.par_set()));
                f.instruction(&Instruction::I32Load(mem32(0)));
                f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
                f.instruction(&Instruction::LocalGet(scope.par_set()));
                f.instruction(&Instruction::I32Load(mem32(4)));
                f.instruction(&Instruction::LocalSet(scope.par_event_ptr()));
                self.emit_host_stream(
                    stream::Stage::Host {
                        read_fn: FN_HTTP_BODY_READ,
                        drop_stream_fn: FN_HTTP_BODY_DROP_READABLE,
                        drop_future_fn: FN_HTTP_BODY_TRAILERS_DROP_READABLE,
                        third: stream::Third::Settled {
                            write_fn: FN_HTTP_RES_FUTURE_WRITE,
                            drop_fn: FN_HTTP_RES_FUTURE_DROP_WRITABLE,
                            ok_at: MEM_HTTP_TRAILERS_ZERO,
                        },
                    },
                    scope.tmp_i32_b(),
                    scope.par_event_ptr(),
                    Some(scope.par_seen_b()),
                    scope,
                    f,
                );
                Ty::NamedPtr("Stream".to_string())
            }
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
                // WIT declaration order (packages/canon/wit/wasi/http.wit).
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
}
