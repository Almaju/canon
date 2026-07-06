/// Canon WASM codegen — emits a core module which is then wrapped into a
/// **Component Model** component (WASI Preview 3) by `component::wrap`.
///
/// The core module:
///   - Imports its linear memory from `"env" "memory"`
///     (provided by a tiny memory-only core module instantiated by the wrapper).
///   - Imports five canonical-ABI builtins from `"wasi:cli/stdout"` —
///     `write-via-stream`, `stream-new`, `stream-write`,
///     `stream-drop-writable`, and `future-drop-readable`. `print_str`
///     stitches them into the native WASI P3 stdout sequence so the
///     produced `.wasm` is portable to any compliant Component Model
///     runtime (no `canon:*` host bridge required for output).
///   - Exports `"run" (func (result i32))` — the entry point that the wrapper
///     lifts as `wasi:cli/run.run`. The i32 result is the canonical-ABI
///     discriminant for `result<_, _>`: 0 = Ok, 1 = Err.
///
/// Memory layout (shared with the host via the lowered import):
///   [0  .. 16]  reserved (was fd_write scratch in the WASI P1 era; kept for
///               alignment but unused now)
///   [16 .. 32]  int-to-string buffer (grows ← from 32)
///   [32]        '\n' byte (appended to int prints)
///   [64 ..   ]  string literal data (UTF-8, packed)
///   [65536 .. ] bump heap (grows → for union/product/list values)
use std::collections::HashMap;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction, MemArg,
    MemorySection, MemoryType, Module, TypeSection, ValType,
};

use crate::ast::{
    ArmLiteral, Block, Expr, FunctionDef, Item, MatchArm, Module as OModule, TypeExpr,
};

mod compile;
mod component;
mod extern_imports;
mod http;
mod literals;
mod strings;
mod ty;
mod web;

use extern_imports::{
    classify_return, collect_extern_imports, is_self_ctor, ExternImport, IndirectReturnShape,
    ParamKind,
};
use http::generate_http_core_module;
use strings::{extra_locals_decl, FuncInfo, LocalScope, StringTable};
use ty::*;
use web::generate_web_core_module;

// ── Memory constants ──────────────────────────────────────────────────────────
const MEM_INT_BUF_END: u32 = 32; // '\n' lives at this byte
const MEM_STR_START: u32 = 64;
pub(super) const MEM_HEAP_START: u32 = 65536; // bump heap begins at second page

// ── Function index constants ──────────────────────────────────────────────
// The imports section starts with the five `wasi:cli/stdout` canonical
// builtins at indices 0..4, followed by every `extern Wasm` declaration
// from the user program (sorted alphabetically by
// `interface@version#fn-name`), followed by the async-runtime waitable
// intrinsics. Compiled functions start right after that block, so their
// indices depend on how many extern imports the program has.
// `WasmGen` populates the dynamic offsets below in `new()`.
//
// `print_str` stitches these five into the canonical-ABI sequence for
// writing a byte buffer to stdout (see `build_print_str`).
const FN_STDOUT_WRITE_VIA_STREAM: u32 = 0; // (i32) -> i32
const FN_STDOUT_STREAM_NEW: u32 = 1; // () -> i64
const FN_STDOUT_STREAM_WRITE: u32 = 2; // (i32, i32, i32) -> i32
const FN_STDOUT_STREAM_DROP_WRITABLE: u32 = 3; // (i32) -> ()
const FN_STDOUT_FUTURE_DROP_READABLE: u32 = 4; // (i32) -> ()
const FIRST_EXTERN_IMPORT_FN: u32 = 5; // first index of a user `extern Wasm` import

