//! HTTP-target codegen: the `Request => Response` handler world.
//! Emits a self-contained core module (own memory, bump global,
//! `cabi_realloc`) whose entry lifts as `wasi:http/handler#handle`.
//! See docs/src/reference/web-target.md for the sibling web path.
use super::*;

/// Builds the self-contained HTTP core module for
/// `component::wrap_http_service`.
pub(super) fn generate_http_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new_http(module);
    gen.compile_http()
}

impl<'m> WasmGen<'m> {
    /// Constructor for HTTP encoder mode. The `wasi:http/types`
    /// binding declarations from `canon/std/http` are consumed by the
    /// mode's own constructor special-cases, not the generic extern
    /// machinery, so they're filtered out here. Any *other* extern
    /// import can't be satisfied by the `wasi:http/service` world and
    /// is a hard error at this stage of the migration — the checker
    /// mirrors this rejection (`GAP_HTTP_WORLD_IMPORTS`), so a checked
    /// program never reaches it; this exit backstops unchecked callers.
    pub(super) fn new_http(ast: &'m OModule) -> Self {
        let mut gen = Self::new(ast);
        gen.http_mode = true;
        gen.extern_imports.retain(|ext| {
            !crate::ast::HTTP_WORLD_IMPORT_PREFIXES
                .iter()
                .any(|p| ext.component_namespace.starts_with(p))
        });
        if !gen.extern_imports.is_empty() {
            let names: Vec<String> = gen
                .extern_imports
                .iter()
                .map(|e| e.full_path.clone())
                .collect();
            eprintln!(
                "error: HTTP handler programs can only import `wasi:http/types` for now: \
                 found extern imports the `wasi:http/service` world can't satisfy: {}. \
                 (Lifting the remaining WASI surface into HTTP handlers is not yet implemented.)",
                names.join(", ")
            );
            std::process::exit(1);
        }
        // No extern or waitable imports in this mode: defined functions
        // start right after the fixed import block. Poison the waitable
        // indices so an accidental call fails validation loudly instead
        // of silently calling the wrong import.
        gen.fn_print_str = HTTP_BASE_DEFINED;
        gen.fn_alloc = HTTP_BASE_DEFINED + 1;
        gen.fn_start = HTTP_BASE_DEFINED + 2; // the `handle` wrapper slot
        gen.fn_list_to_json_array = HTTP_BASE_DEFINED + 3;
        gen.fn_str_cmp = HTTP_BASE_DEFINED + 4;
        gen.fn_list_append = HTTP_BASE_DEFINED + 5;
        gen.fn_list_concat = HTTP_BASE_DEFINED + 6;
        gen.fn_user_start = HTTP_BASE_DEFINED + 7;
        gen.fn_waitable_set_new = u32::MAX;
        gen.fn_waitable_join = u32::MAX;
        gen.fn_waitable_set_wait = u32::MAX;
        gen.fn_waitable_set_drop = u32::MAX;
        gen.fn_subtask_drop = u32::MAX;
        gen.fn_task_return = u32::MAX;
        gen.fn_subtask_cancel = u32::MAX;
        gen
    }

    /// HTTP mode: compile `Response(Body * Headers * Status)` (or the
    /// body-less `Response(Headers * Status)`) into the
    /// `wasi:http/types` construction sequence.
    ///
    /// Construction happens in two phases because the `handle` export
    /// is async-stackful: everything that *creates* handles runs here,
    /// inside the user function, but the body/trailer *writes* are
    /// sync canonical-ABI calls that block until the host consumes
    /// them — they can only run after `task.return` has delivered the
    /// response. So this function stashes the write-phase state
    /// (contents writer + body bytes + trailers writer) at fixed
    /// memory addresses, and `build_http_handle_wrapper` performs the
    /// writes after `task.return`.
    /// Does `e` construct the HTTP component named `name` (`Headers` /
    /// `Status`)? Matched by static type where inference succeeds, and
    /// by the chain's *syntactic base* otherwise — a builder chain like
    /// `Headers().set(…)` whose `.set` returns `Unit` breaks type
    /// inference, so we walk the receivers back to the `Headers()`
    /// constructor.
    pub(super) fn is_http_component(&self, e: &Expr, name: &str) -> bool {
        if let Some(t) = self.infer_ctor_arg_type_name(e) {
            if self.widening_chain(&t).iter().any(|n| n == name) {
                return true;
            }
        }
        match e {
            Expr::Constructor { name: n, .. } => n.name == name,
            Expr::MethodCall {
                receiver, method, ..
            } => method.name == name || self.is_http_component(receiver, name),
            _ => false,
        }
    }

