use crate::ast::{
    extract_receiver_from_params, resolve_new_syntax, BindingsDecl, Block, Expr, ExternWasm,
    FunctionDef, Item, Module, Param, TypeExpr,
};
use crate::bindgen;
use crate::error::{CanonError, Result, Span};
use crate::install::{self, InstallIndex, INSTALL_INDEX_FILENAME};
use crate::lexer::Scanner;
use crate::manifest::{self, Manifest};
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Walk a slice of items and fill in the path string of any `extern Wasm`
/// declaration that was emitted without one.
///
/// `canon install` writes bindgen output with bare `extern Wasm` markers
/// (no URN string) and a sidecar `_install.toml` index mapping each file
/// to its WIT interface URN. The loader consults that index to fill in
/// the per-function path each function's codegen needs: `"<urn>#<fn>"`
/// where `<fn>` is the function's name camel-back-converted to the
/// kebab-case form the WIT source uses.
///
/// Externs that already have an explicit path (user-written one-off
/// `extern Wasm("…")` declarations) are left untouched.
fn patch_extern_paths(items: &mut [Item], urn: &str) {
    for item in items {
        if let Item::Function(f) = item {
            if let Some(ew) = f.extern_wasm.as_mut() {
                if ew.path.is_empty() {
                    ew.path = format!("{}#{}", urn, bindgen::camel_to_kebab(&f.name.name));
                }
            }
        }
    }
}

/// Walk `items` linearly and rewrite every function-type alias that
/// sits under a `bindings "…"` directive into a real `Item::Function`
/// with `extern_wasm` populated.
///
/// `bindings` directives come in two forms:
///   1. `bindings "<urn>"` (no `#fn` fragment) sets a *file-level base*
///      URN. Subsequent function-type aliases auto-derive their
///      canonical-ABI path as `"<urn>#<kebab(name)>"` until another
///      base-form directive replaces it. A file can mix several bases
///      (`url.can` does that for `canon:builtins/url` followed by
///      `canon:builtins/http`).
///   2. `bindings "<urn>#<fn-name>"` (explicit `#fn`) is a *one-shot*
///      override: it applies only to the very next function-type alias,
///      using `<urn>#<fn-name>` verbatim as the path. Used for cases
///      where the Canon name doesn't kebab-back to the WIT name —
///      e.g. `ToJson = (Bool) -> Json` is bound to `#from-bool`, not
///      `#to-json`. After the next alias is consumed, the previous
///      base-form (if any) is back in effect.
///
/// Rewrite rules for individual function-type aliases:
///   * Both camelCase *and* PascalCase aliases are rewritten under a
///     `bindings` directive. PascalCase callbacks (e.g.
///     `Handler = (Request) -> Response`) only stay as type aliases
///     when there is NO active `bindings` base or pending override.
///   * The first product component becomes the receiver for camelCase
///     declarations; PascalCase declarations and zero-arg functions
///     skip the receiver extraction.
///   * Async-ness is read off the return type: a function whose return
///     is `Future<T>` is async (the canonical-ABI lowering uses
///     `[async-lower]`); everything else is sync. No `async` keyword.
///     This keeps the source consistent with the principle "types tell
///     the story" — the function's effect is visible in its signature.
pub fn apply_bindings_directive(items: &mut [Item]) {
    let mut base_urn: Option<String> = None;
    let mut pending_override: Option<String> = None;

    for item in items.iter_mut() {
        // Bindings directives steer the rewriter for subsequent items.
        // A URN with `#` is an explicit one-shot path; without `#` it
        // becomes the new file-level base.
        if let Item::Bindings(BindingsDecl { urn, .. }) = item {
            if urn.contains('#') {
                pending_override = Some(urn.clone());
            } else {
                base_urn = Some(urn.clone());
                pending_override = None;
            }
            continue;
        }

        let Item::TypeDef(td) = item else { continue };
        let TypeExpr::Function {
            generic_params,
            params,
            return_ty,
            ..
        } = &td.body
        else {
            continue;
        };

        // Pick the URN for this declaration. A pending one-shot override
        // wins over the file-level base, and we consume it so the
        // following decl falls back to the base (if any).
        let path = if let Some(p) = pending_override.take() {
            p
        } else if let Some(base) = &base_urn {
            format!("{}#{}", base, bindgen::camel_to_kebab(&td.name.name))
        } else {
            // No bindings context in scope: leave the TypeDef as a
            // function-type alias (the existing Canon callback-type
            // syntax). This is the path user code outside any bindings
            // block follows.
            continue;
        };

        let new_params: Vec<Param> = params
            .iter()
            .map(|t| Param {
                ty: t.clone(),
                mutable: false,
                span: t.span(),
            })
            .collect();

        let starts_lower = td.name.name.chars().next().is_some_and(char::is_lowercase);
        let (receiver, recv_mut, final_params) = if !starts_lower || new_params.is_empty() {
            // PascalCase declarations (constructors, trait
            // overloads) and zero-arg functions don't take a
            // receiver — the parser does the same for non-bindings
            // FunctionDefs.
            (None, false, new_params)
        } else {
            extract_receiver_from_params(new_params)
        };

        let body_span = Span::new(td.span.end, td.span.end, td.span.line, td.span.column);
        // Async detection: a `Future<T>` return type marks the function
        // as async at the canonical-ABI level. We *unwrap* the Future
        // here so `func.return_ty` carries the WIT-canonical return
        // type `T` — the codegen consumes that for the canonical-ABI
        // signature, and `auto_await::collect_method_returns` re-wraps
        // when `is_async` is set so its static-type analysis still sees
        // `Future<T>` at call sites.
        let (return_ty_unwrapped, is_async) = match return_ty.as_ref() {
            TypeExpr::Named { name, generics, .. } if name == "Future" && generics.len() == 1 => {
                (generics[0].clone(), true)
            }
            other => (other.clone(), false),
        };
        let new_func = FunctionDef {
            receiver,
            receiver_mut: recv_mut,
            name: td.name.clone(),
            generic_params: generic_params.clone(),
            params: final_params,
            return_ty: return_ty_unwrapped,
            body: Block {
                exprs: Vec::new(),
                span: body_span,
            },
            extern_wasm: Some(ExternWasm { path, is_async }),
            span: td.span,
        };
        *item = Item::Function(new_func);
    }
}