// ── HTTP-mode import indices ─────────────────────────────────────────
// In HTTP encoder mode (`http_mode`, see `compile_http`) the import
// space is fixed: the five stdout builtins keep indices 0..4 (under
// `wit-component` naming conventions), then the `wasi:http/types`
// functions/intrinsics plus the task-return intrinsic for the
// async-stackful `handle` lift. No extern-Wasm or waitable imports
// exist in this mode; defined functions start at `HTTP_BASE_DEFINED`.
const FN_HTTP_FIELDS_CTOR: u32 = 5; // [constructor]fields          () -> i32
const FN_HTTP_RESPONSE_NEW: u32 = 6; // [static]response.new        (i32 x5) -> ()
const FN_HTTP_FUTURE_NEW: u32 = 7; // [future-new-1][static]response.new  () -> i64
const FN_HTTP_FUTURE_WRITE: u32 = 8; // [future-write-1]… (sync)    (i32,i32) -> i32
const FN_HTTP_FUTURE_DROP_READABLE: u32 = 9; // [future-drop-readable-2]…  (i32) -> ()
const FN_HTTP_FUTURE_DROP_WRITABLE: u32 = 10; // [future-drop-writable-1]… (i32) -> ()
const FN_HTTP_REQUEST_DROP: u32 = 11; // [resource-drop]request      (i32) -> ()
const FN_HTTP_SET_STATUS: u32 = 12; // [method]response.set-status-code (i32,i32) -> i32
const FN_HTTP_STREAM_NEW: u32 = 13; // [stream-new-0][static]response.new () -> i64
const FN_HTTP_STREAM_WRITE: u32 = 14; // [stream-write-0]… (sync)   (i32,i32,i32) -> i32
const FN_HTTP_STREAM_DROP_WRITABLE: u32 = 15; // [stream-drop-writable-0]… (i32) -> ()
const FN_HTTP_TASK_RETURN: u32 = 16; // [task-return]handle
const FN_HTTP_GET_PATH: u32 = 17; // [method]request.get-path-with-query (i32,i32) -> ()
const FN_HTTP_FIELDS_APPEND: u32 = 18; // [method]fields.append (i32 x6) -> ()
const FN_HTTP_GET_METHOD: u32 = 19; // [method]request.get-method (i32,i32) -> ()
const HTTP_BASE_DEFINED: u32 = 20;

// ── Web-mode import indices ──────────────────────────────────────────
// In web encoder mode (`compile_web`, see the web target, docs/src/reference/web-target.md) the import
// space is just the five stdout builtins at 0..4 — the bundled JS host
// (`canon-web.js`) stubs them onto `console.log`. Defined functions
// start right after.
const WEB_BASE_DEFINED: u32 = 5;

// Fixed scratch addresses used by HTTP-mode response construction.
// `build_http_response` runs *inside* the user function; the sync
// body/trailer writes must happen *after* `task.return` hands the
// response to the host (they block until the host consumes them), so
// construction stashes the write-phase state at fixed addresses for
// the `handle` wrapper to pick up. Addresses 0..16 are otherwise
// unused (the int buffer is 16..32, '\n' at 32, strings from 64) and
// no user code runs between the stores and the loads.
const MEM_HTTP_BODY_WRITER: u32 = 0; // contents-stream writer (0 = no body)
const MEM_HTTP_BODY_PTR: u32 = 4;
const MEM_HTTP_BODY_LEN: u32 = 8;
const MEM_HTTP_TRAILERS_WRITER: u32 = 12;
const MEM_HTTP_RET: u32 = 16; // response.new tuple ret (int-buffer reuse is safe)
const MEM_HTTP_TRAILERS_ZERO: u32 = 40; // `ok(none)` — all zero bytes, never written

// ── Type index constants (pre-defined) ──────────────────────────────
const TY_PRINT_STR: u32 = 0; // (i32,i32) → ()  — print_str body / waitable.join
const TY_PRINT_INT: u32 = 1; // (i64) → ()
const TY_PRINT_BOOL: u32 = 2; // (i32) → ()  — also stdout-builtin (i32) -> () slot
                              // `run` is lifted as an *async stackful* function at the component level so
                              // nested calls to `extern Wasm.async` can suspend on `waitable-set.wait`
                              // without tripping wasmtime's "cannot block a synchronous task" check.
                              // The async-stackful lift delivers the result via `task.return(…)` rather
                              // than the function's wasm return value, so `run`'s core signature is
                              // `() -> ()`.
const TY_RUN: u32 = 3; // () → ()
const TY_ALLOC: u32 = 4; // (i32) → (i32)
                         // WASI stdout canonical-ABI builtin signatures (declared after the
                         // pre-existing TY_* slots so user types still start at a stable offset).
const TY_STDOUT_WRITE_VIA_STREAM: u32 = 5; // (i32) → (i32)
const TY_STDOUT_STREAM_NEW: u32 = 6; // () → (i64)
const TY_STDOUT_STREAM_WRITE: u32 = 7; // (i32, i32, i32) → (i32)
                                       // `waitable-set.new` needs `() -> i32`. `TY_RUN` used to fit but is now
                                       // `() -> ()` because `run` is lifted as an *async-stackful* function
                                       // (result delivered via `task.return`).
const TY_HANDLE_RETURN: u32 = 8; // () → (i32)
const TY_USER_START: u32 = 9; // first dynamic user type

// ── Extern Wasm path parsing ─────────────────────────────────────────────────────