    pub(super) fn build_http_response(
        &mut self,
        args: &[Expr],
        scope: &LocalScope,
        f: &mut Function,
    ) -> Ty {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        // Normalise `Response(a * b * c)` (one `ProductValue`) to the
        // component list.
        let exprs: Vec<Expr> = match args {
            [Expr::ProductValue { fields, .. }] => fields.clone(),
            _ => args.to_vec(),
        };
        let has_body = exprs.len() >= 3;
        // Positionless: pick each component by its type, not its slot.
        // `Headers()` and `Status(n)` name themselves; the body is
        // whatever remains. This keeps a formatter-sorted
        // `Response(Headers() * NotFound() * Status(404))` correct even
        // though the body (`NotFound()`) no longer sits first. Falls
        // back to declaration order when a component's type can't be
        // inferred statically.
        let headers_i = exprs
            .iter()
            .position(|e| self.is_http_component(e, "Headers"));
        let status_i = exprs
            .iter()
            .position(|e| self.is_http_component(e, "Status"));
        let (body_expr, headers_expr, status_expr) = match (headers_i, status_i) {
            (Some(hi), Some(si)) => {
                let body = if has_body {
                    (0..exprs.len())
                        .find(|&i| i != hi && i != si)
                        .map(|i| &exprs[i])
                } else {
                    None
                };
                (body, Some(&exprs[hi]), Some(&exprs[si]))
            }
            _ if has_body => (exprs.first(), exprs.get(1), exprs.get(2)),
            _ => (None, exprs.first(), exprs.get(1)),
        };

        // ── Phase 1: user expressions (parked on the operand stack —
        // each may be arbitrary user code, so nothing can live in
        // scratch locals until all three are compiled). ──────────────
        if let Some(e) = body_expr {
            let ty = self.compile_expr(e, scope, f);
            if !ty.is_str_like() {
                // Wrong shape — degrade to an empty body.
                self.drop_value(ty, f);
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::I32Const(0));
            }
        }
        match headers_expr {
            Some(e) => {
                let ty = self.compile_expr(e, scope, f);
                if !matches!(ty, Ty::I32 | Ty::Ptr | Ty::NamedPtr(_)) {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
                }
            }
            None => {
                f.instruction(&Instruction::Call(FN_HTTP_FIELDS_CTOR));
            }
        }
        match status_expr {
            Some(e) => {
                let ty = self.compile_expr(e, scope, f);
                if matches!(ty, Ty::I64) {
                    f.instruction(&Instruction::I32WrapI64);
                } else {
                    self.drop_value(ty, f);
                    f.instruction(&Instruction::I32Const(200));
                }
            }
            None => {
                f.instruction(&Instruction::I32Const(200));
            }
        }
        // Peel into locals — no user code from here on.
        f.instruction(&Instruction::LocalSet(scope.tmp_i32_b())); // status
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // headers
        if has_body {
            // [bptr, blen] → fixed body slots.
            f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // blen
            f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // bptr
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_PTR as i32));
            f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
            f.instruction(&Instruction::I32Store(mem));
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_LEN as i32));
            f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
            f.instruction(&Instruction::I32Store(mem));
        }

        // ── Phase 2: handle creation ─────────────────────────────────
        // Trailers future — reader (low 32) goes to response.new,
        // writer (high 32) to the fixed slot for the post-return write.
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_NEW));
        f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(scope.addr_scratch())); // trailers reader
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
        f.instruction(&Instruction::I64Const(32));
        f.instruction(&Instruction::I64ShrU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Store(mem));

        // Contents stream (only with a body): reader to response.new,
        // writer to the fixed slot. Without a body the slot holds 0
        // and the wrapper skips the write.
        if has_body {
            f.instruction(&Instruction::Call(FN_HTTP_STREAM_NEW));
            f.instruction(&Instruction::LocalTee(scope.tmp_i64()));
            f.instruction(&Instruction::I32WrapI64);
            f.instruction(&Instruction::LocalSet(scope.map_elem_ptr())); // contents reader
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
            f.instruction(&Instruction::LocalGet(scope.tmp_i64()));
            f.instruction(&Instruction::I64Const(32));
            f.instruction(&Instruction::I64ShrU);
            f.instruction(&Instruction::I32WrapI64);
            f.instruction(&Instruction::I32Store(mem));
        } else {
            f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
            f.instruction(&Instruction::I32Const(0));
            f.instruction(&Instruction::I32Store(mem));
        }

        // response.new(headers, contents, trailers-reader, ret)
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        if has_body {
            f.instruction(&Instruction::I32Const(1)); // option<stream>: some
            f.instruction(&Instruction::LocalGet(scope.map_elem_ptr()));
        } else {
            f.instruction(&Instruction::I32Const(0)); // none
            f.instruction(&Instruction::I32Const(0));
        }
        f.instruction(&Instruction::LocalGet(scope.addr_scratch()));
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
        f.instruction(&Instruction::Call(FN_HTTP_RESPONSE_NEW));

        // Unpack tuple<response, future>; drop the transmission future.
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::LocalSet(scope.tmp_i32())); // response
        f.instruction(&Instruction::I32Const(MEM_HTTP_RET as i32 + 4));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_DROP_READABLE));

        // Apply the status (response.new defaults to 200); the bare
        // `result` discriminant is dropped — a rejected code leaves
        // the default, which is the sane degradation.
        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        f.instruction(&Instruction::LocalGet(scope.tmp_i32_b()));
        f.instruction(&Instruction::Call(FN_HTTP_SET_STATUS));
        f.instruction(&Instruction::Drop);

        f.instruction(&Instruction::LocalGet(scope.tmp_i32()));
        Ty::NamedPtr("Response".to_string())
    }

    /// HTTP mode: the core export behind
    /// `[async-lift-stackful]wasi:http/handler@…#handle`. Core
    /// signature `(request: i32) -> ()` — the result is delivered via
    /// `[task-return]handle` mid-function, after which the task keeps
    /// running to perform the blocking body/trailer writes:
    ///
    ///   1. call the user's `(Request) -> Response` function,
    ///   2. drop the request handle (introspection is slice 2),
    ///   3. `task.return(ok(response))` — the host starts sending,
    ///   4. sync `stream.write` of the body bytes (blocks until the
    ///      host consumes), then `stream.drop-writable` (ends the
    ///      body),
    ///   5. sync `future.write` of `ok(none)` trailers, then
    ///      `future.drop-writable`.
    ///
    /// Step 4/5 state comes from the fixed memory slots
    /// `build_http_response` filled. No handles leak.
    pub(super) fn build_http_handle_wrapper(&self, user_fn_idx: u32) -> Function {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        let mut f = Function::new([(1, ValType::I32)]); // local 1: response
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(user_fn_idx));
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(FN_HTTP_REQUEST_DROP));

        // task.return(ok(response)) — `result<own<response>, error-code>`
        // lowered flat as the joined slots of both arms; the ok arm
        // uses (disc = 0, handle) and pads the six error-code slots.
        f.instruction(&Instruction::I32Const(0)); // ok
        f.instruction(&Instruction::LocalGet(1)); // response handle
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Call(FN_HTTP_TASK_RETURN));

        // ── Post-return: body ────────────────────────────────────────
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_PTR as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_LEN as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_STREAM_WRITE));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::I32Const(MEM_HTTP_BODY_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_STREAM_DROP_WRITABLE));
        f.instruction(&Instruction::End);

        // ── Post-return: trailers (`ok(none)` — zero bytes) ──────────
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_ZERO as i32));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_WRITE));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::I32Const(MEM_HTTP_TRAILERS_WRITER as i32));
        f.instruction(&Instruction::I32Load(mem));
        f.instruction(&Instruction::Call(FN_HTTP_FUTURE_DROP_WRITABLE));

        f.instruction(&Instruction::End);
        f
    }

    /// HTTP mode: the module owns its allocator, so `cabi_realloc` is
    /// defined here (same bump global `$alloc` uses) instead of living
    /// in the component wrapper's memory-provider module.
    /// `old_ptr`/`old_size` are ignored — one-pass bump, never frees.
    pub(super) fn build_cabi_realloc(&self) -> Function {
        let mut f = Function::new([(1, ValType::I32)]); // local 4: aligned
                                                        // aligned = (bump + align - 1) & -align
        f.instruction(&Instruction::GlobalGet(0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalTee(4));
        // bump = aligned + new_size
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::GlobalSet(0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::End);
        f
    }

    /// HTTP encoder mode counterpart of `compile()`.
    ///
    /// Emits a *self-contained* core module for
    /// `wit_component::ComponentEncoder`: own memory + bump global +
    /// exported `cabi_realloc`, imports named per `wit-component`'s
    /// mangling conventions (stdout intrinsics hang off
    /// `write-via-stream`, http intrinsics off `[static]response.new`),
    /// and the entry exported as `wasi:http/handler@…#handle`. The
    /// hand-rolled component path (`compile()` + `component::wrap`)
    /// stays in place for the CLI world.
    pub(super) fn compile_http(&mut self) -> Vec<u8> {
        use crate::ast::{entry_world_of, EntryWorld};

        // Pre-passes — identical to `compile()`.
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();
        // Dynamic type registrations shared with `compile()` plus the
        // two http-specific shapes.
        let ty_i32x2_to_i32 =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32]);
        let list_to_json_array_ty =
            self.get_or_add_wasm_type(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
        let str_cmp_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);
        let list_append_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I64],
            &[ValType::I32; 2],
        );
        let list_concat_ty = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32; 2]);
        let _list_map_loop_ty = self.get_or_add_wasm_type(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32, ValType::I32],
        );
        let ty_response_new = self.get_or_add_wasm_type(&[ValType::I32; 5], &[]);
        let ty_fields_append = self.get_or_add_wasm_type(&[ValType::I32; 6], &[]);
        let ty_task_return_handle = self.get_or_add_wasm_type(
            &[
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I64,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            &[],
        );
        let ty_cabi_realloc = self.get_or_add_wasm_type(&[ValType::I32; 4], &[ValType::I32]);

        // The entry function: the free `(Request) -> Response` the
        // checker validated. Its compiled index feeds the wrapper.
        let entry_name = self
            .ast
            .items
            .iter()
            .find_map(|item| match item {
                Item::Function(func)
                    if func.receiver.is_none()
                        && entry_world_of(&func.return_ty) == Some(EntryWorld::Http) =>
                {
                    Some(func.name.name.clone())
                }
                _ => None,
            })
            // invariant: the checker only selects the HTTP encoder mode when a
            // `Request => Response` entry is present, so one exists here.
            .expect("checker guarantees an HTTP entry exists");
        let user_fn_idx = self
            .func_table
            .get(&(None, entry_name.clone()))
            .map(|info| info.func_idx)
            .unwrap_or_else(|| panic!("HTTP entry `{entry_name}` missing from func table"));

        let mut m = Module::new();

        // ── Type section — same fixed TY_* prefix as `compile()` ─────
        let mut types = TypeSection::new();
        types.ty().function([ValType::I32, ValType::I32], []); // 0
        types.ty().function([ValType::I32], []); // 1
        types.ty().function([], []); // 2
        types.ty().function([ValType::I32], [ValType::I32]); // 3
        types.ty().function([ValType::I32], [ValType::I32]); // 4
        types.ty().function([], [ValType::I64]); // 5
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]); // 6
        types.ty().function([], [ValType::I32]); // 7
        let user_sigs: Vec<_> = self.user_type_sigs.clone();
        for (params, results) in &user_sigs {
            types
                .ty()
                .function(params.iter().cloned(), results.iter().cloned());
        }
        m.section(&types);

        // ── Import section (indices fixed — see FN_HTTP_*) ───────────
        const STDOUT_MODULE: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";
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
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            STDOUT_MODULE,
            "[future-drop-readable-1]write-via-stream",
            EntityType::Function(TY_PRINT_BOOL),
        );
        let http = component::WASI_HTTP_TYPES_MODULE;
        imports.import(
            http,
            "[constructor]fields",
            EntityType::Function(TY_HANDLE_RETURN),
        );
        imports.import(
            http,
            "[static]response.new",
            EntityType::Function(ty_response_new),
        );
        imports.import(
            http,
            "[future-new-1][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            http,
            "[future-write-1][static]response.new",
            EntityType::Function(ty_i32x2_to_i32),
        );
        imports.import(
            http,
            "[future-drop-readable-2][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[future-drop-writable-1][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[resource-drop]request",
            EntityType::Function(TY_PRINT_BOOL),
        );
        imports.import(
            http,
            "[method]response.set-status-code",
            EntityType::Function(ty_i32x2_to_i32),
        );
        imports.import(
            http,
            "[stream-new-0][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_NEW),
        );
        imports.import(
            http,
            "[stream-write-0][static]response.new",
            EntityType::Function(TY_STDOUT_STREAM_WRITE),
        );
        imports.import(
            http,
            "[stream-drop-writable-0][static]response.new",
            EntityType::Function(TY_PRINT_BOOL),
        );
        // task.return for the async-stackful `handle` lift. The
        // `result<own<response>, error-code>` result lowers flat to
        // the *joined* slots of both arms:
        // (disc, own-handle/err-disc, then error-code's joined payload
        // slots i32,i64,i32,i32,i32,i32). The ok arm only uses the
        // first two; the rest are padding zeros.
        imports.import(
            "[export]wasi:http/handler@0.3.0-rc-2026-03-15",
            "[task-return]handle",
            EntityType::Function(ty_task_return_handle),
        );
        imports.import(
            http,
            "[method]request.get-path-with-query",
            EntityType::Function(TY_PRINT_STR),
        );
        imports.import(
            http,
            "[method]fields.append",
            EntityType::Function(ty_fields_append),
        );
        imports.import(
            http,
            "[method]request.get-method",
            EntityType::Function(TY_PRINT_STR),
        );
        m.section(&imports);

        // ── Function section ─────────────────────────────────────────
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_ALLOC);
        funcs.function(TY_PRINT_BOOL); // fn_start slot = handle wrapper, (i32) -> ()
        funcs.function(list_to_json_array_ty);
        funcs.function(str_cmp_ty);
        funcs.function(list_append_ty);
        funcs.function(list_concat_ty);
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
        funcs.function(ty_cabi_realloc); // cabi_realloc, appended last
        m.section(&funcs);

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

        // ── Exports ──────────────────────────────────────────────────
        let cabi_realloc_idx = self.fn_user_start + self.compiled_user_funcs.len() as u32;
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("cabi_realloc", ExportKind::Func, cabi_realloc_idx);
        exports.export(
            &format!(
                "[async-lift-stackful]{}#handle",
                component::WASI_HTTP_HANDLER
            ),
            ExportKind::Func,
            self.fn_start,
        );
        m.section(&exports);

        // ── Code section — order must match the function section ─────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_alloc());
        codes.function(&self.build_http_handle_wrapper(user_fn_idx));
        codes.function(&self.build_list_to_json_array());
        codes.function(&self.build_str_cmp());
        codes.function(&self.build_list_append());
        codes.function(&self.build_list_concat());
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
        m.section(&codes);

        // ── Data ─────────────────────────────────────────────────────
        let mut data = DataSection::new();
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