// ---------------------------------------------------------------------------
// Bundled packages
// ---------------------------------------------------------------------------
//
// The shipped packages (`canon/std`, `canon/wasi`, …) are baked into the
// compiler binary at build time by `build.rs`, which walks `packages/` and
// emits a flat registry as `bundled_packages.rs`. The registry replaces what
// used to be hand-maintained `STDLIB` and `WASI_BINDINGS` arrays — drop a new
// file under `packages/<ns>/<pkg>/` and the next `cargo build` picks it up.

/// One package shipped with the compiler.
#[derive(Debug, Clone, Copy)]
pub struct BundledPackage {
    /// Canonical name, e.g. `"canon/std"`. Matches the package's
    /// declared `name` in its `canon.toml`.
    pub name: &'static str,
    /// The full `canon.toml` source, parsed lazily on first use.
    pub manifest_src: &'static str,
    /// Every `.can` file under the package root, sorted alphabetically by
    /// package-relative path.
    pub files: &'static [BundledFile],
}

/// One file inside a bundled package.
#[derive(Debug, Clone, Copy)]
pub struct BundledFile {
    /// Path relative to the package root, e.g. `"clocks/monotonic_clock.can"`.
    /// Always uses `/` separators so it matches `use` paths users write.
    pub path: &'static str,
    /// The file's source, embedded at build time via `include_str!`.
    pub source: &'static str,
    /// Build-time absolute path. Used by the LSP for go-to-definition when
    /// the binary is run against its source tree. For an installed binary
    /// this path won't exist on the user's filesystem; the LSP must cope
    /// (same caveat as the previous `CARGO_MANIFEST_DIR` baked-in path).
    pub abs_path: &'static str,
    /// WIT interface URN of the form `"<ns>:<pkg>/<iface>@<version>"`,
    /// or `None` for hand-written files. Populated at build time by
    /// `build.rs` from the package's `bindgen/_install.toml`. The
    /// loader uses this to patch the path string of bare `extern Wasm`
    /// declarations (i.e. ones the bindgen emitted without an explicit
    /// URN) to the form `"<urn>#<fn-kebab>"` that codegen expects.
    pub wit_urn: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/bundled_packages.rs"));

/// Find a bundled package by its canonical name (`"canon/std"`).
pub fn bundled_package(name: &str) -> Option<&'static BundledPackage> {
    BUNDLED_PACKAGES.iter().find(|p| p.name == name)
}

/// Find a specific file inside a bundled package.
pub fn bundled_file(pkg: &BundledPackage, rel_path: &str) -> Option<&'static BundledFile> {
    pkg.files.iter().find(|f| f.path == rel_path)
}

/// Resolve a `use a/b/c/…/Z` path against the bundled packages.
///
/// Returns the matching file plus the owning package, or `None` if no
/// bundled package's name is a prefix of `use_path`.
///
/// The matching rule mirrors what the local-file loader does for
/// directories: walk the trailing segments as a path within the package,
/// kebab-casing the final type-name segment to find its `.can` file.
pub fn resolve_bundled_use(
    use_path: &str,
) -> Option<(&'static BundledPackage, &'static BundledFile)> {
    // Two-segment package prefix: `<namespace>/<package>`.
    let segments: Vec<&str> = use_path.split('/').collect();
    if segments.len() < 3 {
        // Need at least `<ns>/<pkg>/<something>` to be a package import.
        return None;
    }
    let package_name = format!("{}/{}", segments[0], segments[1]);
    let pkg = bundled_package(&package_name)?;

    // Walk the remaining segments. Intermediate ones are directory names
    // (kept as-is, no case translation); the final segment is the type or
    // file name and gets kebab-cased before we append `.can`.
    let rest = &segments[2..];
    let (last, dirs) = rest.split_last()?;
    let mut rel = String::new();
    for d in dirs {
        rel.push_str(d);
        rel.push('/');
    }
    let stem = if last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        kebab_case(last)
    } else {
        // Already a snake_case or kebab-case path segment — use directly.
        // This covers `use canon/wasi/clocks/monotonic_clock`.
        (*last).to_string()
    };
    rel.push_str(&stem);
    rel.push_str(".can");

    let file = bundled_file(pkg, &rel)?;
    Some((pkg, file))
}