// ── Global index constants ──────────────────────────────────────────────────────────
// The bump pointer is now an *imported* mutable global so it can be shared
// between the user core module and the component wrapper's `cabi_realloc`
// helper. Both bump from the same pointer, which keeps Canon-allocated heap
// data and host-allocated string returns in a single coherent heap.
const GLOBAL_BUMP_PTR: u32 = 0;

// ── WASM representation of a Canon expression ──────────────────────────────

struct WasmGen<'m> {
    ast: &'m OModule,
    strings: StringTable,

    // Type definitions: name → body TypeExpr (from AST TypeDef items)
    type_defs: HashMap<String, TypeExpr>,

    // For each union type: sorted list of variant names
    union_variants: HashMap<String, Vec<String>>, // union_name → [variant1, variant2, ...]

    // For each variant name: which union it belongs to
    variant_parent: HashMap<String, String>, // variant → union_type_name

    // For each variant name: its discriminant tag (alphabetical order within the union)
    variant_tag: HashMap<String, u32>,

    // User function table: (Option<receiver_type_name>, method_name) → FuncInfo
    func_table: HashMap<(Option<String>, String), FuncInfo>,

    // Every compiled user function in func-index order: (func_idx,
    // type_idx, def). The single source of truth for the emitted
    // function and code sections — `func_table` is a *lookup* structure
    // whose keys can collide (constructor families register several
    // bodies for one type name; the commutative aliases point several
    // keys at one body), so deriving section contents from it lets the
    // function-section and code-section lengths drift apart, which is
    // invalid wasm. This list cannot: one entry per compiled body.
    compiled_user_funcs: Vec<(u32, u32, FunctionDef)>,

    // WASM type deduplication
    user_type_sigs: Vec<(Vec<ValType>, Vec<ValType>)>, // index 0 → TY_USER_START
    user_type_map: HashMap<(Vec<ValType>, Vec<ValType>), u32>, // → absolute type idx

    /// User `extern Wasm` declarations, in the order they appear in the
    /// emitted core module's import section (sorted by `core_namespace`, then
    /// `fn_name`, so the order is deterministic).
    extern_imports: Vec<ExternImport>,

    // Dynamic function indices in the core module's index space. These are
    // computed in `new()` once `extern_imports.len()` is known. After the
    // imports block (host.print + N externs + 5 waitable intrinsics),
    // defined functions follow at index `1 + N + 5`.
    //
    // The waitable intrinsics implement the canonical-ABI async-wait
    // sequence emitted by `emit_async_call` for the not-Returned status
    // path. They're imported as `canon:async/waitable.<name>` (a
    // compiler-synthesised module-import name); `component::wrap` builds
    // a synthetic core instance from the canon section that exports the
    // matching functions. They're imported unconditionally so the import
    // section is shape-stable regardless of program content.
    //
    // `task.return` is grouped here too — it's needed by `run`'s async
    // stackful lift to deliver the `result<_, _>` value (since async
    // lift bodies don't return values directly).
    fn_waitable_set_new: u32,  // `()         -> i32`
    fn_waitable_join: u32,     // `(i32, i32) -> ()`   (waitable, set)
    fn_waitable_set_wait: u32, // `(i32, i32) -> i32`  (set, payload-area) -> event-code
    fn_waitable_set_drop: u32, // `(i32)      -> ()`   (set)
    fn_subtask_drop: u32,      // `(i32)      -> ()`   (subtask)
    fn_task_return: u32,       // `(i32)      -> ()`   (discriminant for result<_,_>)
    fn_subtask_cancel: u32,    // `(i32)      -> i32`  (subtask) -> new state
    //                                                  Used by `compile_race` to
    //                                                  abandon the loser. The
    //                                                  i32 result is the new
    //                                                  CallState; we drop it.
    fn_print_str: u32,
    fn_print_int: u32,
    fn_print_bool: u32,
    fn_alloc: u32,
    fn_start: u32, // exported as "run"
    /// Helper that converts a `List<String>` (list of pre-encoded JSON
    /// values) into a single `Json` string, joining elements with `,`
    /// and wrapping with `[`/`]`. Always emitted — unused programs pay
    /// a few hundred bytes of dead code, which is acceptable for now.
    /// Core signature: `(list_ptr: i32, list_len: i32) -> (i32, i32)`.
    /// See `build_list_to_json_array` for the body.
    fn_list_to_json_array: u32,
    /// Formats and prints an `f64` (fixed-point, up to 6 fraction
    /// digits, trailing zeros trimmed; `NaN` / `Inf` / `-Inf` for the
    /// specials). Core signature: `(f64) -> ()`. See
    /// `build_print_float`.
    fn_print_float: u32,
    /// Renders an `i64` as its decimal string in a fresh heap
    /// allocation — the value half of `String(Int)` / `Int.String()`
    /// (conversion-is-construction, the language spec (docs/src/spec/)). Same
    /// digit loop as `build_print_int` but the bytes are copied out of
    /// the shared int buffer into an `$alloc` block so later renders
    /// can't clobber the result. Core signature: `(i64) -> (i32, i32)`.
    fn_int_to_str: u32,
    /// Byte-wise lexicographic string compare returning -1/0/1 —
    /// backs `String.lt/le/gt/ge/ne` (and the alphabetical-order rule
    /// the language is built on). Core signature:
    /// `(ptr1, len1, ptr2, len2) -> i32`.
    fn_str_cmp: u32,
    /// `(list_ptr, count, slot: i64) -> (ptr, count)` — fresh list
    /// with `slot` appended. The call site packs the element into the
    /// 8-byte slot (i64 as-is; strings as `ptr | len << 32`, matching
    /// the `build_list_literal` layout).
    fn_list_append: u32,
    /// `(ptr1, count1, ptr2, count2) -> (ptr, count)` — fresh list
    /// with the second list's slots after the first's.
    fn_list_concat: u32,
    fn_user_start: u32,
    /// `Some("Result")` / `Some("Option")` while compiling the body
    /// of a function whose declared return type is that shape (one
    /// i32 pointer at the core level). Gates `?`'s early return: an
    /// `Err`/`None` propagates unchanged when the inner and enclosing
    /// kinds match (both tag 0 at offset 0); in any other context
    /// (e.g. `main`), `?` extracts unconditionally as before.
    cur_fn_early_return: Option<&'static str>,
    /// HTTP encoder mode: the module is self-contained (own memory,
    /// own bump global, exported `cabi_realloc`), imports follow
    /// `wit-component` naming conventions, and the entry export is
    /// `wasi:http/handler@…#handle` instead of `run`. Constructors
    /// `Headers()` / `Response(…)` compile to `wasi:http/types` calls.
    http_mode: bool,
}

