//! Web-target codegen: the Elm-style `init`/`update`/`view` triple lowered
//! to a plain core module (the JS host `canon-web.js` is the wrapper).
//! See docs/src/reference/web-target.md.
use super::*;

/// Flat core shape of a web app's model value (what the user's `init`
/// returns), used by the export wrappers to normalize the model to
/// the single opaque i64 the JS host threads through `update`/`view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WebModelShape {
    /// `Int`-aliased model — already i64.
    I64,
    /// `Float`-aliased model — reinterpreted to i64.
    F64,
    /// Product/union/option model — a heap pointer, zero-extended.
    Ptr,
    /// `String`-aliased model — (ptr, len) boxed into an 8-byte cell.
    Str,
}

/// Builds the self-contained web-app core module (the web target, docs/src/reference/web-target.md).
/// Unlike the CLI/HTTP worlds this is a plain core module, not a
/// component — browsers instantiate core wasm directly and the
/// bundled JS host (`canon-web.js`) is the "component wrapper".
pub fn generate_web_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new_web(module);
    gen.compile_web()
}

impl<'m> WasmGen<'m> {
    /// Constructor for web encoder mode (the web target, docs/src/reference/web-target.md). The browser
    /// host implements only the stdout print stubs, so any extern
    /// import is a hard error at this stage.
    pub(super) fn new_web(ast: &'m OModule) -> Self {
        let mut gen = Self::new(ast);
        if !gen.extern_imports.is_empty() {
            let names: Vec<String> = gen
                .extern_imports
                .iter()
                .map(|e| e.full_path.clone())
                .collect();
            eprintln!(
                "error: web-app programs can't use extern imports yet: the browser host \
                 implements only the print surface. Found: {}. (Extending the web host's \
                 import surface is not yet implemented.)",
                names.join(", ")
            );
            std::process::exit(1);
        }
        // Defined functions start right after the five stdout imports.
        // The three export wrappers (init/update/view) take the slots
        // after `alloc`; `fn_start` doubles as the init-wrapper index.
        gen.fn_print_str = WEB_BASE_DEFINED;
        gen.fn_alloc = WEB_BASE_DEFINED + 1;
        gen.fn_start = WEB_BASE_DEFINED + 2; // init wrapper; update/view at +3/+4
        gen.fn_list_to_json_array = WEB_BASE_DEFINED + 5;
        gen.fn_str_cmp = WEB_BASE_DEFINED + 6;
        gen.fn_list_append = WEB_BASE_DEFINED + 7;
        gen.fn_list_concat = WEB_BASE_DEFINED + 8;
        gen.fn_user_start = WEB_BASE_DEFINED + 9;
        gen.fn_waitable_set_new = u32::MAX;
        gen.fn_waitable_join = u32::MAX;
        gen.fn_waitable_set_wait = u32::MAX;
        gen.fn_waitable_set_drop = u32::MAX;
        gen.fn_subtask_drop = u32::MAX;
        gen.fn_task_return = u32::MAX;
        gen.fn_subtask_cancel = u32::MAX;
        gen
    }

