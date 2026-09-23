//! Dispatch: a union, `Bool`, literal, or binding dispatch, and binding an arm's payload.
use super::compile::*;
use super::*;

impl<'m> WasmGen<'m> {
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

        // Binding dispatch: a single no-literal, non-variant arm always
        // runs — no comparison, pure binding of the scrutinee under the
        // arm's pattern name (the checker guarantees the pattern names
        // the scrutinee's type).
        if let [arm] = arms {
            if arm.literal.is_none() && !self.arm_is_variant_arm(arm, &scrut_ty) {
                return self.emit_binding_dispatch(scrut_ty, arm, &arm_result_ty, scope, f);
            }
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
        // `NamedPtr` and `NamedPtrOf` share an in-memory layout, so both
        // dispatch the same way — the only difference is that
        // `NamedPtrOf` carries enough type info for arms to extract the
        // string payload (handled in `compile_arm_body`).
        let union_name = match &scrut_ty {
            Ty::NamedPtr(n) => Some(n.clone()),
            Ty::NamedPtrOf(n, _, _) => Some(n.clone()),
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

    /// Whether a dispatch arm's pattern is a *variant* of the scrutinee
    /// (so the union/Bool machinery owns it) rather than the scrutinee's
    /// own type (the binding-dispatch shape). A pattern naming the union
    /// itself is not a variant arm.
    pub(super) fn arm_is_variant_arm(&self, arm: &MatchArm, scrut_ty: &Ty) -> bool {
        let Some(name) = arm_type_name(arm) else {
            return false;
        };
        match scrut_ty {
            Ty::I32 => arm_tag(arm).is_some(),
            Ty::NamedPtr(n) | Ty::NamedPtrOf(n, _, _) => {
                if name == n {
                    return false;
                }
                matches!(name, "Some" | "None" | "Ok" | "Err")
                    || self
                        .union_variants
                        .get(n)
                        .is_some_and(|vs| vs.iter().any(|v| v == name))
            }
            _ => false,
        }
    }

    /// Compile a binding dispatch: the single arm always runs, so no
    /// comparison is emitted — the scrutinee is stashed in the dedicated
    /// `bind_scrut_*` locals and bound inside the arm body under the
    /// arm's pattern name, the scrutinee's own type name, and (for a
    /// bare primitive) the primitive's name, mirroring the
    /// literal-dispatch catch-all. Same single-slot nesting caveat as
    /// `lit_scrut_ptr`: a binding dispatch nested inside another binding
    /// dispatch's arm body reuses the slots.
    pub(super) fn emit_binding_dispatch(
        &mut self,
        scrut_ty: Ty,
        arm: &MatchArm,
        result_ty: &Ty,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let mut bound_names: Vec<String> = Vec::new();
        if let Some(n) = arm_type_name(arm) {
            bound_names.push(n.to_string());
        }
        if let Some(n) = scrut_ty.canon_name() {
            bound_names.push(n.to_string());
        }
        match &scrut_ty {
            Ty::Str => bound_names.push("String".to_string()),
            Ty::I64 => bound_names.push("Int".to_string()),
            Ty::F64 => bound_names.push("Float".to_string()),
            Ty::I32 => bound_names.push("Bool".to_string()),
            _ => {}
        }
        let slot = match &scrut_ty {
            Ty::Unit => None,
            Ty::I64 => {
                f.instruction(&Instruction::LocalSet(scope.bind_scrut_i64()));
                Some(scope.bind_scrut_i64())
            }
            Ty::F64 => {
                f.instruction(&Instruction::LocalSet(scope.bind_scrut_f64()));
                Some(scope.bind_scrut_f64())
            }
            Ty::Str | Ty::NamedStr(_) | Ty::List => {
                f.instruction(&Instruction::LocalSet(scope.bind_scrut_ptr() + 1));
                f.instruction(&Instruction::LocalSet(scope.bind_scrut_ptr()));
                Some(scope.bind_scrut_ptr())
            }
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                f.instruction(&Instruction::LocalSet(scope.bind_scrut_ptr()));
                Some(scope.bind_scrut_ptr())
            }
        };
        let mut arm_scope = scope.clone();
        if let Some(idx) = slot {
            for n in &bound_names {
                arm_scope.vars.insert(n.clone(), (idx, scrut_ty.clone()));
            }
        }
        self.compile_arm_body_prebound(arm, result_ty, &arm_scope, f);
        self.load_result(result_ty, scope, f)
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
            Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) | Ty::Ptr => match ty {
                Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) | Ty::Ptr => {
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
        // The arm body is one dispatch deeper, so anything it binds
        // takes the next pair down and leaves this arm's name intact.
        scope.arm_depth = base_scope.arm_depth + 1;
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
                // Load i64 at +4 into the depth's dedicated i64 and
                // bind the arm's name to it. Variant payloads of `Int`
                // user-newtype (or the primitive directly) use the same
                // 8-byte slot at offset 4 of the union struct — see
                // `build_union_value` and `store_value_at_offset`.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::I64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_i64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_i64(), payload_ty));
            }
            Ty::F64 => {
                // Same slot as I64, through the f64-typed local — wasm
                // locals are monomorphic.
                f.instruction(&Instruction::LocalGet(base_scope.alloc_ptr()));
                f.instruction(&Instruction::F64Load(MemArg {
                    offset: 4,
                    align: 3,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_f64()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_f64(), payload_ty));
            }
            Ty::I32 => {
                // Bool / discriminant-style payload at +4, in the
                // pointer pair's first slot.
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
            Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
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
            Ty::List => {
                // A list payload is a `(ptr, len)` pair at +4 / +8, the
                // same slots a string payload uses.
                self.load_payload_at(base_scope.alloc_ptr(), 4, &payload_ty, f);
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr() + 1));
                f.instruction(&Instruction::LocalSet(base_scope.arm_payload_ptr()));
                scope
                    .vars
                    .insert(bound_name, (base_scope.arm_payload_ptr(), payload_ty));
            }
            Ty::Unit => {}
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
            Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
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
}
