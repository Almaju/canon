//! List vocabulary lowered inline: `Mapped`, `Folded`, `Filtered` over a list, with the lambda compiled in the loop.
use super::*;

impl<'m> WasmGen<'m> {
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
    /// The reserved `(i32,i32,i32) -> (i32,i32,i32)` block type that
    /// carries the `(src, dst, remaining)` trio through the
    /// `compile_list_map` / `compile_list_filter` loops.
    pub(super) fn list_loop_trio_ty(&self) -> u32 {
        self.user_type_map
            .get(&(
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32, ValType::I32, ValType::I32],
            ))
            .copied()
            // invariant: `compile()` reserves this (i32,i32,i32)->(i32,i32,i32)
            // loop type in `user_type_map` before any list loop is compiled.
            .expect("list-loop trio type reserved in compile()")
    }

    /// Loads the current list element (slot at `addr_scratch`) into the
    /// map-element locals per its repr: i64 slots into `map_elem_i64`,
    /// f64 slots into `map_elem_f64`, everything else — a `(ptr, len)`
    /// pair, or one pointer / `Bool` in the low half — into
    /// `map_elem_ptr`/`+1`.
    pub(super) fn bind_list_elem(&self, elem_repr: &Ty, scope: &LocalScope, f: &mut Function) {
        match elem_repr {
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.map_elem_i64()));
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.map_elem_f64()));
            }
            _ => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr()));
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(scope.map_elem_ptr() + 1));
            }
        }
    }

    /// The scope a list-lambda body compiles in: the parameter's whole
    /// alias chain bound to the element local for its repr.
    pub(super) fn lambda_elem_scope(
        &self,
        elem_name: &str,
        elem_repr: &Ty,
        scope: &LocalScope,
    ) -> LocalScope {
        let elem_local = match elem_repr {
            Ty::I64 => scope.map_elem_i64(),
            Ty::F64 => scope.map_elem_f64(),
            _ => scope.map_elem_ptr(),
        };
        let mut inner = LocalScope {
            vars: scope.vars.clone(),
            param_count: scope.param_count,
            arm_depth: scope.arm_depth,
        };
        for alias in self.collect_alias_chain(elem_name) {
            inner.vars.insert(alias, (elem_local, elem_repr.clone()));
        }
        inner
    }

    /// The `(elem_name, elem_repr, body)` of an inline unary lambda
    /// argument whose element repr the list loops support. `None` sends
    /// the caller to `identity_list_fallback`.
    pub(super) fn inline_unary_lambda(&self, args: &[Expr]) -> Option<(String, Ty, Block)> {
        if let Some(Expr::Lambda { params, body, .. }) = args.first() {
            if params.len() == 1 {
                if let TypeExpr::Named { name, .. } = &params[0].ty {
                    let elem = self.resolve_repr(name);
                    if !matches!(elem, Ty::Unit) {
                        return Some((name.clone(), elem, body.clone()));
                    }
                }
            }
        }
        None
    }

    /// Identity fallback for a list transform whose argument shape
    /// isn't inlinable: compile-and-drop the args, pass the receiver's
    /// `(ptr, len)` through unchanged.
    pub(super) fn identity_list_fallback(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
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

    pub(super) fn compile_list_map(
        &mut self,
        elem_name: &str,
        elem_repr: &Ty,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self.list_loop_trio_ty();
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
        self.bind_list_elem(elem_repr, scope, f);
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
        let inner = self.lambda_elem_scope(elem_name, elem_repr, scope);
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
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

    /// Compiles `list.Filtered(lambda)` as a fused loop over the list.
    /// Structure mirrors `compile_list_map`: the loop-carried state
    /// `(src, dst, remaining)` lives on the operand stack (the reserved
    /// i32-trio block type) while the predicate body runs, so arbitrary
    /// user code inside the lambda can't clobber it. Kept elements are
    /// copied as raw 8-byte slots — the slot layout is identical for
    /// every element repr, so one `i64` move covers both scalar and
    /// string-shaped elements. The destination is allocated at full
    /// source length (a safe upper bound); the final length is the
    /// number of kept elements.
    /// The `(init, acc_name, elem_name, body)` of a fold argument —
    /// `init * lambda` in either order, the lambda taking one product
    /// parameter whose component naming the lambda's return type (or
    /// aliasing to it) is the accumulator and whose other component is
    /// the element. `None` when the argument has any other shape.
    pub(super) fn inline_fold_lambda<'e>(
        &self,
        args: &'e [Expr],
    ) -> Option<(&'e Expr, String, String, Block)> {
        let parts: &[Expr] = match args {
            [Expr::ProductValue { fields, .. }] => fields,
            other => other,
        };
        if parts.len() != 2 {
            return None;
        }
        let (lambda, init) = match parts {
            [l @ Expr::Lambda { .. }, i] | [i, l @ Expr::Lambda { .. }] => (l, i),
            _ => return None,
        };
        let Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } = lambda
        else {
            return None;
        };
        let [param] = params.as_slice() else {
            return None;
        };
        let TypeExpr::Product { fields, .. } = &param.ty else {
            return None;
        };
        let [a, b] = fields.as_slice() else {
            return None;
        };
        let (a, b) = (named_type_name(a)?, named_type_name(b)?);
        let ret = named_type_name(return_ty)?;
        let is_acc = |n: &str| self.collect_alias_chain(n).contains(&ret);
        let (acc, elem) = if a == ret || (is_acc(&a) && !is_acc(&b)) {
            (a, b)
        } else {
            (b, a)
        };
        Some((init, acc, elem, body.clone()))
    }

    /// Store the value on the stack into the fold-accumulator locals
    /// for its repr; `bind_fold_acc` is the read side.
    pub(super) fn store_fold_acc(&self, ty: &Ty, scope: &LocalScope, f: &mut Function) {
        match ty {
            Ty::I64 => f.instruction(&Instruction::LocalSet(scope.fold_acc_i64())),
            Ty::F64 => f.instruction(&Instruction::LocalSet(scope.fold_acc_f64())),
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.fold_acc_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.fold_acc_ptr()))
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.fold_acc_ptr()))
            }
            Ty::Unit => f,
        };
    }

    /// Compiles `list.Folded(init * lambda)`: the accumulator lives in
    /// the dedicated `fold_acc_*` locals across iterations, the current
    /// element in the map-element locals, and the loop-carried
    /// `(src, _, remaining)` on the operand stack (reusing the map/filter
    /// trio block type with an unused middle slot) where the lambda body
    /// can't clobber it.
    pub(super) fn compile_list_fold(
        &mut self,
        init: &Expr,
        acc_name: &str,
        elem_name: &str,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self.list_loop_trio_ty();
        let acc_repr = self.resolve_repr(acc_name);
        let elem_repr = self.resolve_repr(elem_name);

        // ── Setup. Stack: [src, len] ─────────────────────────────────
        // The initial accumulator is user code: compile it before
        // touching any scratch local, then park it in the fold locals.
        let init_ty = self.compile_expr(init, scope, f);
        self.store_fold_acc(&init_ty, scope, f);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // len
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));

        f.instruction(&Instruction::Block(BlockType::FunctionType(trio)));
        f.instruction(&Instruction::Loop(BlockType::FunctionType(trio)));
        // [src, 0, rem] — exit when rem == 0.
        f.instruction(&Instruction::LocalTee(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // rem
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // src
        self.bind_list_elem(&elem_repr, scope, f);
        // Park the next iteration's state: [src + 8, 0, rem - 1].
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);

        // ── The lambda body ──────────────────────────────────────────
        // The element binds under its whole alias chain; the
        // accumulator under its exact name only, so an accumulator
        // newtype (`Total = Int`) never shadows an `Int` element.
        let mut inner = self.lambda_elem_scope(elem_name, &elem_repr, scope);
        let acc_local = match acc_repr {
            Ty::I64 => scope.fold_acc_i64(),
            Ty::F64 => scope.fold_acc_f64(),
            _ => scope.fold_acc_ptr(),
        };
        inner
            .vars
            .insert(acc_name.to_string(), (acc_local, acc_repr.clone()));
        let out_ty = self.compile_block_return(body, &inner, f);
        self.store_fold_acc(&out_ty, scope, f);
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // loop
        f.instruction(&Instruction::End); // block

        // [src_f, 0, rem_f] → the accumulator.
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::Drop);
        self.push_local(acc_local, &acc_repr, f);
        acc_repr
    }

    pub(super) fn compile_list_filter(
        &mut self,
        elem_name: &str,
        elem_repr: &Ty,
        body: &Block,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let trio = self.list_loop_trio_ty();
        let mem64 = MemArg {
            offset: 0,
            align: 3,
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
                                                                  // Bottom-of-stack survivor: dst_base (the final kept-count is
                                                                  // recovered from it as (dst_final - dst_base) / 8) …
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
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
        self.bind_list_elem(elem_repr, scope, f);
        // Park the next iteration's state and the current raw slot on
        // the operand stack where the predicate body can't touch them:
        // [new_src, dst_cur, new_rem, raw_slot]. The raw slot is kept
        // because the body may clobber the element locals (nested map).
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        match elem_repr {
            // The i64 element local already holds the slot verbatim.
            Ty::I64 => {
                f.instruction(&Instruction::LocalGet(scope.map_elem_i64()));
            }
            _ => {
                f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
                f.instruction(&Instruction::I64Load(mem64));
            }
        }

        // ── The predicate body ───────────────────────────────────────
        let inner = self.lambda_elem_scope(elem_name, elem_repr, scope);
        let out_ty = self.compile_block_return(body, &inner, f);
        if !matches!(out_ty, Ty::I32) {
            // Non-Bool predicate result (checker-rejected shapes) —
            // drop it and keep nothing.
            self.drop_value(out_ty, f);
            f.instruction(&Instruction::I32Const(0));
        }

        // [new_src, dst_cur, new_rem, raw_slot, keep?] → locals.
        f.instruction(&Instruction::LocalSet(scope.rbool())); // keep?
        f.instruction(&Instruction::LocalSet(scope.tmp_i64())); // raw_slot
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // new_rem
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst_cur
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // new_src
                                                                     // Kept: store the raw slot at dst_cur and advance dst.
        f.instruction(&Instruction::LocalGet(scope.rbool()));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Store(mem64));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b()));
        f.instruction(&Instruction::End);
        // Rebuild the trio and continue.
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End); // loop
        f.instruction(&Instruction::End); // block

        // [dst_base, src_f, dst_f, rem_f] → [dst_base, kept].
        f.instruction(&Instruction::Drop); // rem_f
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // dst_f
        f.instruction(&Instruction::Drop); // src_f
        f.instruction(&Instruction::LocalTee(scope.addr_scratch())); // dst_base
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(3));
        f.instruction(&Instruction::I32ShrU);
        Ty::List
    }
}