/// Parse a bundled package's manifest. Called lazily because the loader
/// only needs the dep graph, not every package's metadata up front.
pub fn parse_bundled_manifest(pkg: &BundledPackage) -> std::result::Result<Manifest, String> {
    manifest::parse(pkg.manifest_src).map_err(|e| format!("{}: {}", pkg.name, e))
}

// ---------------------------------------------------------------------------
// Module loading
// ---------------------------------------------------------------------------

pub struct LoadResult {
    pub module: Module,
    /// Index in `module.items` where items declared in the entry file
    /// begin. Items before this index were pulled in via `use` and are
    /// exempt from per-file ordering rules.
    pub entry_items_start: usize,
    /// Every user-authored source file that contributed to this module,
    /// in load order. Bundled package files are deliberately excluded —
    /// they ship with the compiler and aren't the user's responsibility
    /// to format. The entry file is always the first element.
    pub local_sources: Vec<LoadedSource>,
}

/// A user-authored Canon source file as the loader saw it on disk.
/// Used by the pipeline to enforce canonical formatting (see
/// `enforce_format` in `main.rs`).
#[derive(Debug, Clone)]
pub struct LoadedSource {
    pub path: PathBuf,
    pub source: String,
}

impl LoadCtx {
    /// Declaration index for a directory tree: name → files declaring
    /// it. Built once per directory, cached for the load. Skips
    /// generated/output trees (`bindgen/`, `build/`, `target/`,
    /// `node_modules/`) and hidden directories.
    fn dir_index(&mut self, dir: &Path) -> &std::collections::HashMap<String, Vec<PathBuf>> {
        if !self.dir_indexes.contains_key(dir) {
            let mut map: std::collections::HashMap<String, Vec<PathBuf>> =
                std::collections::HashMap::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = fs::read_dir(&d) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    if path.is_dir() {
                        if !name.starts_with('.')
                            && !matches!(
                                name.as_str(),
                                "bindgen" | "build" | "target" | "node_modules"
                            )
                        {
                            stack.push(path);
                        }
                    } else if name.ends_with(".can") {
                        if let Ok(src) = fs::read_to_string(&path) {
                            for decl in scan_decl_names(&src) {
                                let files = map.entry(decl).or_default();
                                if !files.contains(&path) {
                                    files.push(path.clone());
                                }
                            }
                        }
                    }
                }
            }
            for files in map.values_mut() {
                files.sort();
            }
            self.dir_indexes.insert(dir.to_path_buf(), map);
        }
        &self.dir_indexes[dir]
    }
}

struct LoadCtx {
    seen: HashSet<PathBuf>,
    /// Deduplicates bundled imports so a single package is loaded once even
    /// when multiple files transitively `use` it. Keyed by absolute bundled
    /// file path (`pkg.name + "/" + file.path`).
    seen_bundled: HashSet<String>,
    /// Every top-level declaration name loaded so far — consulted by
    /// name resolution so already-satisfied references don't re-resolve.
    declared: HashSet<String>,
    /// Cached per-directory declaration indexes for sibling resolution:
    /// dir → (name → files declaring it).
    dir_indexes:
        std::collections::HashMap<PathBuf, std::collections::HashMap<String, Vec<PathBuf>>>,
    items: Vec<Item>,
    /// User-authored sources accumulated during load (entry + transitive
    /// local `use` imports). Mirrors `seen` but keeps each file's full
    /// text so callers can validate canonical formatting later.
    local_sources: Vec<LoadedSource>,
    /// Root of the project that contains the entry file, identified by
    /// the nearest ancestor directory containing an `canon.toml`. `None`
    /// when the entry is a loose `.can` file outside any project (in that
    /// case `use` paths resolve via bundled packages and local-relative
    /// lookup only, exactly as before this field existed).
    ///
    /// When set, `process_use` consults `<project_root>/bindgen/` between
    /// the bundled-package check and the local-relative fallback. This is
    /// where `canon install` writes the materialized bindings declared
    /// in the manifest's `[imports]` table.
    project_root: Option<PathBuf>,
    /// Parsed `<project_root>/bindgen/_install.toml`, when present.
    /// Used to patch bare `extern Wasm` paths in local bindgen files —
    /// see `patch_extern_paths`. Loaded once when the project root is
    /// resolved; `None` when there's no project root or no index file.
    project_install_index: Option<InstallIndex>,
}

