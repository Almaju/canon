//! Constructor compilation: products, unions, options and results, list literals, and routing a call's inputs to the components they bind by type.
use super::compile::*;
use super::*;

impl<'m> WasmGen<'m> {
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
        syntactic_type_name(e)
            .is_some_and(|name| self.collect_alias_chain(name).iter().any(|n| n == "Byte"))
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

    /// Emit a call to the stdlib `String` constructor family member for
    /// `recv` (`"Bool"` / `"Float"` / `"Int"`) — the pure-Canon decimal
    /// renderers in `canon/string.can`. The receiver value is
    /// already on the stack. The loader's string prelude
    /// (`inject_string_prelude`) loads the module wherever a render
    /// site compiles, so a miss here is a compiler bug.
    pub(super) fn emit_render_to_str(
        &mut self,
        recv: &str,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let info = self
            .func_table
            .get(&(Some(recv.to_string()), "String".to_string()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("`canon/String` carries the `{recv} => String` renderer (string prelude)")
            });
        self.emit_func_table_call(&info, &[], scope, f)
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
                        ("Int", Ty::F64) => {
                            f.instruction(&Instruction::I64TruncF64S);
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
                                self.emit_render_to_str("Int", scope, f)
                            }
                        }
                        ("String", Ty::F64) => self.emit_render_to_str("Float", scope, f),
                        ("String", Ty::I32) => self.emit_render_to_str("Bool", scope, f),
                        ("Int", ty) if ty.is_str_like() => {
                            // `Int("42")` — the fallible parse constructor
                            // from `canon/Int`. The compiled string is
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
            // pure-Canon `canon/Map` / `canon/Set` recursive
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
                        for cand in self.dispatch_candidates(&first_ty) {
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
                        TypeExpr::Product { .. } | TypeExpr::Repeat { .. } => {
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
                                    // Zero-width target (`Printed = Unit`, an
                                    // evidence newtype): the argument is
                                    // evaluated for its effects and dropped —
                                    // the same value-discarding the piped
                                    // spelling (`x -> Printed`) compiles to.
                                    Ty::Unit => {
                                        self.drop_value(arg_ty, f);
                                        repr
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
    pub(super) fn builtin_result_type(
        &self,
        method: &str,
        receiver: &Expr,
        args: &[Expr],
    ) -> Option<String> {
        // A constructor family of the same name owns it (`Depth * Tokens
        // => Skipped` in canonc): the builtin never applies, and the
        // family's declared result is what the chain carries.
        if self.func_table.keys().any(|(_, m)| m == method) {
            return None;
        }
        match method {
            // `Ne`/`Le`/`Gt`/`Ge` — like `And`/`Or`/`Not` — are stdlib
            // result newtypes of `Bool` now, kept here as the static-type
            // fallback for chains (each erases to `Bool` on the stack).
            "Eq" | "Ne" | "Lt" | "Le" | "Gt" | "Ge" | "And" | "Or" | "Not" => {
                Some("Bool".to_string())
            }
            "Length" | "ByteAt" => Some("Int".to_string()),
            "Joined" | "Substring" => Some("String".to_string()),
            // List transforms preserve list-ness; a fold yields its
            // accumulator, the lambda's declared return type.
            "Mapped" | "Appended" | "Skipped" | "Reversed" | "Sorted" => Some("List".to_string()),
            "Folded" => crate::checker::fold_result_type(method, args),
            // The base's operation with the base's result: `Width ->
            // Difference(1)` is an `Int` (the checker's
            // `expr_type_name_in_scope` agrees), so binding it to a
            // `Width` slot takes a relabel.
            "Sum" | "Difference" | "Product" | "Quotient" | "Remainder" => self
                .infer_ctor_arg_type_name(receiver)
                .map(|r| self.resolve_repr(&r))
                .and_then(|repr| match repr {
                    Ty::I64 => Some("Int".to_string()),
                    Ty::F64 => Some("Float".to_string()),
                    _ => None,
                }),
            _ => None,
        }
    }

    pub(super) fn infer_ctor_arg_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.name.clone()),
            // Newtype unwrap (`x.String`) — a PascalCase field names the
            // component's type, which *is* the value's type.
            Expr::FieldAccess { field, .. } if crate::ast::is_type_name(&field.name) => {
                Some(field.name.clone())
            }
            // A repetition component (`Limbs.1`) is one `Limbs`.
            Expr::FieldAccess {
                receiver, field, ..
            } if field.name.parse::<u64>().is_ok() => match receiver.as_ref() {
                Expr::Ident(id) => Some(id.name.clone()),
                _ => None,
            },
            // A method chain's static type comes from the callee's
            // registered result type — this is what lets a pipe hang off
            // a chain (`Map() -> Insert(…) -> Keys`). Builtin
            // methods aren't in `func_table`, so chains ending in them
            // still return `None` and the call falls through to the
            // pre-pipe routing paths.
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                // An unknown receiver type only rules out the
                // func-table lookup — the fallbacks below read the
                // method name alone, so a chain whose receiver isn't
                // statically typed (a `?` unwrap, say) still resolves
                // `-> Source` to `Source`. Bailing here instead cost
                // the caller its type, and `commutative_order` silently
                // kept written order for a call whose components then
                // landed in the wrong slots.
                let recv = self.infer_ctor_arg_type_name(receiver);
                if let Some((target, _)) = recv
                    .as_deref()
                    .and_then(|r| self.message_target(r, &method.name))
                {
                    return Some(target);
                }
                for c in recv.into_iter().flat_map(|r| self.dispatch_candidates(&r)) {
                    if let Some(info) = self.func_table.get(&(Some(c), method.name.clone())) {
                        // A constructor's result is its own type, and a
                        // bodied declaration is always named after the
                        // type it constructs. Reading the registered
                        // result type instead loses a scalar newtype's
                        // name to the primitive it erases to — and that
                        // name is exactly what tells two `Int` newtypes
                        // apart when a call binds its inputs by type.
                        if self.type_defs.contains_key(&method.name) {
                            return Some(method.name.clone());
                        }
                        return match &info.result_ty {
                            Ty::NamedPtr(n) | Ty::NamedStr(n) | Ty::NamedPtrOf(n, _, _) => {
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
                if let Some(t) = self.builtin_result_type(&method.name, receiver, args) {
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
            // `call?` is the payload of the `Result` / `Option` the call
            // produced — read it off the callee's registered result type,
            // so a command applied to an unwrapped value (`box -> Lid(2)?
            // -> Lid(3)?`) still finds its receiver's type.
            Expr::Try { inner, .. } => match self.callee_info(inner)?.result_ty {
                Ty::NamedPtrOf(_, ok, _) => Some(ok),
                _ => None,
            },
            _ => self.infer_static_type_name(expr),
        }
    }

    /// The member a call expression resolves to: a message application,
    /// a method on the receiver's type or alias chain, a constructor
    /// family member selected by its first argument's type, or a free
    /// function. `None` for builtins and anything not statically typed.
    pub(super) fn callee_info(&self, expr: &Expr) -> Option<FuncInfo> {
        match expr {
            Expr::MethodCall {
                receiver, method, ..
            } => {
                let recv = self.infer_ctor_arg_type_name(receiver)?;
                if let Some((_, info)) = self.message_target(&recv, &method.name) {
                    return Some(info);
                }
                self.dispatch_candidates(&recv)
                    .into_iter()
                    .find_map(|c| {
                        self.func_table
                            .get(&(Some(c), method.name.clone()))
                            .cloned()
                    })
                    .or_else(|| self.func_table.get(&(None, method.name.clone())).cloned())
            }
            Expr::Constructor { name, args, .. } => {
                let first = match args.as_slice() {
                    [Expr::ProductValue { fields, .. }] => fields.first(),
                    [first, ..] => Some(first),
                    [] => None,
                };
                match first {
                    Some(arg) => {
                        let arg_ty = self.infer_ctor_arg_type_name(arg)?;
                        self.dispatch_candidates(&arg_ty).into_iter().find_map(|c| {
                            self.func_table.get(&(Some(c), name.name.clone())).cloned()
                        })
                    }
                    None => self
                        .func_table
                        .get(&(None, name.name.clone()))
                        .or_else(|| {
                            self.func_table
                                .get(&(Some(name.name.clone()), "Self".to_string()))
                        })
                        .cloned(),
                }
            }
            _ => None,
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
            Expr::IntLit { .. } => Some("Int".to_string()),
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
        // Keep the payload's type on the option so `?` reads it back in
        // its own shape (`Some(todo)?` is a `Todo` pointer, not an i64).
        match Self::payload_type_name(&payload_ty) {
            Some(name) => Ty::NamedPtrOf("Option".to_string(), name.clone(), name),
            None => Ty::NamedPtr("Option".to_string()),
        }
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

    /// Wraps a value that already *is* a variant's payload in its union
    /// struct — the `payload -> Union` pipe. `build_union_value` compiles
    /// the variant's individual *fields* from the argument list, so a
    /// boxed product variant (two or more fields, stored behind one
    /// pointer) can't go through it: the payload here is that pointer
    /// already, and it stores straight through. Every other payload shape
    /// is a single value the argument path handles as it stands.
    pub(super) fn inject_union_variant(
        &mut self,
        union_name: &str,
        variant: &str,
        payload: &Expr,
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let tag = self.variant_tag[variant];
        let total = self.union_total_size(union_name);
        if self.product_field_layout(variant).len() < 2 {
            let args = std::slice::from_ref(payload);
            return self.build_union_value(union_name, variant, tag, total, args, scope, f);
        }
        let _ = self.compile_expr(payload, scope, f);
        f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
        f.instruction(&Instruction::I32Const(total as i32));
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
            offset: 4,
            align: 2,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        Ty::NamedPtr(union_name.to_string())
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

    /// Every type name a value of type `name` can be looked up under, in
    /// preference order. A declared param type may sit anywhere on the
    /// value's widening chain: the exact name, the variant's parent
    /// union (`True()` fills a `Bool` param), or a newtype's underlying
    /// type (`Port` fills an `Int` one).
    /// The command `message` applies to a value whose static type is
    /// `recv`, as `(receiver type, member)`: `("Map", Map * Insert => Map)`
    /// for `map -> Insert(…)`, found on the receiver or along its alias
    /// chain.
    pub(super) fn message_target(&self, recv: &str, message: &str) -> Option<(String, FuncInfo)> {
        self.dispatch_candidates(recv)
            .into_iter()
            .find_map(|candidate| {
                self.commands
                    .get(&(candidate.clone(), message.to_string()))
                    .map(|info| (candidate, info.clone()))
            })
    }

    pub(super) fn dispatch_candidates(&self, name: &str) -> Vec<String> {
        let mut out = vec![name.to_string()];
        if let Some(parent) = self.variant_parent.get(name) {
            out.push(parent.clone());
        }
        for link in self.collect_alias_chain(name) {
            if !out.contains(&link) {
                out.push(link);
            }
        }
        out
    }

    /// Order a call's inputs to match the callee's declared components.
    ///
    /// Commutative calling lets any component pipe in on the left of
    /// `->` (`docs/src/spec/functions.md`), so written order is not slot
    /// order: `Sep(124) -> Tail(text)` and `text -> Tail(Sep(124))` are
    /// the same call. Binding is by type, so the caller can compile the
    /// inputs in parameter order.
    ///
    /// Exact matches are tried first: one per component, first unused
    /// value wins. Failing that, a value may fill a component it
    /// *widens to* — `Tail = Tokens` means a `Tail` is a `Tokens`, so
    /// it fills a `Tokens` slot. Widening is one-directional: a
    /// `Tokens` does not fill a `Tail`, because the newtype is the
    /// narrower claim. On that pass a component matched by more than
    /// one value bails out entirely — written order deciding a slot is
    /// exactly what binding by type must not do, and a function's
    /// components are not distinct newtypes the way a product's fields
    /// are (`Int * OtherInt => Gt` binds either way round).
    ///
    /// Returns `None` when the callee's components aren't known
    /// one-per-input, when a component goes unmatched or ambiguous, or
    /// when the written order already is the declaration order.
    pub(super) fn commutative_order(
        &self,
        input_types: &[String],
        inputs: &[Expr],
    ) -> Option<Vec<Expr>> {
        if inputs.len() < 2 || input_types.len() != inputs.len() {
            return None;
        }
        let value_names: Vec<Option<String>> = inputs
            .iter()
            .map(|e| self.infer_ctor_arg_type_name(e))
            .collect();
        let order = self
            .assign_inputs(input_types, &value_names, Fit::Exact)
            .or_else(|| self.assign_inputs(input_types, &value_names, Fit::Widens))
            .or_else(|| self.assign_inputs(input_types, &value_names, Fit::Base))?;
        if order.iter().enumerate().all(|(i, &vi)| i == vi) {
            return None;
        }
        Some(order.into_iter().map(|vi| inputs[vi].clone()).collect())
    }

    /// Bind each declared component to an input index. `Fit::Exact`
    /// matches a value to its own type (or its union); `Fit::Widens`
    /// also to anything on its alias chain; `Fit::Base` also to a
    /// newtype of it — the untagged literal the checker lets fill a
    /// `Key = String` component (`map -> Contains("a")`). Past `Exact`,
    /// a component two values could fill fails the whole assignment.
    fn assign_inputs(
        &self,
        input_types: &[String],
        value_names: &[Option<String>],
        fit: Fit,
    ) -> Option<Vec<usize>> {
        let mut used = vec![false; value_names.len()];
        let mut order = Vec::with_capacity(input_types.len());
        for want in input_types {
            let candidates: Vec<usize> = (0..value_names.len())
                .filter(|&vi| {
                    !used[vi]
                        && value_names[vi].as_ref().is_some_and(|nm| {
                            self.field_match_score(nm, want) == 2
                                || (fit != Fit::Exact
                                    && self.collect_alias_chain(nm).iter().any(|n| n == want))
                                || (fit == Fit::Base
                                    && self.collect_alias_chain(want).iter().any(|n| n == nm))
                        })
                })
                .collect();
            let vi = *candidates.first()?;
            if fit != Fit::Exact && candidates.len() > 1 {
                return None;
            }
            used[vi] = true;
            order.push(vi);
        }
        Some(order)
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
            .map(|(_, repr, _)| repr_byte_size(repr))
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
            Ty::I32 | Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
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
            Ty::List => {
                // A list field is a `(ptr, len)` pair like a string's.
                f.instruction(&Instruction::LocalSet(scope.tmp_i32()));
                self.load_payload_at(scope.tmp_i32(), field_offset, &field_repr, f);
                Some(field_repr)
            }
            Ty::Unit => {
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
    ///   * `Ty::I64` / `Ty::F64` → one 8-byte scalar at offset 0.
    ///   * `Ty::I32`             → one i32 at offset 0, sign-extended.
    ///   * a heap pointer        → one i32 at offset 0, zero-extended.
    ///   * `Ty::Str`/`NamedStr`/`List` → i32 ptr at offset 0, i32 len at
    ///     offset 4.
    ///
    /// The fixed 8-byte stride lets the same `(ptr, len)` representation
    /// describe lists of any of the above types; downstream methods
    /// dispatch on a `Ty::List` receiver and read back according to the
    /// expected element shape (see `compile_builtin_method`).
    ///
    /// Every element is compiled before the list is allocated: an element
    /// that allocates (a product, a union, a nested list) clobbers the
    /// scratch locals, so the base pointer only goes into `alloc_ptr` once
    /// no user code is left to run. The elements wait on the operand stack
    /// and are stored last-to-first.
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
        let tys: Vec<Ty> = args
            .iter()
            .map(|arg| self.compile_expr(arg, scope, f))
            .collect();

        f.instruction(&Instruction::I32Const((n * 8) as i32));
        f.instruction(&Instruction::Call(self.fn_alloc));
        f.instruction(&Instruction::LocalSet(scope.alloc_ptr()));

        for (i, ty) in tys.iter().enumerate().rev() {
            let slot_offset = (i as u64) * 8;
            let mem64 = MemArg {
                offset: slot_offset,
                align: 3,
                memory_index: 0,
            };
            match ty {
                Ty::I64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(mem64));
                }
                Ty::F64 => {
                    f.instruction(&Instruction::LocalSet(scope.tmp_f64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_f64()));
                    f.instruction(&Instruction::F64Store(mem64));
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
                    f.instruction(&Instruction::I64Store(mem64));
                }
                Ty::Ptr | Ty::NamedPtr(_) | Ty::NamedPtrOf(_, _, _) => {
                    // A product / union / container value is one heap
                    // pointer: zero-extend it so the slot's upper half
                    // is defined, and store the whole 8 bytes.
                    f.instruction(&Instruction::I64ExtendI32U);
                    f.instruction(&Instruction::LocalSet(scope.tmp_i64()));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
                    f.instruction(&Instruction::I64Store(mem64));
                }
                Ty::Str | Ty::NamedStr(_) | Ty::List => {
                    // Stack: [ptr, len]. Stash len, then ptr, then store
                    // them at offset+0 and offset+4 of the slot.
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // len
                    f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // ptr
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset,
                        align: 2,
                        memory_index: 0,
                    }));
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
                    f.instruction(&Instruction::I32Store(MemArg {
                        offset: slot_offset + 4,
                        align: 2,
                        memory_index: 0,
                    }));
                }
                Ty::Unit => {
                    // Zero the slot so a later read doesn't see
                    // uninitialised heap bytes.
                    f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
                    f.instruction(&Instruction::I64Const(0));
                    f.instruction(&Instruction::I64Store(mem64));
                }
            }
        }

        f.instruction(&Instruction::LocalGet(scope.alloc_ptr()));
        f.instruction(&Instruction::I32Const(n as i32));
        Ty::List
    }
}

/// How loosely `assign_inputs` lets a value fill a declared component.
#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Exact,
    Widens,
    Base,
}