impl<'m> WasmGen<'m> {
    fn new(ast: &'m OModule) -> Self {
        let extern_imports = collect_extern_imports(ast);
        let n_externs = extern_imports.len() as u32;
        // Function-index layout after `(1 host.print + N externs)`:
        //   waitable intrinsics (5)
        //   defined functions (print_str, print_int, print_bool, alloc,
        //                       start/run, user functions...)
        // After 5 stdout canonical-builtin imports (FN_STDOUT_*) at
        // indices 0..4 and N extern Wasm imports at 5..5+N, the next
        // block is the 6 waitable+task intrinsics, then the defined
        // functions follow.
        let base_waitable = FIRST_EXTERN_IMPORT_FN + n_externs; // = 5 + N
        let base_defined = base_waitable + 7; // skip the 7 waitable+task imports
                                              // (set-new, join, set-wait,
                                              //  set-drop, subtask-drop,
                                              //  task-return, subtask-cancel)
        WasmGen {
            ast,
            strings: StringTable::new(),
            type_defs: HashMap::new(),
            union_variants: HashMap::new(),
            variant_parent: HashMap::new(),
            variant_tag: HashMap::new(),
            func_table: HashMap::new(),
            compiled_user_funcs: Vec::new(),
            user_type_sigs: Vec::new(),
            user_type_map: HashMap::new(),

            extern_imports,
            fn_waitable_set_new: base_waitable,
            fn_waitable_join: base_waitable + 1,
            fn_waitable_set_wait: base_waitable + 2,
            fn_waitable_set_drop: base_waitable + 3,
            fn_subtask_drop: base_waitable + 4,
            fn_task_return: base_waitable + 5,
            fn_subtask_cancel: base_waitable + 6,
            fn_print_str: base_defined,
            fn_print_int: base_defined + 1,
            fn_print_bool: base_defined + 2,
            fn_alloc: base_defined + 3,
            fn_start: base_defined + 4,
            fn_list_to_json_array: base_defined + 5,
            fn_print_float: base_defined + 6,
            fn_int_to_str: base_defined + 7,
            fn_str_cmp: base_defined + 8,
            fn_list_append: base_defined + 9,
            fn_list_concat: base_defined + 10,
            fn_user_start: base_defined + 11,
            cur_fn_early_return: None,
            http_mode: false,
        }
    }

    fn build_type_defs(&mut self) {
        for item in self.ast.items.iter() {
            if let Item::TypeDef(td) = item {
                self.type_defs.insert(td.name.name.clone(), td.body.clone());
            }
        }
    }