/// Walk up from `start` looking for the nearest directory that contains
/// an `canon.toml`. Returns that directory, or `None` if the walk
/// reaches the filesystem root without finding one. Used to anchor the
/// `bindgen/` lookup so a project's installed bindings are reachable
/// from any source file beneath the project root.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur: &Path = start;
    loop {
        if cur.join("canon.toml").is_file() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

pub fn load_module(entry: &Path) -> Result<LoadResult> {
    let canonical = entry.canonicalize().map_err(|err| CanonError::CheckError {
        message: format!("could not resolve `{}`: {}", entry.display(), err),
        span: Span::default(),
    })?;
    let dir = canonical.parent().unwrap_or_else(|| Path::new("."));
    let project_root = find_project_root(dir);
    let project_install_index = project_root
        .as_ref()
        .map(|root| root.join("bindgen").join(INSTALL_INDEX_FILENAME))
        .filter(|p| p.is_file())
        .and_then(|p| fs::read_to_string(&p).ok())
        .and_then(|src| install::parse_install_index(&src).ok());
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_bundled: HashSet::new(),
        declared: HashSet::new(),
        dir_indexes: std::collections::HashMap::new(),
        items: Vec::new(),
        local_sources: Vec::new(),
        project_root,
        project_install_index,
    };
    let source = fs::read_to_string(&canonical).map_err(|err| CanonError::CheckError {
        message: format!("could not read `{}`: {}", canonical.display(), err),
        span: Span::default(),
    })?;
    ctx.seen.insert(canonical.to_path_buf());
    ctx.local_sources.push(LoadedSource {
        path: canonical.to_path_buf(),
        source: source.clone(),
    });
    let entry_items_start = load_entry_source(&source, dir, &mut ctx)?;
    let span = Span::default();
    let mut module = Module {
        items: ctx.items,
        span,
    };
    // Auto-await: insert implicit `Expr::Await` nodes wherever a `Future<T>`
    // value is used in a position that expects `T`. Runs before the checker
    // so type comparisons see the post-rewrite tree.
    crate::checker::auto_await::transform(&mut module);
    Ok(LoadResult {
        module,
        entry_items_start,
        local_sources: ctx.local_sources,
    })
}

/// Same as `load_source`, but returns the index in `ctx.items` where the
/// entry file's own items begin. Used by the checker to scope per-file
/// ordering rules to user-authored code. The entry file is never a
/// bindgen file by construction, so no URN patching applies.
fn load_entry_source(source: &str, dir: &Path, ctx: &mut LoadCtx) -> Result<usize> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    // Apply bindings rewrite first — it converts function-type-alias
    // TypeDefs into FunctionDefs, which `resolve_new_syntax` then
    // processes (PascalCase constructor renaming, trait-impl receiver
    // extraction). Doing it the other way around would leave bindings
    // declarations stuck as TypeDefs through resolve_new_syntax and
    // they'd miss the trait-receiver extraction that makes `42.ToJson()`
    // dispatch correctly.
    apply_bindings_directive(&mut module.items);
    resolve_new_syntax(&mut module);

    let mut alias_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Alias(a) => alias_items.push(a),
            Item::Use(_) => {}
            other => other_items.push(other),
        }
    }
    ctx.declared.extend(decl_names(&other_items));
    for a in &alias_items {
        ctx.declared.insert(a.local.name.clone());
    }
    let renames = resolve_local_aliases(&alias_items, dir, ctx)?;
    resolve_referenced_names(&other_items, dir, ctx)?;
    let start = ctx.items.len();
    ctx.items.extend(other_items);
    ctx.items.extend(renames);
    Ok(start)
}

// ── Name resolution ─────────────────────────────────────────────────────
//
// There is no `use`. A file's referenced names resolve automatically:
//
//   1. names declared in the file itself (or already loaded),
//   2. sibling files in the same directory tree (a folder is a shelf,
//      not a scope — the kebab-case file for the name, or any file that
//      declares it),
//   3. the bundled standard library's `src/` tree.
//
// Bindgen output (`bindgen/` trees, bundled or project-local) never
// participates in name resolution — generated FFI shims are reached
// only through explicit alias declarations
// (`now = wasi/clocks/monotonic_clock/now`), which also disambiguate
// genuine collisions (`HttpStatus = std/http/Status`).

/// Names the compiler knows intrinsically — never resolved to files.
const RESOLUTION_BUILTINS: &[&str] = &[
    "Bit", "Bool", "Byte", "Bytes", "Err", "False", "Float", "Future", "Handle", "Hex", "Int",
    "List", "Map", "None", "Ok", "Option", "Result", "Self", "Set", "Some", "String", "True",
    "Unit",
];

fn is_resolution_builtin(name: &str) -> bool {
    RESOLUTION_BUILTINS.contains(&name)
}

/// Top-level declaration names in a set of items (types, functions,
/// alias locals). These never need resolving.
fn decl_names(items: &[Item]) -> HashSet<String> {
    let mut out = HashSet::new();
    for item in items {
        match item {
            Item::TypeDef(td) => {
                out.insert(td.name.name.clone());
            }
            Item::Function(f) => {
                out.insert(f.name.name.clone());
            }
            Item::Alias(a) => {
                out.insert(a.local.name.clone());
            }
            _ => {}
        }
    }
    out
}

/// Scan a Canon source for its top-level declaration names without
/// parsing: a declaration is a line starting in column 0 with an
/// identifier followed by `=`. Cheap enough to run over whole
/// directories and the bundled registry.
fn scan_decl_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let bytes = line.as_bytes();
        if bytes.first().is_none_or(|b| !b.is_ascii_alphabetic()) {
            continue;
        }
        let end = bytes
            .iter()
            .position(|b| !b.is_ascii_alphanumeric())
            .unwrap_or(bytes.len());
        if line[end..].trim_start().starts_with('=') {
            out.push(line[..end].to_string());
        }
    }
    out
}