    /// Web encoder mode (the web target, docs/src/reference/web-target.md): emits a self-contained core
    /// module (own memory, own bump global) exporting the Elm-triple
    /// ABI the bundled JS host (`canon-web.js`) drives:
    ///
    ///   init()                       -> i64        opaque model
    ///   update(model, msg_ptr, len)  -> i64        msg is UTF-8 in guest memory
    ///   view(model)                  -> (i32, i32) UTF-8 HTML (ptr, len)
    ///   alloc(size)                  -> i32        lets JS place the msg bytes
    ///   memory
    ///
    /// The model is whatever the user's `init` returns, normalized to
    /// one opaque i64 the host threads back into `update`/`view` (see
    /// `WebModelShape`). The only imports are the five stdout print
    /// intrinsics, which the JS host maps onto `console.log` — so
    /// `.print()` debugging works in the browser console.
    pub(super) fn compile_web(&mut self) -> Vec<u8> {
        // Pre-passes — identical to `compile()`.
        self.build_type_defs();
        self.build_variant_info();
        self.collect_all_strings();
        self.assign_func_indices();
        // invariant: the checker only selects the web encoder mode when the
        // Elm triple (`Model => Html` view + `init`/`update`) is present.
        let web = crate::ast::find_web_entry(&self.ast.items)
            .expect("checker guarantees a web entry exists");
        let model = web.model;

        // Dynamic type registrations shared with `compile()` plus the
        // three wrapper shapes.
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
        let ty_init_wrapper = self.get_or_add_wasm_type(&[], &[ValType::I64]);
        let ty_update_wrapper =
            self.get_or_add_wasm_type(&[ValType::I64, ValType::I32, ValType::I32], &[ValType::I64]);
        let ty_view_wrapper =
            self.get_or_add_wasm_type(&[ValType::I64], &[ValType::I32, ValType::I32]);

        // The entry triple's compiled indices and the model's flat
        // core shape (from `init`'s result signature).
        let init_info = self
            .func_table
            .get(&web.init)
            .cloned()
            // invariant: `find_web_entry` returns func-table keys registered by
            // `assign_func_indices`, so the triple's entries are always present.
            .expect("web entry `init` missing from func table");
        let update_info = self
            .func_table
            .get(&web.update)
            .cloned()
            // invariant: see `init` above — `find_web_entry` keys are registered.
            .expect("web entry `update` missing from func table");
        let view_info = self
            .func_table
            .get(&web.view)
            .cloned()
            // invariant: see `init` above — `find_web_entry` keys are registered.
            .expect("web entry `view` missing from func table");
        let sig_of = |type_idx: u32| -> &(Vec<ValType>, Vec<ValType>) {
            &self.user_type_sigs[(type_idx - TY_USER_START) as usize]
        };
        let init_results = sig_of(init_info.type_idx).1.clone();
        let model_shape = match init_results.as_slice() {
            [ValType::I64] => WebModelShape::I64,
            [ValType::F64] => WebModelShape::F64,
            [ValType::I32] => WebModelShape::Ptr,
            [ValType::I32, ValType::I32] => WebModelShape::Str,
            other => {
                eprintln!(
                    "error: unsupported web model shape {other:?}: the model must be a \
                     product, union, Int, Float, or String-aliased type"
                );
                std::process::exit(1);
            }
        };
        let model_flat: &[ValType] = match model_shape {
            WebModelShape::I64 => &[ValType::I64],
            WebModelShape::F64 => &[ValType::F64],
            WebModelShape::Ptr => &[ValType::I32],
            WebModelShape::Str => &[ValType::I32, ValType::I32],
        };
        let (update_params, update_results) = sig_of(update_info.type_idx).clone();
        let expected_update: Vec<ValType> = model_flat
            .iter()
            .chain(&[ValType::I32, ValType::I32])
            .cloned()
            .collect();
        if update_params != expected_update || update_results != init_results {
            eprintln!(
                "error: web entry shape mismatch: `update` must be \
                 `({model} * String) -> {model}` with the same model type `init` returns"
            );
            std::process::exit(1);
        }
        let (view_params, view_results) = sig_of(view_info.type_idx).clone();
        if view_params != model_flat || view_results != [ValType::I32, ValType::I32] {
            eprintln!(
                "error: web entry shape mismatch: `view` must be `({model}) -> Html` \
                 with the same model type `init` returns"
            );
            std::process::exit(1);
        }

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

        // ── Import section: the five stdout print intrinsics only ────
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
        m.section(&imports);

        // ── Function section — order matches the WEB_BASE_DEFINED map ─
        let mut funcs = FunctionSection::new();
        funcs.function(TY_PRINT_STR);
        funcs.function(TY_ALLOC);
        funcs.function(ty_init_wrapper);
        funcs.function(ty_update_wrapper);
        funcs.function(ty_view_wrapper);
        funcs.function(list_to_json_array_ty);
        funcs.function(str_cmp_ty);
        funcs.function(list_append_ty);
        funcs.function(list_concat_ty);
        for (_, type_idx, _) in &self.compiled_user_funcs {
            funcs.function(*type_idx);
        }
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

        // ── Exports — the JS-host ABI ────────────────────────────────
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("alloc", ExportKind::Func, self.fn_alloc);
        exports.export("init", ExportKind::Func, self.fn_start);
        exports.export("update", ExportKind::Func, self.fn_start + 1);
        exports.export("view", ExportKind::Func, self.fn_start + 2);
        m.section(&exports);

        // ── Code section — order must match the function section ─────
        let mut codes = CodeSection::new();
        codes.function(&self.build_print_str());
        codes.function(&self.build_alloc());
        codes.function(&self.build_web_init_wrapper(init_info.func_idx, model_shape));
        codes.function(&self.build_web_update_wrapper(update_info.func_idx, model_shape));
        codes.function(&self.build_web_view_wrapper(view_info.func_idx, model_shape));
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

    /// Normalize the model value(s) on the stack into the opaque i64
    /// handed to the JS host. `base` is the index of three scratch i32
    /// locals the caller declared (used only for the `Str` boxing).
    pub(super) fn emit_web_model_wrap(&self, f: &mut Function, shape: WebModelShape, base: u32) {
        let mem = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        match shape {
            WebModelShape::I64 => {}
            WebModelShape::Ptr => {
                f.instruction(&Instruction::I64ExtendI32U);
            }
            WebModelShape::F64 => {
                f.instruction(&Instruction::I64ReinterpretF64);
            }
            WebModelShape::Str => {
                // [ptr, len] → box into a fresh 8-byte cell.
                f.instruction(&Instruction::LocalSet(base + 1)); // len
                f.instruction(&Instruction::LocalSet(base)); // ptr
                f.instruction(&Instruction::I32Const(8));
                f.instruction(&Instruction::Call(self.fn_alloc));
                f.instruction(&Instruction::LocalTee(base + 2));
                f.instruction(&Instruction::LocalGet(base));
                f.instruction(&Instruction::I32Store(mem));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::LocalGet(base + 1));
                f.instruction(&Instruction::I32Store(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I64ExtendI32U);
            }
        }
    }

    /// Push the model back in its user-function shape from the opaque
    /// i64 in local `model_local`. `base` as in `emit_web_model_wrap`.
    pub(super) fn emit_web_model_unwrap(
        &self,
        f: &mut Function,
        shape: WebModelShape,
        model_local: u32,
        base: u32,
    ) {
        match shape {
            WebModelShape::I64 => {
                f.instruction(&Instruction::LocalGet(model_local));
            }
            WebModelShape::Ptr => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::I32WrapI64);
            }
            WebModelShape::F64 => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::F64ReinterpretI64);
            }
            WebModelShape::Str => {
                f.instruction(&Instruction::LocalGet(model_local));
                f.instruction(&Instruction::I32WrapI64);
                f.instruction(&Instruction::LocalSet(base + 2));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                f.instruction(&Instruction::LocalGet(base + 2));
                f.instruction(&Instruction::I32Load(MemArg {
                    offset: 4,
                    align: 2,
                    memory_index: 0,
                }));
            }
        }
    }

    /// `init() -> i64` — call the user's `init`, normalize the model.
    pub(super) fn build_web_init_wrapper(&self, init_idx: u32, shape: WebModelShape) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 0..2 (no params)
        f.instruction(&Instruction::Call(init_idx));
        self.emit_web_model_wrap(&mut f, shape, 0);
        f.instruction(&Instruction::End);
        f
    }

    /// `update(model: i64, msg_ptr: i32, msg_len: i32) -> i64`.
    pub(super) fn build_web_update_wrapper(
        &self,
        update_idx: u32,
        shape: WebModelShape,
    ) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 3..5 after params
        self.emit_web_model_unwrap(&mut f, shape, 0, 3);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::Call(update_idx));
        self.emit_web_model_wrap(&mut f, shape, 3);
        f.instruction(&Instruction::End);
        f
    }

    /// `view(model: i64) -> (i32, i32)` — UTF-8 HTML (ptr, len).
    pub(super) fn build_web_view_wrapper(&self, view_idx: u32, shape: WebModelShape) -> Function {
        let mut f = Function::new([(3, ValType::I32)]); // locals 1..3 after the param
        self.emit_web_model_unwrap(&mut f, shape, 0, 1);
        f.instruction(&Instruction::Call(view_idx));
        f.instruction(&Instruction::End);
        f
    }
}