    fn build_variant_info(&mut self) {
        for (name, body) in &self.type_defs {
            if let TypeExpr::Union { variants, .. } = body {
                let mut names: Vec<String> = variants
                    .iter()
                    .filter_map(|v| {
                        if let TypeExpr::Named { name, .. } = v {
                            Some(name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                names.sort();
                for (tag, variant_name) in names.iter().enumerate() {
                    self.variant_parent
                        .insert(variant_name.clone(), name.clone());
                    self.variant_tag.insert(variant_name.clone(), tag as u32);
                }
                self.union_variants.insert(name.clone(), names);
            }
        }

        // Built-in: Bool = False + True
        self.union_variants.insert(
            "Bool".to_string(),
            vec!["False".to_string(), "True".to_string()],
        );
        self.variant_parent
            .insert("False".to_string(), "Bool".to_string());
        self.variant_parent
            .insert("True".to_string(), "Bool".to_string());
        self.variant_tag.insert("False".to_string(), 0);
        self.variant_tag.insert("True".to_string(), 1);

        // Built-in: Option = None + Some (alphabetical)
        self.union_variants.insert(
            "Option".to_string(),
            vec!["None".to_string(), "Some".to_string()],
        );
        self.variant_parent
            .insert("None".to_string(), "Option".to_string());
        self.variant_parent
            .insert("Some".to_string(), "Option".to_string());
        self.variant_tag.insert("None".to_string(), 0);
        self.variant_tag.insert("Some".to_string(), 1);

        // Built-in: Result = Err + Ok (alphabetical)
        self.union_variants.insert(
            "Result".to_string(),
            vec!["Err".to_string(), "Ok".to_string()],
        );
        self.variant_parent
            .insert("Err".to_string(), "Result".to_string());
        self.variant_parent
            .insert("Ok".to_string(), "Result".to_string());
        self.variant_tag.insert("Err".to_string(), 0);
        self.variant_tag.insert("Ok".to_string(), 1);

        // Newtype aliases of unions inherit their variant set so that
        // dispatching on a value whose static type is the alias resolves
        // correctly. E.g. `MessageContent = Option<Content>` is
        // registered with the same variants as `Option`, letting
        // `someMessageContent.(None => ..., Some<Content> => ...)`
        // compile through the same `emit_union_dispatch` path as a raw
        // `Option`.
        //
        // The alias chain is walked through `type_defs` until we hit a
        // name that's already in `union_variants` (either a user union
        // or a builtin like `Option`/`Result`/`Bool`). The bound of 20
        // hops guards against a malformed cyclic alias.
        let alias_defs: Vec<(String, String)> = self
            .type_defs
            .iter()
            .filter_map(|(alias_name, body)| {
                if let TypeExpr::Named { name: target, .. } = body {
                    Some((alias_name.clone(), target.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (alias_name, initial_target) in alias_defs {
            if self.union_variants.contains_key(&alias_name) {
                continue; // already a union itself, not an alias
            }
            let mut current = initial_target;
            for _ in 0..20 {
                if let Some(variants) = self.union_variants.get(&current) {
                    let variants = variants.clone();
                    self.union_variants.insert(alias_name.clone(), variants);
                    break;
                }
                match self.type_defs.get(&current) {
                    Some(TypeExpr::Named { name: next, .. }) => current = next.clone(),
                    _ => break,
                }
            }
        }
    }

    fn collect_all_strings(&mut self) {
        self.strings.intern("False");
        self.strings.intern("True");
        for item in self.ast.items.iter() {
            if let Item::Function(f) = item {
                self.collect_strings_block(&f.body);
            }
        }
    }

    fn collect_strings_block(&mut self, block: &Block) {
        for expr in &block.exprs {
            self.collect_strings_expr(expr);
        }
    }
    fn collect_strings_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::StringLit { value, .. } => {
                // Literals are stored *without* a trailing newline. `.print`
                // on a string emits its own `\n` (see `emit_print`), giving
                // host-returned strings and literals identical output.
                self.strings.intern(value);
            }
            Expr::JsonLit { parts, .. } => {
                // Intern every Static fragment so the `compile_expr`
                // path — which lowers a mixed JsonLit to a concat
                // chain over synthesized `StringLit`s — finds each
                // fragment in the intern table. Recurse into Interp
                // expressions so their strings are also interned.
                for p in parts {
                    match p {
                        crate::ast::JsonLitPart::Static(s) => {
                            self.strings.intern(s);
                        }
                        crate::ast::JsonLitPart::Interp(e) => self.collect_strings_expr(e),
                    }
                }
            }
            Expr::HtmlLit { parts, .. } => {
                // Same as JsonLit: pre-intern Static fragments for the
                // concat-chain lowering, recurse into interpolations.
                for p in parts {
                    match p {
                        crate::ast::HtmlLitPart::Static(s) => {
                            self.strings.intern(s);
                        }
                        crate::ast::HtmlLitPart::Interp(e) => self.collect_strings_expr(e),
                    }
                }
            }
            Expr::FieldAccess { receiver, .. } => self.collect_strings_expr(receiver),
            Expr::MethodCall { receiver, args, .. } => {
                self.collect_strings_expr(receiver);
                for a in args {
                    self.collect_strings_expr(a);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.collect_strings_expr(scrutinee);
                for arm in arms {
                    self.collect_strings_block(&arm.body);
                }
            }
            Expr::Lambda { body, .. } => self.collect_strings_block(body),
            Expr::Constructor { args, .. } => {
                for a in args {
                    self.collect_strings_expr(a);
                }
            }
            Expr::ProductValue { fields, .. } => {
                for f in fields {
                    self.collect_strings_expr(f);
                }
            }
            Expr::Try { inner, .. } => self.collect_strings_expr(inner),
            _ => {}
        }
    }

    fn assign_func_indices(&mut self) {
        // 1. Extern Wasm imports: register them in the func table so method
        //    calls find them. Their core function indices were already
        //    assigned by `collect_extern_imports`.
        let extern_imports = self.extern_imports.clone();
        for ext in &extern_imports {
            // Always register the core wasm type so the import section can
            // look it up later. For async externs this is the
            // `(flat_params, ret_ptr?) -> i32` async-lower shape (per
            // wasmparser's `Abi::LowerAsync`); for sync externs it's the
            // flat-scalar / indirect-return shape computed by
            // `collect_extern_imports`.
            let type_idx = self.get_or_add_wasm_type(&ext.params, &ext.results);
            // Find the AST function so we can pull its receiver type and
            // Canon return type — needed for proper method dispatch.
            let Some(func) = self.ast.items.iter().find_map(|item| {
                if let Item::Function(f) = item {
                    if let Some(e) = &f.extern_wasm {
                        if e.path == ext.full_path {
                            return Some(f);
                        }
                    }
                }
                None
            }) else {
                continue;
            };
            let result_ty = self.resolve_return_ty(func);
            let key = (
                func.receiver.as_ref().map(|r| r.name.clone()),
                func.name.name.clone(),
            );
            // The Canon-side result type depends on the indirect-return
            // shape: a bare `String` return is `Ty::Str`, while a
            // `Result<Ok, Err>` (both string-aliased) becomes
            // `Ty::NamedPtrStr("Result", ok_name, err_name)` so `?` and
            // dispatch arms can extract the string payload with the right
            // Canon-level type on either branch.
            let surface_result_ty = match &ext.indirect_return {
                Some(IndirectReturnShape::String) => {
                    // Preserve any String-alias name (e.g. `HttpServer`,
                    // `Now`, `Url`) so subsequent method dispatch finds
                    // the right key. `resolve_return_ty` already wraps
                    // String-aliased types as `Ty::NamedStr(name)`.
                    match &result_ty {
                        Ty::NamedStr(_) => result_ty.clone(),
                        _ => Ty::Str,
                    }
                }
                Some(IndirectReturnShape::ResultStringString { ok_name, err_name }) => {
                    Ty::NamedPtrStr("Result".to_string(), ok_name.clone(), err_name.clone())
                }
                Some(IndirectReturnShape::OptionString) => Ty::NamedPtrStr(
                    "Option".to_string(),
                    "String".to_string(),
                    "String".to_string(),
                ),
                Some(IndirectReturnShape::ListString) => Ty::List,
                Some(IndirectReturnShape::ScalarRecord { product, .. }) => {
                    Ty::NamedPtr(product.clone())
                }
                None => result_ty,
            };
            let info = FuncInfo {
                func_idx: ext.func_idx,
                type_idx,
                result_ty: surface_result_ty,
                narrow_params: ext.narrow_params.clone(),
                narrow_result_signed: ext.narrow_result_signed,
                indirect_return: ext.indirect_return.clone(),
                is_async: ext.is_async,
            };
            self.func_table.insert(key, info.clone());

            // Self-renamed constructors (parsed from `Name = (P) -> Name` or
            // `Name = (P) -> Result<Name, _>`) are also registered
            // commutatively under `(P, Name)` so the user can write either
            // `Name(p)` (constructor style) or `p.Name()` (method style).
            // The codegen call-site handling for both routes through
            // `emit_func_table_call`, so a single `FuncInfo` suffices.
            if is_self_ctor(func) {
                if let Some(first_param) = func.params.first() {
                    if let TypeExpr::Named {
                        name: param_name, ..
                    } = &first_param.ty
                    {
                        let recv_name = func
                            .receiver
                            .as_ref()
                            .map(|r| r.name.clone())
                            .unwrap_or_default();
                        let commutative_key = (Some(param_name.clone()), recv_name);
                        self.func_table.entry(commutative_key).or_insert(info);
                    }
                }
            }
        }

        // 2. Compiled user functions: each gets the next available index.
        let mut idx = self.fn_user_start;
        for item in self.ast.items.iter() {
            if let Item::Function(func) = item {
                // Skip main (inlined into $start)
                if func.name.name == "main" && func.receiver.is_none() {
                    continue;
                }
                // Skip extern wasm declarations (handled above)
                if func.extern_wasm.is_some() {
                    continue;
                }
                // Skip trait type defs (Function-typed bodies)
                if let TypeExpr::Function { .. } = &func.return_ty { /* but still compile */ }

                let params = self.func_wasm_params(func);
                let results = self.func_wasm_results(func);
                let type_idx = self.get_or_add_wasm_type(&params, &results);
                // Surface result type: classify `Result<String-aliased,
                // String-aliased>` returns the same way as externs so
                // `?` and dispatch arms can extract string payloads via
                // the `Ty::NamedPtrStr` path. The function body itself
                // returns an i32 pointer (via `build_result_ok` /
                // `build_result_err`) whose memory layout matches the
                // extern indirect-return area (tag at +0, ptr at +4,
                // len at +8), so no calling-convention change is needed
                // — only the type label.
                let result_ty = match classify_return(&func.return_ty, &results, &self.type_defs) {
                    Some(IndirectReturnShape::ResultStringString { ok_name, err_name }) => {
                        Ty::NamedPtrStr("Result".to_string(), ok_name, err_name)
                    }
                    // `Option<String-alias>` bodies keep the payload's
                    // string-ness in the surface type so `?` extracts a
                    // (ptr, len) pair instead of misreading the slot as
                    // one i64. Dispatch is unaffected — it keys on the
                    // container name, exactly like the Result case.
                    Some(IndirectReturnShape::OptionString) => {
                        let payload = match &func.return_ty {
                            TypeExpr::Named { generics, .. } if !generics.is_empty() => {
                                named_type_name(&generics[0])
                                    .unwrap_or_else(|| "String".to_string())
                            }
                            _ => "String".to_string(),
                        };
                        Ty::NamedPtrStr("Option".to_string(), payload.clone(), payload)
                    }
                    _ => self.resolve_return_ty(func),
                };

                let key = (
                    func.receiver.as_ref().map(|r| r.name.clone()),
                    func.name.name.clone(),
                );
                let info = FuncInfo {
                    func_idx: idx,
                    type_idx,
                    result_ty,
                    narrow_params: Vec::new(),
                    narrow_result_signed: None,
                    indirect_return: None,
                    is_async: false,
                };
                if is_self_ctor(func) {
                    // Constructor families: several `Self`-renamed bodies
                    // share the `(Type, "Self")` primary key. The zero-arg
                    // member owns it (it's what a bare `Type()` call
                    // dispatches through); parameterized members are
                    // reached via the per-param commutative keys below,
                    // so they only fill the primary slot when nothing
                    // else has.
                    if func.params.is_empty() {
                        self.func_table.insert(key, info.clone());
                    } else {
                        self.func_table.entry(key).or_insert_with(|| info.clone());
                    }
                } else {
                    self.func_table.insert(key, info.clone());
                }

                // Self-ctor commutative registration (mirrors the extern
                // block above). After `resolve_new_syntax`, a function
                // declared as `Name = (P) -> R<Name, E>` is rewritten to
                // `Self = (P) -> ...` with receiver `Name`. We also need
                // to make it reachable from `p.Name()` — the natural
                // method-call form on a value of type `P` — by adding
                // an alias entry under `(Some(P), Name)`. Without this,
                // body-defined validating constructors (`Json = (String)
                // -> Result<Json, MalformedJson> { … }`) fall through
                // to the type-newtype constructor path in
                // `compile_constructor`, silently dropping the body.
                // Every param component registers (not just the first):
                // commutative calling lets any component be the
                // receiver, and for a constructor family the per-param
                // keys are what keep members distinct — the checker
                // guards that no two members collide on a component
                // type.
                if is_self_ctor(func) {
                    let recv_name = func
                        .receiver
                        .as_ref()
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    let mut components: Vec<String> = Vec::new();
                    for param in &func.params {
                        match &param.ty {
                            TypeExpr::Named {
                                name: param_name, ..
                            } => components.push(param_name.clone()),
                            TypeExpr::Product { fields, .. } => {
                                for field in fields {
                                    if let TypeExpr::Named {
                                        name: field_name, ..
                                    } = field
                                    {
                                        components.push(field_name.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    for param_name in components {
                        let commutative_key = (Some(param_name), recv_name.clone());
                        self.func_table
                            .entry(commutative_key)
                            .or_insert_with(|| info.clone());
                    }
                }
                self.compiled_user_funcs.push((idx, type_idx, func.clone()));
                idx += 1;
            }
        }
    }

    // ── Type system ───────────────────────────────────────────────────────────

    fn get_or_add_wasm_type(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        let key = (params.to_vec(), results.to_vec());
        if let Some(&idx) = self.user_type_map.get(&key) {
            return idx;
        }
        let idx = TY_USER_START + self.user_type_sigs.len() as u32;
        self.user_type_sigs
            .push((params.to_vec(), results.to_vec()));
        self.user_type_map.insert(key, idx);
        idx
    }
}

// ── Validation ─────────────────────────────────────────────────────────────────
pub(super) fn validate(bytes: &[u8]) {
    use wasmparser::{Parser, Validator, WasmFeatures};
    let mut v = Validator::new_with_features(WasmFeatures::all());
    for payload in Parser::new(0).parse_all(bytes) {
        let p = match payload {
            Ok(p) => p,
            Err(e) => {
                eprintln!("internal error: generated invalid wasm (parse): {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = v.payload(&p) {
            eprintln!("internal error: generated invalid wasm (validate): {e}");
            std::process::exit(1);
        }
    }
}

/// Emits the raw core WASM module — used by the Component Model wrapper.
fn generate_core_module(module: &OModule) -> Vec<u8> {
    let mut gen = WasmGen::new(module);
    gen.compile()
}

/// Returns whether the program has a free function returning `Response`
/// (or `Result<Response, _>`), per the entry-point rule
/// (docs/src/spec/functions.md). When true, codegen routes through
/// `component::wrap_http_service` instead of the CLI path.
fn has_http_entry(module: &OModule) -> bool {
    use crate::ast::{entry_world_of, EntryWorld};
    module.items.iter().any(|item| match item {
        Item::Function(func) => {
            func.receiver.is_none() && entry_world_of(&func.return_ty) == Some(EntryWorld::Http)
        }
        _ => false,
    })
}

/// Builds the final Component Model component (`.wasm` bytes).
///
/// The output is a WASI Preview 3 component that exports
/// `wasi:cli/run@0.3.0-rc-2026-03-15`, imports `wasi:cli/stdout@0.3.0-rc-2026-03-15`,
/// and additionally imports every interface referenced by an `extern Wasm`
/// declaration in the user program.
/// It is validated with `wasmparser` before being returned.
pub fn generate(module: &OModule) -> Vec<u8> {
    // Branch on the entry-point's world (see the entry-point rule,
    // docs/src/spec/functions.md). CLI entries flow through the existing
    // hand-rolled `wasm-encoder` pipeline; HTTP entries route to a
    // separate codegen path that delegates type-section emission to
    // `wit-component` (the resource + variant surface in
    // `wasi:http/types` is too large to maintain by hand).
    //
    // The checker has already validated which entry shape applies
    // (slice 1a); this dispatch is the authoritative entry-world router.
    if has_http_entry(module) {
        let bytes = component::wrap_http_service(module);
        validate(&bytes);
        return bytes;
    }

    // Web-app entries (the `init`/`update`/`view` triple) emit a raw
    // core module — the JS host is the wrapper. See the web target, docs/src/reference/web-target.md.
    if crate::ast::find_web_entry(&module.items).is_some() {
        let bytes = generate_web_core_module(module);
        validate(&bytes);
        return bytes;
    }

    let core = generate_core_module(module);
    let externs = collect_extern_imports(module);
    // Run the async-inference fixpoint so the component wrapper can
    // surface async metadata in the emitted WIT and — once async lowering
    // lands — attach `CanonicalOption::Async` to the right lifts/lowers.
    let async_set = crate::codegen::async_analysis::analyse(module);
    let bytes = component::wrap(&core, &externs, &async_set);
    validate(&bytes);
    bytes
}

/// Returns the WIT world description that accompanies the compiled `.wasm`.
pub fn generate_wit(module: &OModule) -> String {
    let async_set = crate::codegen::async_analysis::analyse(module);
    component::generate_wit(module, &async_set)
}