/// Lazily-built index of the bundled packages' `src/` declarations:
/// name → every (package, file) that declares it. Bindgen files
/// (those carrying a `wit_urn`) are excluded by design.
fn std_src_index(
) -> &'static std::collections::HashMap<String, Vec<(&'static BundledPackage, &'static BundledFile)>>
{
    use std::sync::OnceLock;
    static INDEX: OnceLock<
        std::collections::HashMap<String, Vec<(&'static BundledPackage, &'static BundledFile)>>,
    > = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: std::collections::HashMap<
            String,
            Vec<(&'static BundledPackage, &'static BundledFile)>,
        > = std::collections::HashMap::new();
        for pkg in BUNDLED_PACKAGES.iter() {
            for file in pkg.files.iter() {
                if file.wit_urn.is_some() {
                    continue;
                }
                let mut names = scan_decl_names(file.source);
                // The file-naming convention also names the file's
                // primary type: `stream.can` answers for `Stream` even
                // when the file only declares that type's *functions*
                // (combinator modules over builtin generics).
                if let Some(stem) = file
                    .path
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.strip_suffix(".can"))
                {
                    names.push(crate::bindgen::naming::kebab_to_pascal(stem));
                }
                for name in names {
                    let entry = map.entry(name).or_default();
                    if !entry.iter().any(|(_, f)| f.path == file.path) {
                        entry.push((pkg, file));
                    }
                }
            }
        }
        map
    })
}

/// Collect every name a set of items *references* — type positions,
/// constructor calls, PascalCase method/field names (trait calls and
/// type-as-method constructors), and the `ToJson` conversions implied
/// by interpolated JSON literals.
fn collect_referenced_names(items: &[Item], out: &mut std::collections::BTreeSet<String>) {
    for item in items {
        match item {
            // A typedef's union body *introduces* its variant names
            // (implicit unit variants) rather than referencing them —
            // `Ord = Equal + Greater + Less` must not go looking for an
            // `equal.can`. Generic arguments inside variants are still
            // real references (`Some<Content>`).
            Item::TypeDef(td) => match &td.body {
                TypeExpr::Union { variants, .. } => {
                    for v in variants {
                        if let TypeExpr::Named { generics, .. } = v {
                            for g in generics {
                                collect_type_names(g, out);
                            }
                        } else {
                            collect_type_names(v, out);
                        }
                    }
                }
                other => collect_type_names(other, out),
            },
            Item::Function(f) => {
                if let Some(recv) = &f.receiver {
                    out.insert(recv.name.clone());
                }
                for p in &f.params {
                    collect_type_names(&p.ty, out);
                }
                collect_type_names(&f.return_ty, out);
                for e in &f.body.exprs {
                    collect_expr_names_res(e, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_type_names(ty: &TypeExpr, out: &mut std::collections::BTreeSet<String>) {
    match ty {
        TypeExpr::Named { name, generics, .. } => {
            out.insert(name.clone());
            for g in generics {
                collect_type_names(g, out);
            }
        }
        TypeExpr::Union { variants, .. } => {
            for v in variants {
                collect_type_names(v, out);
            }
        }
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                collect_type_names(f, out);
            }
        }
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => collect_type_names(ty, out),
        TypeExpr::Function {
            params, return_ty, ..
        } => {
            for p in params {
                collect_type_names(p, out);
            }
            collect_type_names(return_ty, out);
        }
    }
}

fn collect_expr_names_res(expr: &Expr, out: &mut std::collections::BTreeSet<String>) {
    use crate::ast::JsonLitPart;
    match expr {
        Expr::Ident(id) => {
            if id
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
            {
                out.insert(id.name.clone());
            }
        }
        Expr::Constructor { name, args, .. } => {
            out.insert(name.name.clone());
            for a in args {
                collect_expr_names_res(a, out);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            ..
        } => {
            if method
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
            {
                out.insert(method.name.clone());
            }
            for ta in type_args {
                collect_type_names(ta, out);
            }
            collect_expr_names_res(receiver, out);
            for a in args {
                collect_expr_names_res(a, out);
            }
        }
        Expr::FieldAccess {
            receiver, field, ..
        } => {
            if field
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
            {
                out.insert(field.name.clone());
            }
            collect_expr_names_res(receiver, out);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_names_res(scrutinee, out);
            for arm in arms {
                collect_type_names(&arm.param_ty, out);
                collect_type_names(&arm.return_ty, out);
                for e in &arm.body.exprs {
                    collect_expr_names_res(e, out);
                }
            }
        }
        Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } => {
            for p in params {
                collect_type_names(&p.ty, out);
            }
            collect_type_names(return_ty, out);
            for e in &body.exprs {
                collect_expr_names_res(e, out);
            }
        }
        Expr::JsonLit { parts, .. } => {
            for p in parts {
                if let JsonLitPart::Interp(e) = p {
                    out.insert("ToJson".to_string());
                    collect_expr_names_res(e, out);
                }
            }
        }
        Expr::Try { inner, .. } | Expr::Await { inner, .. } => collect_expr_names_res(inner, out),
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                collect_expr_names_res(f, out);
            }
        }
        Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. } => {}
    }
}

/// Candidate `use`-machinery paths for an alias declaration's RHS.
/// `std/time/Instant` → primary `canon/std/time/Instant` (file =
/// kebab of the final PascalCase segment); `wasi/clocks/types/Instant`
/// needs the fallback, where the segment *before* the name is the file.
fn alias_candidate_paths(a: &crate::ast::AliasDecl) -> (String, Option<String>) {
    let mut segs = a.path.clone();
    if segs.first().map(String::as_str) == Some("std") {
        segs[0] = "canon/std".to_string();
    }
    let last = segs.last().cloned().unwrap_or_default();
    let is_pascal = last.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if is_pascal {
        let primary = segs.join("/");
        let fallback = (segs.len() > 1).then(|| segs[..segs.len() - 1].join("/"));
        (primary, fallback)
    } else {
        // camelCase function alias: the file is the preceding segment.
        (segs[..segs.len() - 1].join("/"), None)
    }
}

fn synthetic_use(path: String) -> crate::ast::UseDecl {
    crate::ast::UseDecl {
        name: crate::ast::Ident {
            name: path,
            span: Span::default(),
        },
        span: Span::default(),
    }
}

/// Process alias declarations for a local (on-disk) file, and return
/// any synthetic rename typedefs (`HttpStatus = Status`) to append.
fn resolve_local_aliases(
    aliases: &[crate::ast::AliasDecl],
    dir: &Path,
    ctx: &mut LoadCtx,
) -> Result<Vec<Item>> {
    let mut extra = Vec::new();
    for a in aliases {
        let (primary, fallback) = alias_candidate_paths(a);
        let res = process_use(&synthetic_use(primary), dir, ctx);
        if res.is_err() {
            if let Some(fb) = fallback {
                process_use(&synthetic_use(fb), dir, ctx)?;
            } else {
                res?;
            }
        }
        extra.extend(alias_rename_item(a)?);
    }
    Ok(extra)
}

/// If the alias renames (`Local = …/Name` with `Local != Name`), emit a
/// typedef `Local = Name` so the local name works everywhere the
/// original does. Only PascalCase (type) aliases can rename.
fn alias_rename_item(a: &crate::ast::AliasDecl) -> Result<Vec<Item>> {
    let last = a.path.last().cloned().unwrap_or_default();
    if a.local.name == last {
        return Ok(Vec::new());
    }
    let is_pascal = last.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if !is_pascal {
        return Err(CanonError::CheckError {
            message: format!(
                "function alias `{}` cannot rename `{}` — declare it under its own name",
                a.local.name, last
            ),
            span: a.span,
        });
    }
    Ok(vec![Item::TypeDef(crate::ast::TypeDef {
        name: a.local.clone(),
        generic_params: Vec::new(),
        body: TypeExpr::Named {
            name: last,
            generics: Vec::new(),
            span: a.span,
        },
        span: a.span,
    })])
}

/// Resolve a local file's referenced names: sibling directory tree
/// first, then the bundled std `src/` index. Unresolved names are left
/// for the checker to report.
fn resolve_referenced_names(items: &[Item], dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let mut refs = std::collections::BTreeSet::new();
    collect_referenced_names(items, &mut refs);
    let own = decl_names(items);
    for name in refs {
        if is_resolution_builtin(&name) || own.contains(&name) || ctx.declared.contains(&name) {
            continue;
        }
        // 1. Sibling files in the entry's directory tree.
        let local_matches = ctx.dir_index(dir).get(&name).cloned().unwrap_or_default();
        match local_matches.len() {
            0 => {}
            1 => {
                load_into(&local_matches[0], ctx)?;
                continue;
            }
            _ => {
                return Err(CanonError::CheckError {
                    message: format!(
                        "`{}` is declared in more than one file here: {} — rename one, or \
                         disambiguate with an alias declaration",
                        name,
                        local_matches
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    span: Span::default(),
                });
            }
        }
        // 2. Bundled std src.
        if let Some(cands) = std_src_index().get(&name) {
            if cands.len() > 1 {
                return Err(CanonError::CheckError {
                    message: format!(
                        "`{}` is ambiguous in the standard library ({}) — disambiguate with an \
                         alias declaration like `{} = std/…/{}`",
                        name,
                        cands
                            .iter()
                            .map(|(p, f)| format!("{}/{}", p.name, f.path))
                            .collect::<Vec<_>>()
                            .join(", "),
                        name,
                        name
                    ),
                    span: Span::default(),
                });
            }
            let (pkg, file) = cands[0];
            let key = format!("{}/{}", pkg.name, file.path);
            if ctx.seen_bundled.insert(key) {
                load_bundled_source(pkg, file, ctx)?;
            }
        }
        // Not found anywhere: the checker reports unknown types with
        // proper spans; many names (builtin methods, trait calls on
        // local types) simply have nothing to load.
    }
    Ok(())
}

fn process_use(u: &crate::ast::UseDecl, dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let path_str = &u.name.name;

    // Package resolution: if the first two segments name a bundled package,
    // serve the file from the embedded registry. (Project-manifest-driven
    // dep gating will layer on top of this in a later phase.)
    if let Some((pkg, file)) = resolve_bundled_use(path_str) {
        let key = format!("{}/{}", pkg.name, file.path);
        if ctx.seen_bundled.insert(key) {
            // Bundled files have no on-disk directory we can hand to nested
            // `use` lookups. Their `use` lines resolve either against the
            // bundled registry (cross-package) or against the importing
            // file's own directory within its package (same-package).
            load_bundled_source(pkg, file, ctx)?;
        }
        return Ok(());
    }

    // Project `bindgen/` lookup. When the entry file lives inside a
    // project (an ancestor directory has `canon.toml`), `use` paths
    // also resolve against `<project_root>/bindgen/<path>.can` — the
    // directory where `canon install` materializes external bindings
    // declared in the manifest's `[imports]` table. Sits between the
    // bundled-package check and the local-relative fallback so bindgen
    // output is reachable from any source file in the project without
    // overriding either bundled std lookups or in-project sibling files.
    if let Some(root) = ctx.project_root.clone() {
        let bindgen_file = root.join("bindgen").join(format!("{}.can", path_str));
        if bindgen_file.is_file() {
            let canonical = bindgen_file
                .canonicalize()
                .map_err(|err| CanonError::CheckError {
                    message: format!("could not resolve `{}`: {}", bindgen_file.display(), err),
                    span: u.span,
                })?;
            load_into(&canonical, ctx)?;
            return Ok(());
        }
    }

    // Local file/module lookup.
    let segments: Vec<&str> = path_str.split('/').collect();
    let type_name = segments[segments.len() - 1];
    let file_stem = kebab_case(type_name);
    let mut file_dir = dir.to_path_buf();
    for seg in &segments[..segments.len() - 1] {
        file_dir = file_dir.join(seg);
    }

    let candidate = file_dir.join(format!("{}.can", file_stem));
    let module_candidate = file_dir.join(&file_stem).join("main.can");

    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|err| CanonError::CheckError {
                message: format!("could not resolve `{}`: {}", candidate.display(), err),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
    } else if module_candidate.exists() {
        let canonical = module_candidate
            .canonicalize()
            .map_err(|err| CanonError::CheckError {
                message: format!(
                    "could not resolve `{}`: {}",
                    module_candidate.display(),
                    err
                ),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
    } else {
        return Err(CanonError::CheckError {
            message: format!(
                "`use {}` cannot find `{}`",
                u.name.name,
                candidate.display(),
            ),
            span: u.span,
        });
    }
    Ok(())
}

fn load_into(path: &Path, ctx: &mut LoadCtx) -> Result<()> {
    if !ctx.seen.insert(path.to_path_buf()) {
        return Ok(());
    }
    let source = fs::read_to_string(path).map_err(|err| CanonError::CheckError {
        message: format!("could not read `{}`: {}", path.display(), err),
        span: Span::default(),
    })?;
    ctx.local_sources.push(LoadedSource {
        path: path.to_path_buf(),
        source: source.clone(),
    });
    let wit_urn = urn_for_local_file(path, ctx);
    load_source(
        &source,
        path.parent().unwrap_or_else(|| Path::new(".")),
        wit_urn.as_deref(),
        ctx,
    )
}

/// Compute the WIT interface URN for a local file by looking it up in
/// the project's install index. Returns `None` for files outside
/// `<project_root>/bindgen/`, for projects without a `_install.toml`,
/// or for `bindgen/` files the index doesn't know about.
fn urn_for_local_file(path: &Path, ctx: &LoadCtx) -> Option<String> {
    let project_root = ctx.project_root.as_ref()?;
    let index = ctx.project_install_index.as_ref()?;
    let bindgen_root = project_root.join("bindgen");
    let rel = path.strip_prefix(&bindgen_root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    index.urn_for(&rel_str).map(|s| s.to_string())
}

fn load_source(source: &str, dir: &Path, wit_urn: Option<&str>, ctx: &mut LoadCtx) -> Result<()> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    // Bindings rewrite runs before resolve_new_syntax so the produced
    // FunctionDefs go through the same PascalCase / trait-impl /
    // constructor normalisation as hand-written functions.
    apply_bindings_directive(&mut module.items);
    resolve_new_syntax(&mut module);

    let mut alias_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Alias(a) => alias_items.push(a),
            Item::Use(_) => {}
            other => other_items.push(other),
        }
    }
    ctx.declared.extend(decl_names(&other_items));
    for a in &alias_items {
        ctx.declared.insert(a.local.name.clone());
    }
    let renames = resolve_local_aliases(&alias_items, dir, ctx)?;
    resolve_referenced_names(&other_items, dir, ctx)?;
    if let Some(urn) = wit_urn {
        patch_extern_paths(&mut other_items, urn);
    }
    ctx.items.extend(other_items);
    ctx.items.extend(renames);
    Ok(())
}

/// Load a bundled file's source. `use` lines inside the source resolve
/// against either the bundled registry (cross-package) or the importing
/// file's directory within its package (same-package). Local-disk paths
/// are not available because the bundle is in-memory.
fn load_bundled_source(
    pkg: &'static BundledPackage,
    current: &'static BundledFile,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let mut scanner = Scanner::new(current.source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    // Same order as `load_source`: rewrite bindings first, then let
    // resolve_new_syntax pick up the produced FunctionDefs for the
    // PascalCase normalisation pass.
    apply_bindings_directive(&mut module.items);
    resolve_new_syntax(&mut module);

    let mut alias_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Alias(a) => alias_items.push(a),
            Item::Use(_) => {}
            other => other_items.push(other),
        }
    }
    ctx.declared.extend(decl_names(&other_items));
    for a in &alias_items {
        ctx.declared.insert(a.local.name.clone());
    }
    let mut renames = Vec::new();
    for a in &alias_items {
        let (primary, fallback) = alias_candidate_paths(a);
        let res = process_bundled_use(pkg, current, &synthetic_use(primary), ctx);
        if res.is_err() {
            if let Some(fb) = fallback {
                process_bundled_use(pkg, current, &synthetic_use(fb), ctx)?;
            } else {
                res?;
            }
        }
        renames.extend(alias_rename_item(a)?);
    }
    resolve_bundled_names(pkg, &other_items, ctx)?;
    if let Some(urn) = current.wit_urn {
        patch_extern_paths(&mut other_items, urn);
    }
    ctx.items.extend(other_items);
    ctx.items.extend(renames);
    Ok(())
}

/// Resolve a bundled file's referenced names against its own package's
/// `src/` declarations (the same flat-namespace rule local files get).
fn resolve_bundled_names(
    pkg: &'static BundledPackage,
    items: &[Item],
    ctx: &mut LoadCtx,
) -> Result<()> {
    let mut refs = std::collections::BTreeSet::new();
    collect_referenced_names(items, &mut refs);
    let own = decl_names(items);
    for name in refs {
        if is_resolution_builtin(&name) || own.contains(&name) || ctx.declared.contains(&name) {
            continue;
        }
        let Some(cands) = std_src_index().get(&name) else {
            continue;
        };
        let in_pkg: Vec<_> = cands.iter().filter(|(p, _)| p.name == pkg.name).collect();
        if in_pkg.len() != 1 {
            continue; // absent or ambiguous — leave to the checker
        }
        let (p, file) = *in_pkg[0];
        let key = format!("{}/{}", p.name, file.path);
        if ctx.seen_bundled.insert(key) {
            load_bundled_source(p, file, ctx)?;
        }
    }
    Ok(())
}

/// Resolve a `use` directive that appeared inside a bundled file.
///
/// Resolution rule:
/// 1. If the path's first two segments name a known bundled package, treat
///    as a cross-package import.
/// 2. Otherwise try same-directory relative first: `use Foo` from
///    `time/instant.can` resolves to `time/foo.can`. This is how sibling
///    imports inside the package have always worked.
/// 3. If that misses, try the use path as a package-root-relative path:
///    `use wasi/random/random` from `random.can` resolves to
///    `wasi/random/random.can` at the package root. This is what lets
///    `canon/std`'s hand-written wrappers `use wasi/…` against the
///    bindings materialized under `<package>/bindgen/` (which the
///    bundler flattens into the same namespace as `src/`).
fn process_bundled_use(
    pkg: &'static BundledPackage,
    current: &'static BundledFile,
    u: &crate::ast::UseDecl,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let path_str = &u.name.name;

    // Cross-package lookup. `resolve_bundled_use` only returns `Some` when
    // the path's first two segments name an actual bundled package.
    if let Some((other_pkg, file)) = resolve_bundled_use(path_str) {
        let key = format!("{}/{}", other_pkg.name, file.path);
        if ctx.seen_bundled.insert(key) {
            load_bundled_source(other_pkg, file, ctx)?;
        }
        return Ok(());
    }

    // Compute the importing file's directory within its package, used
    // for the sibling-relative attempt.
    let current_dir = current
        .path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();

    // Translate the use path into a candidate filename relative to some
    // base directory: `use sub/Foo` becomes `<base>sub/foo.can`,
    // `use lowercase/path` becomes `<base>lowercase/path.can`.
    let candidate_path = |base: &str| -> String {
        let segments: Vec<&str> = path_str.split('/').collect();
        let (last, dirs) = segments.split_last().expect("split_last on non-empty Vec");
        let mut rel = base.to_string();
        for d in dirs {
            rel.push_str(d);
            rel.push('/');
        }
        let stem = if last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            kebab_case(last)
        } else {
            (*last).to_string()
        };
        rel.push_str(&stem);
        rel.push_str(".can");
        rel
    };

    let sibling_rel = candidate_path(&current_dir);
    let root_rel = candidate_path("");

    let file = bundled_file(pkg, &sibling_rel).or_else(|| bundled_file(pkg, &root_rel));

    let Some(file) = file else {
        // Report both lookup paths so the error names exactly what we
        // tried. The sibling form comes first because it's the more
        // intuitive case for in-package imports.
        let looked_for = if sibling_rel == root_rel {
            sibling_rel.clone()
        } else {
            format!("{sibling_rel}` or `{root_rel}")
        };
        return Err(CanonError::CheckError {
            message: format!(
                "`use {}` from package `{}` not found (looked for `{}`)",
                u.name.name, pkg.name, looked_for,
            ),
            span: u.span,
        });
    };
    let key = format!("{}/{}", pkg.name, file.path);
    if ctx.seen_bundled.insert(key) {
        load_bundled_source(pkg, file, ctx)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// Convert a PascalCase type name to its kebab-case file stem.
/// `UserRole` → `user-role`, `HttpServer` → `http-server`, `Color` → `color`
pub fn kebab_case(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 && chars[i - 1].is_ascii_lowercase() {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
