use crate::ast::{
    extract_receiver_from_params, resolve_new_syntax, BindingsDecl, Block, Expr, ExternWasm,
    FunctionDef, Item, JsonLitPart, MatchArm, Module, PackageDecl, Param, TypeExpr,
};
use crate::bindgen;
use crate::error::{CanonError, Result, Span};
use crate::install::{self, InstallIndex, INSTALL_INDEX_FILENAME};
use crate::lexer::Scanner;
use crate::manifest::{self, Manifest};
use crate::parser::Parser;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    /// Always uses `/` separators.
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

/// Resolve a package path (`a/b/c/…/Z`, e.g. `canon/std/Json`) against
/// the bundled packages. Only the JSON prelude and tooling use this
/// path-shaped lookup now — ordinary references resolve by name via
/// `bundled_decl_matches`.
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
    /// begin. Items before this index were pulled in by reference
    /// discovery and are exempt from per-file ordering rules.
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

struct LoadCtx {
    seen: HashSet<PathBuf>,
    /// Deduplicates bundled imports so a single package is loaded once even
    /// when multiple files transitively reference it. Keyed by absolute
    /// bundled file path (`pkg.name + "/" + file.path`).
    seen_bundled: HashSet<String>,
    items: Vec<Item>,
    /// Every top-level name (type or function) defined by an item loaded
    /// so far — plus the names of the file currently being processed,
    /// which are registered *before* its references are resolved so that
    /// mutually-referencing files don't chase each other. Reference
    /// discovery consults this set before searching any root.
    defined: HashSet<String>,
    /// Lazily-built recursive file-stem indexes for local project trees,
    /// keyed by the directory the scan was rooted at. See
    /// `local_stem_index`.
    local_stems: HashMap<PathBuf, HashMap<String, Vec<PathBuf>>>,
    /// Lazily-built declaration index over `<project_root>/bindgen/`:
    /// declared name → declaring files.
    bindgen_decls: Option<HashMap<String, Vec<PathBuf>>>,
    /// Lazily-built declaration index over the project's `deps/` tree.
    deps_decls: Option<HashMap<String, Vec<PathBuf>>>,
    /// User-authored sources accumulated during load (entry + transitive
    /// local imports). Mirrors `seen` but keeps each file's full
    /// text so callers can validate canonical formatting later.
    local_sources: Vec<LoadedSource>,
    /// Root of the project that contains the entry file, identified by
    /// the nearest ancestor directory containing an `canon.toml`. `None`
    /// when the entry is a loose `.can` file outside any project (in that
    /// case references resolve via the local tree and bundled packages
    /// only).
    ///
    /// When set, reference discovery consults `<project_root>/bindgen/` —
    /// where `canon install` writes the materialized bindings declared
    /// in the manifest's `[imports]` table.
    project_root: Option<PathBuf>,
    /// Parsed `<project_root>/bindgen/_install.toml`, when present.
    /// Used to patch bare `extern Wasm` paths in local bindgen files —
    /// see `patch_extern_paths`. Loaded once when the project root is
    /// resolved; `None` when there's no project root or no index file.
    project_install_index: Option<InstallIndex>,
    /// Canonicalized path of the project's vendored-dependency tree
    /// (`<root>/deps/`, see PACKAGES.md), when it exists on disk. The
    /// root is the project root when there is one, otherwise the entry
    /// file's directory — so manifest-free projects (the PACKAGES.md
    /// end state) resolve `deps/` without an `canon.toml` marker.
    ///
    /// `Some` enables two things: reference discovery resolves names
    /// against the `deps/` tree, and `load_into` recognizes files
    /// under this prefix as vendored, which requires (and validates)
    /// their `package` directive.
    deps_dir: Option<PathBuf>,
    /// Version agreement across each vendored package: maps a package
    /// key (`"<ns>:<name>"`) to the version its first-loaded file
    /// declared plus that file's label, so a mismatch in a later file
    /// can name both sides.
    deps_versions: HashMap<String, (String, String)>,
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
    let deps_dir = project_root
        .as_deref()
        .unwrap_or(dir)
        .join("deps")
        .canonicalize()
        .ok();
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_bundled: HashSet::new(),
        items: Vec::new(),
        defined: HashSet::new(),
        local_stems: HashMap::new(),
        bindgen_decls: None,
        deps_decls: None,
        local_sources: Vec::new(),
        project_root,
        project_install_index,
        deps_dir,
        deps_versions: HashMap::new(),
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
    // The entry file is never vendored, so a `package` directive in it
    // is always an error.
    validate_package_directives(&module.items, None, ctx)?;

    let other_items = module.items;
    register_defined_names(&other_items, ctx);
    inject_json_prelude(&other_items, ctx)?;
    inject_int_prelude(&other_items, ctx)?;
    discover_references(&other_items, dir, ctx)?;
    let start = ctx.items.len();
    ctx.items.extend(other_items);
    Ok(start)
}

/// Int prelude: the fallible parse constructor `Int(String) ->
/// Result<Int, MalformedInt>` lives in `canon/std/Int` (pure Canon),
/// but the name `Int` is undiscoverable — it appears in virtually
/// every program as the builtin type. Mirror of the JSON prelude:
/// load the stdlib module only when the program actually reaches for
/// the parse — an `Int(…)` constructor whose argument isn't already a
/// numeric literal, or a zero-arg `.Int()` conversion call. Skipped
/// when the file supplies its own `Int` constructor function.
fn inject_int_prelude(other_items: &[Item], ctx: &mut LoadCtx) -> Result<()> {
    let already_in_scope = other_items.iter().any(|item| match item {
        Item::Function(f) => {
            f.name.name == "Int" || f.receiver.as_ref().is_some_and(|r| r.name == "Int")
        }
        _ => false,
    });
    if already_in_scope || !items_use_int_parse(other_items) {
        return Ok(());
    }
    let Some((pkg, file)) = resolve_bundled_use("canon/std/Int") else {
        return Ok(());
    };
    let key = format!("{}/{}", pkg.name, file.path);
    if ctx.seen_bundled.insert(key) {
        load_bundled_source(pkg, file, ctx)?;
    }
    Ok(())
}

fn items_use_int_parse(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Function(f) => f.body.exprs.iter().any(expr_uses_int_parse),
        _ => false,
    })
}

fn expr_uses_int_parse(expr: &Expr) -> bool {
    match expr {
        Expr::Constructor { name, args, .. } => {
            (name.name == "Int"
                && args.first().is_some_and(|a| {
                    !matches!(
                        a,
                        Expr::IntLit { .. } | Expr::FloatLit { .. } | Expr::HexLit { .. }
                    )
                }))
                || args.iter().any(expr_uses_int_parse)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            (method.name == "Int" && args.is_empty())
                || expr_uses_int_parse(receiver)
                || args.iter().any(expr_uses_int_parse)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_uses_int_parse(scrutinee)
                || arms
                    .iter()
                    .any(|arm| arm.body.exprs.iter().any(expr_uses_int_parse))
        }
        Expr::Try { inner, .. } | Expr::Await { inner, .. } => expr_uses_int_parse(inner),
        Expr::Lambda { body, .. } => body.exprs.iter().any(expr_uses_int_parse),
        Expr::ProductValue { fields, .. } => fields.iter().any(expr_uses_int_parse),
        Expr::FieldAccess { receiver, .. } => expr_uses_int_parse(receiver),
        Expr::JsonLit { .. }
        | Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. } => false,
    }
}

/// JSON prelude: `canon/std/Json` loads automatically — like Rust's
/// prelude, JSON support doesn't need an explicit import. A fully static
/// JSON literal is constant-folded and needs nothing at all (the checker
/// knows `Json = String` intrinsically), so the stdlib module is pulled
/// in only when the program actually reaches for its machinery:
/// interpolation inside a literal (`{"n":Int}` converts via `ToJson`),
/// the validating `Json(...)` constructor, or an explicit `.ToJson()` /
/// `.Json()` call. Skipped when the file defines `Json` itself or the
/// module already loaded a `Json` definition.
fn inject_json_prelude(other_items: &[Item], ctx: &mut LoadCtx) -> Result<()> {
    let already_in_scope = ctx.defined.contains("Json")
        || other_items.iter().any(|item| match item {
            Item::TypeDef(td) => td.name.name == "Json",
            Item::Function(f) => f.name.name == "Json" && f.extern_wasm.is_some(),
            _ => false,
        });
    if already_in_scope || !items_use_json_machinery(other_items) {
        return Ok(());
    }
    let Some((pkg, file)) = resolve_bundled_use("canon/std/Json") else {
        return Ok(());
    };
    let key = format!("{}/{}", pkg.name, file.path);
    if ctx.seen_bundled.insert(key) {
        load_bundled_source(pkg, file, ctx)?;
    }
    Ok(())
}

fn items_use_json_machinery(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Function(f) => f.body.exprs.iter().any(expr_uses_json_machinery),
        _ => false,
    })
}

fn expr_uses_json_machinery(expr: &Expr) -> bool {
    use crate::ast::JsonLitPart;
    match expr {
        Expr::JsonLit { parts, .. } => parts.iter().any(|p| match p {
            JsonLitPart::Static(_) => false,
            JsonLitPart::Interp(e) => {
                // The interpolation itself needs `ToJson`, whatever the
                // inner expression is.
                let _ = e;
                true
            }
        }),
        Expr::Constructor { name, args, .. } => {
            name.name == "Json" || args.iter().any(expr_uses_json_machinery)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            method.name == "ToJson"
                || method.name == "Json"
                || expr_uses_json_machinery(receiver)
                || args.iter().any(expr_uses_json_machinery)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_uses_json_machinery(scrutinee)
                || arms
                    .iter()
                    .any(|arm| arm.body.exprs.iter().any(expr_uses_json_machinery))
        }
        Expr::Try { inner, .. } | Expr::Await { inner, .. } => expr_uses_json_machinery(inner),
        Expr::Lambda { body, .. } => body.exprs.iter().any(expr_uses_json_machinery),
        Expr::ProductValue { fields, .. } => fields.iter().any(expr_uses_json_machinery),
        Expr::FieldAccess { receiver, .. } => expr_uses_json_machinery(receiver),
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. } => false,
    }
}

// ---------------------------------------------------------------------------
// Reference discovery — the `use`-less import rule
// ---------------------------------------------------------------------------
//
// There is no import statement. A reference to a name `Z` that the
// current file does not define resolves — by convention, name → file —
// against, in this order of *search* (not precedence):
//
//   1. the referencing file's own directory tree (recursive):
//      `<kebab(Z)>.can` or `<kebab(Z)>/main.can`, skipping `deps/`,
//      `bindgen/`, `target/`, and hidden directories;
//   2. the project's `bindgen/` tree, by declared name;
//   3. the project's `deps/` tree (vendored packages), by declared name;
//   4. the bundled packages (`canon/std`), by declared name.
//
// The non-local roots are *declaration-indexed* rather than
// filename-matched because binding files declare functions whose names
// don't kebab-back to their file (`getRandomU64` lives in `random.can`).
//
// Ambiguity is a hard error, not a precedence (PACKAGES.md
// § Resolution): a name resolving in more than one place fails the
// build naming every candidate. A name resolving nowhere is *not* a
// loader error — the checker reports undefined names with full type
// context, so discovery stays best-effort and only ever adds files.

/// Names that never trigger discovery: builtin types, builtin generic
/// containers, their builtin variants, and the intrinsically-known
/// `Json` alias (the JSON prelude decides when the stdlib machinery is
/// actually needed — see `inject_json_prelude`).
// NOTE: `Map` and `Set` are deliberately NOT here — they are ordinary
// pure-Canon stdlib modules (`canon/std/{map,set}.can`), so referencing
// either name loads its file like any other stdlib type. `Int` IS here
// (it appears in virtually every program as the builtin type), so the
// stdlib parse constructor `Int(String)` loads through
// `inject_int_prelude` instead — the same targeted mechanism as the
// JSON prelude.
const UNDISCOVERABLE_TYPES: &[&str] = &[
    "Bool",
    "Deserialize",
    "Err",
    "ExitCode",
    "False",
    "Float",
    "Future",
    "Handle",
    "Hex",
    "Int",
    "Json",
    "List",
    "Network",
    "Never",
    "None",
    "Ok",
    "Option",
    "Result",
    "Self",
    "Serialize",
    "Some",
    "Stderr",
    "Stdin",
    "Stdout",
    "Stream",
    "String",
    "True",
    "Unit",
];

/// Builtin method names (mirrors the checker's `is_known_method` plus
/// the concurrency combinators). Calling one of these can never mean
/// "load a file" — they're implemented by the codegen directly.
const UNDISCOVERABLE_METHODS: &[&str] = &[
    "add",
    "and",
    "append",
    "byteAt",
    "concat",
    "contains",
    "div",
    "empty",
    "eq",
    "first",
    "ge",
    "get",
    "gt",
    "insert",
    "keys",
    "le",
    "len",
    "length",
    "lt",
    "main",
    "map",
    "mod",
    "mul",
    "ne",
    "not",
    "or",
    "parallel",
    "print",
    "race",
    "rem",
    "remove",
    "slice",
    "sub",
    "substring",
    "values",
];

fn is_undiscoverable(name: &str) -> bool {
    UNDISCOVERABLE_TYPES.contains(&name) || UNDISCOVERABLE_METHODS.contains(&name)
}

/// Register every top-level name `items` defines into `ctx.defined`.
/// Called *before* the same items' references are resolved so that
/// self-references and mutually-referencing files terminate.
fn register_defined_names(items: &[Item], ctx: &mut LoadCtx) {
    for item in items {
        match item {
            Item::TypeDef(td) => {
                ctx.defined.insert(td.name.name.clone());
            }
            Item::Function(f) => {
                ctx.defined.insert(f.name.name.clone());
            }
            _ => {}
        }
    }
}

/// Referenced names, each with the span of its first occurrence (for
/// error reporting). BTreeMap so resolution order — and therefore item
/// order in the loaded module — is deterministic and alphabetical, the
/// same order `use` lines used to load in.
type Refs = BTreeMap<String, Span>;

fn collect_item_refs(item: &Item, out: &mut Refs) {
    match item {
        Item::TypeDef(td) => {
            let skip: HashSet<&str> = td
                .generic_params
                .iter()
                .map(|g| g.name.name.as_str())
                .collect();
            collect_ty_refs(&td.body, &skip, out);
        }
        Item::Function(f) => {
            let skip: HashSet<&str> = f
                .generic_params
                .iter()
                .map(|g| g.name.name.as_str())
                .collect();
            if let Some(r) = &f.receiver {
                add_ref(&r.name, r.span, &skip, out);
            }
            for p in &f.params {
                collect_ty_refs(&p.ty, &skip, out);
            }
            collect_ty_refs(&f.return_ty, &skip, out);
            for e in &f.body.exprs {
                collect_expr_refs(e, &skip, out);
            }
        }
        _ => {}
    }
}

fn add_ref(name: &str, span: Span, skip: &HashSet<&str>, out: &mut Refs) {
    if skip.contains(name) {
        return;
    }
    out.entry(name.to_string()).or_insert(span);
}

fn collect_ty_refs(ty: &TypeExpr, skip: &HashSet<&str>, out: &mut Refs) {
    match ty {
        TypeExpr::Named {
            name,
            generics,
            span,
        } => {
            add_ref(name, *span, skip, out);
            for g in generics {
                collect_ty_refs(g, skip, out);
            }
        }
        TypeExpr::Union { variants, .. } => {
            for v in variants {
                collect_ty_refs(v, skip, out);
            }
        }
        TypeExpr::Product { fields, .. } => {
            for f in fields {
                collect_ty_refs(f, skip, out);
            }
        }
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => collect_ty_refs(ty, skip, out),
        TypeExpr::Function {
            generic_params,
            params,
            return_ty,
            ..
        } => {
            let mut inner: HashSet<&str> = skip.clone();
            inner.extend(generic_params.iter().map(|g| g.name.name.as_str()));
            for p in params {
                collect_ty_refs(p, &inner, out);
            }
            collect_ty_refs(return_ty, &inner, out);
        }
    }
}

/// Expression walk. Deliberately narrower than the checker's dead-code
/// walk: bare identifiers (parameters referenced by their type name —
/// the type already appears in the signature) and field-access names
/// (product components — declared where the product's type is) add
/// nothing a signature didn't, and chasing them would invite spurious
/// loads.
fn collect_expr_refs(expr: &Expr, skip: &HashSet<&str>, out: &mut Refs) {
    match expr {
        Expr::Constructor { name, args, .. } => {
            add_ref(&name.name, name.span, skip, out);
            for a in args {
                collect_expr_refs(a, skip, out);
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            type_args,
            args,
            ..
        } => {
            add_ref(&method.name, method.span, skip, out);
            collect_expr_refs(receiver, skip, out);
            for t in type_args {
                collect_ty_refs(t, skip, out);
            }
            for a in args {
                collect_expr_refs(a, skip, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_refs(scrutinee, skip, out);
            for MatchArm {
                param_ty,
                return_ty,
                body,
                ..
            } in arms
            {
                collect_ty_refs(param_ty, skip, out);
                collect_ty_refs(return_ty, skip, out);
                for e in &body.exprs {
                    collect_expr_refs(e, skip, out);
                }
            }
        }
        Expr::Try { inner, .. } | Expr::Await { inner, .. } => collect_expr_refs(inner, skip, out),
        Expr::Lambda {
            params,
            return_ty,
            body,
            ..
        } => {
            for p in params {
                collect_ty_refs(&p.ty, skip, out);
            }
            collect_ty_refs(return_ty, skip, out);
            for e in &body.exprs {
                collect_expr_refs(e, skip, out);
            }
        }
        Expr::ProductValue { fields, .. } => {
            for f in fields {
                collect_expr_refs(f, skip, out);
            }
        }
        Expr::FieldAccess { receiver, .. } => collect_expr_refs(receiver, skip, out),
        Expr::JsonLit { parts, .. } => {
            for p in parts {
                if let JsonLitPart::Interp(e) = p {
                    collect_expr_refs(e, skip, out);
                }
            }
        }
        Expr::Ident(_)
        | Expr::StringLit { .. }
        | Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::HexLit { .. } => {}
    }
}

/// One place a discovered name resolved to.
enum Found {
    Local(PathBuf),
    Bundled(&'static BundledPackage, &'static BundledFile),
}

impl Found {
    fn label(&self) -> String {
        match self {
            Found::Local(p) => p.display().to_string(),
            Found::Bundled(pkg, file) => format!("{}/{} (bundled)", pkg.name, file.path),
        }
    }
}

/// Resolve every name `items` references but the module doesn't define.
/// `dir` is the referencing file's directory — the root of the local
/// search. Loading a discovered file recursively discovers *its*
/// references, so a single type reference pulls in the whole closure.
fn discover_references(items: &[Item], dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let mut refs = Refs::new();
    for item in items {
        collect_item_refs(item, &mut refs);
    }
    for (name, span) in refs {
        if is_undiscoverable(&name) || ctx.defined.contains(&name) {
            continue;
        }
        resolve_reference(&name, span, dir, ctx)?;
    }
    Ok(())
}

fn resolve_reference(name: &str, span: Span, dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let mut found: Vec<Found> = Vec::new();

    // 1. Local tree — name → file convention.
    let stem = kebab_case(name);
    for path in ctx.local_stem_matches(dir, &stem) {
        found.push(Found::Local(path));
    }

    // 2. Project `bindgen/` — where `canon install` materializes the
    //    bindings declared in the manifest's `[imports]` table.
    for path in ctx.bindgen_decl_matches(name) {
        found.push(Found::Local(path));
    }

    // 3. Vendored dependencies under `deps/`.
    for path in ctx.deps_decl_matches(name) {
        found.push(Found::Local(path));
    }

    // 4. Bundled packages. Wrapper (`src/`) declarations shadow the
    //    package's own bindgen substrate — that split is an internal
    //    layering detail of the package, not a user-visible ambiguity.
    for (pkg, file) in bundled_decl_matches(name, false) {
        found.push(Found::Bundled(pkg, file));
    }

    // De-duplicate: a path can be reachable through more than one root
    // (e.g. a deps file is also inside the referencing file's tree when
    // the referencing file itself lives under `deps/`).
    let mut unique: Vec<Found> = Vec::new();
    let mut seen_labels: HashSet<String> = HashSet::new();
    for f in found {
        let key = match &f {
            Found::Local(p) => p
                .canonicalize()
                .unwrap_or_else(|_| p.clone())
                .display()
                .to_string(),
            Found::Bundled(pkg, file) => format!("{}/{}", pkg.name, file.path),
        };
        if seen_labels.insert(key) {
            unique.push(f);
        }
    }

    // A candidate that's already loaded means this reference was
    // resolved by a file currently mid-load (mutual references) — the
    // name lands in the module either way, so there's nothing to do.
    let already_loaded = unique.iter().any(|f| match f {
        Found::Local(p) => {
            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
            ctx.seen.contains(&canonical)
        }
        Found::Bundled(pkg, file) => ctx
            .seen_bundled
            .contains(&format!("{}/{}", pkg.name, file.path)),
    });
    if already_loaded {
        return Ok(());
    }

    match unique.len() {
        0 => Ok(()),
        1 => match unique.remove(0) {
            Found::Local(p) => {
                let canonical = p.canonicalize().map_err(|err| CanonError::CheckError {
                    message: format!("could not resolve `{}`: {}", p.display(), err),
                    span,
                })?;
                load_into(&canonical, ctx)
            }
            Found::Bundled(pkg, file) => {
                let key = format!("{}/{}", pkg.name, file.path);
                if ctx.seen_bundled.insert(key) {
                    load_bundled_source(pkg, file, ctx)?;
                }
                Ok(())
            }
        },
        _ => {
            let labels: Vec<String> = unique.iter().map(Found::label).collect();
            Err(CanonError::CheckError {
                message: format!(
                    "`{}` is ambiguous: it resolves to `{}` (names are globally unique \
                     across a project, its dependencies, and the standard library — \
                     rename one side)",
                    name,
                    labels.join("`, `"),
                ),
                span,
            })
        }
    }
}

impl LoadCtx {
    /// Files in the tree rooted at `dir` whose stem is `stem` (or
    /// `<stem>/main.can`). The scan is recursive, skips `deps/`,
    /// `bindgen/`, `target/`, and hidden directories, and is cached per
    /// root.
    fn local_stem_matches(&mut self, dir: &Path, stem: &str) -> Vec<PathBuf> {
        let root = dir.to_path_buf();
        let index = self
            .local_stems
            .entry(root.clone())
            .or_insert_with(|| build_stem_index(&root));
        index.get(stem).cloned().unwrap_or_default()
    }

    fn bindgen_decl_matches(&mut self, name: &str) -> Vec<PathBuf> {
        let Some(root) = self.project_root.as_ref().map(|r| r.join("bindgen")) else {
            return Vec::new();
        };
        let index = self
            .bindgen_decls
            .get_or_insert_with(|| build_decl_index(&root));
        index.get(name).cloned().unwrap_or_default()
    }

    fn deps_decl_matches(&mut self, name: &str) -> Vec<PathBuf> {
        let Some(root) = self.deps_dir.clone() else {
            return Vec::new();
        };
        let index = self
            .deps_decls
            .get_or_insert_with(|| build_decl_index(&root));
        index.get(name).cloned().unwrap_or_default()
    }
}

const SKIPPED_DIR_NAMES: &[&str] = &["bindgen", "deps", "target"];

/// Recursive file-stem index over a local tree: `foo.can` registers
/// under `foo`; `foo/main.can` additionally registers under `foo`
/// (the module-directory form). Entries are visited in sorted order so
/// candidate lists are deterministic.
fn build_stem_index(root: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let file_name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if file_name.starts_with('.') || SKIPPED_DIR_NAMES.contains(&file_name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if let Some(stem) = file_name.strip_suffix(".can") {
                if stem == "main" {
                    if let Some(parent) = path
                        .parent()
                        .filter(|p| *p != root)
                        .and_then(|p| p.file_name())
                    {
                        map.entry(parent.to_string_lossy().to_string())
                            .or_default()
                            .push(path.clone());
                    }
                }
                map.entry(stem.to_string()).or_default().push(path);
            }
        }
    }
    map
}

/// Declaration index over an on-disk tree: every top-level name each
/// `.can` file declares, mapped to the declaring files. Files that fail
/// to parse contribute nothing — they'll error properly if and when
/// they're actually loaded.
fn build_decl_index(root: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "can") {
                let Ok(src) = fs::read_to_string(&path) else {
                    continue;
                };
                for name in declared_names_of_source(&src) {
                    map.entry(name).or_default().push(path.clone());
                }
            }
        }
    }
    map
}

/// Top-level names a source declares. Runs the real parser — bindings
/// files' function-type aliases are `TypeDef`s pre-rewrite, so the name
/// set is the same either side of `apply_bindings_directive`.
fn declared_names_of_source(source: &str) -> Vec<String> {
    let Ok(tokens) = Scanner::new(source).scan_tokens() else {
        return Vec::new();
    };
    let Ok(module) = Parser::new(tokens).parse() else {
        return Vec::new();
    };
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::TypeDef(td) => Some(td.name.name.clone()),
            Item::Function(f) => Some(f.name.name.clone()),
            _ => None,
        })
        .collect()
}

/// Global declaration index over every bundled file:
/// name → (package index, file index) pairs. Built once per process.
fn bundled_decl_index() -> &'static HashMap<String, Vec<(usize, usize)>> {
    static INDEX: OnceLock<HashMap<String, Vec<(usize, usize)>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (pi, pkg) in BUNDLED_PACKAGES.iter().enumerate() {
            for (fi, file) in pkg.files.iter().enumerate() {
                for name in declared_names_of_source(file.source) {
                    let entry = map.entry(name).or_default();
                    if !entry.contains(&(pi, fi)) {
                        entry.push((pi, fi));
                    }
                }
            }
        }
        map
    })
}

/// Bundled files declaring `name`. A bundled package is two trees
/// flattened into one namespace: hand-written wrappers (`src/`,
/// `wit_urn == None`) and generated bindings (`bindgen/`,
/// `wit_urn == Some`). When a name is declared in both, the referrer's
/// own tier wins: user code and wrappers see the wrapper (`Request`
/// means the stdlib `Request`, not the raw resource newtype), while a
/// bindings file referencing a sibling interface's type stays inside
/// the bindings substrate.
fn bundled_decl_matches(
    name: &str,
    referrer_is_bindgen: bool,
) -> Vec<(&'static BundledPackage, &'static BundledFile)> {
    let Some(hits) = bundled_decl_index().get(name) else {
        return Vec::new();
    };
    let resolve = |&(pi, fi): &(usize, usize)| {
        let pkg = &BUNDLED_PACKAGES[pi];
        (pkg, &pkg.files[fi])
    };
    let tier: Vec<_> = hits
        .iter()
        .map(resolve)
        .filter(|(_, f)| f.wit_urn.is_some() == referrer_is_bindgen)
        .collect();
    if !tier.is_empty() {
        return tier;
    }
    hits.iter().map(resolve).collect()
}

/// Load the transitive import closure of `items` — a parsed (possibly
/// unsaved) buffer — the way `load_module` would, rooted at `dir`.
/// Tooling entry point (the LSP): resolution errors are swallowed
/// because a half-typed buffer shouldn't lose every diagnostic to one
/// ambiguous name.
pub fn load_import_closure(items: &[Item], dir: &Path) -> Vec<Item> {
    let project_root = find_project_root(dir);
    let project_install_index = project_root
        .as_ref()
        .map(|root| root.join("bindgen").join(INSTALL_INDEX_FILENAME))
        .filter(|p| p.is_file())
        .and_then(|p| fs::read_to_string(&p).ok())
        .and_then(|src| install::parse_install_index(&src).ok());
    let deps_dir = project_root
        .as_deref()
        .unwrap_or(dir)
        .join("deps")
        .canonicalize()
        .ok();
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_bundled: HashSet::new(),
        items: Vec::new(),
        defined: HashSet::new(),
        local_stems: HashMap::new(),
        bindgen_decls: None,
        deps_decls: None,
        local_sources: Vec::new(),
        project_root,
        project_install_index,
        deps_dir,
        deps_versions: HashMap::new(),
    };
    register_defined_names(items, &mut ctx);
    let _ = inject_json_prelude(items, &mut ctx);
    let _ = discover_references(items, dir, &mut ctx);
    ctx.items
}

/// Bundled files declaring `name`, wrapper tier first. Tooling helper
/// (go-to-definition into the shipped sources).
pub fn bundled_files_declaring(name: &str) -> Vec<&'static BundledFile> {
    bundled_decl_matches(name, false)
        .into_iter()
        .map(|(_, file)| file)
        .collect()
}

/// Discovery for a file inside a bundled package: no filesystem, so the
/// only root is the bundled registry itself, tiered by the referrer.
fn discover_bundled_references(
    items: &[Item],
    current: &'static BundledFile,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let referrer_is_bindgen = current.wit_urn.is_some();
    let mut refs = Refs::new();
    for item in items {
        collect_item_refs(item, &mut refs);
    }
    for (name, span) in refs {
        if is_undiscoverable(&name) || ctx.defined.contains(&name) {
            continue;
        }
        let candidates = bundled_decl_matches(&name, referrer_is_bindgen);
        let already_loaded = candidates.iter().any(|(pkg, file)| {
            ctx.seen_bundled
                .contains(&format!("{}/{}", pkg.name, file.path))
        });
        if already_loaded {
            continue;
        }
        match candidates.len() {
            0 => {}
            1 => {
                let (pkg, file) = candidates[0];
                let key = format!("{}/{}", pkg.name, file.path);
                if ctx.seen_bundled.insert(key) {
                    load_bundled_source(pkg, file, ctx)?;
                }
            }
            _ => {
                let labels: Vec<String> = candidates
                    .iter()
                    .map(|(pkg, file)| format!("{}/{}", pkg.name, file.path))
                    .collect();
                return Err(CanonError::CheckError {
                    message: format!(
                        "`{}` is ambiguous inside the bundled packages: it resolves to `{}`",
                        name,
                        labels.join("`, `"),
                    ),
                    span,
                });
            }
        }
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
    let deps_pkg = deps_pkg_for_file(path, ctx)?;
    load_source(
        &source,
        path.parent().unwrap_or_else(|| Path::new(".")),
        wit_urn.as_deref(),
        deps_pkg.as_ref(),
        ctx,
    )
}

/// When `path` lives under the project's `deps/` tree, compute the
/// vendored package it belongs to: the expected coordinate key
/// (`"<ns>:<name>"`, from the first two path components under `deps/`)
/// plus a stable display label (`"deps/<ns>/<name>/<file>.can"`) for
/// error messages. Returns `None` for ordinary project files.
///
/// Both `path` and `ctx.deps_dir` are canonicalized by their producers,
/// so the prefix test is a plain component comparison.
fn deps_pkg_for_file(path: &Path, ctx: &LoadCtx) -> Result<Option<(String, String)>> {
    let Some(deps) = &ctx.deps_dir else {
        return Ok(None);
    };
    let Ok(rel) = path.strip_prefix(deps) else {
        return Ok(None);
    };
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let label = format!("deps/{rel_str}");
    let components: Vec<&str> = rel_str.split('/').collect();
    if components.len() < 3 {
        // `deps/foo.can` or `deps/acme/foo.can` — no `<ns>/<name>/`
        // directory to belong to, so no coordinate can be right.
        return Err(CanonError::CheckError {
            message: format!("vendored file `{label}` must live under `deps/<namespace>/<name>/`"),
            span: Span::default(),
        });
    }
    let key = format!("{}:{}", components[0], components[1]);
    Ok(Some((key, label)))
}

/// Split a package coordinate `"<ns>:<name>@<version>"` into its three
/// parts. Namespace and name are lowercase kebab identifiers (matching
/// the OCI / wkg package-name grammar PACKAGES.md adopts); the version
/// is any non-empty semver-ish string (`1.2.3`,
/// `0.3.0-rc-2026-03-15`, …).
fn parse_package_coordinate(s: &str) -> Option<(&str, &str, &str)> {
    let (left, version) = s.split_once('@')?;
    let (ns, name) = left.split_once(':')?;
    let seg_ok = |seg: &str| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let ver_ok = !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+');
    (seg_ok(ns) && seg_ok(name) && ver_ok).then_some((ns, name, version))
}

/// Enforce the `package` directive rules from PACKAGES.md on one file's
/// parsed items.
///
/// `deps_pkg` is `Some((expected_key, label))` when the file is
/// vendored (lives under `deps/`), `None` otherwise. Outside `deps/`
/// the directive is forbidden — a project's own code has no version;
/// publication gives it one. Inside `deps/` exactly one directive is
/// required, it must be the file's first declaration, its coordinate
/// must be well-formed and match the `deps/<ns>/<name>/` directory the
/// file lives in, and every file of one vendored package must agree on
/// the version (tracked in `ctx.deps_versions`).
fn validate_package_directives(
    items: &[Item],
    deps_pkg: Option<&(String, String)>,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let decls: Vec<(usize, &PackageDecl)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::Package(p) => Some((i, p)),
            _ => None,
        })
        .collect();

    let Some((expected_key, label)) = deps_pkg else {
        if let Some((_, decl)) = decls.first() {
            return Err(CanonError::CheckError {
                message: "the `package` directive is only allowed in vendored files under `deps/`"
                    .to_string(),
                span: decl.span,
            });
        }
        return Ok(());
    };

    let Some(&(idx, decl)) = decls.first() else {
        return Err(CanonError::CheckError {
            message: format!("vendored file `{label}` is missing its `package` directive"),
            span: Span::default(),
        });
    };
    if decls.len() > 1 {
        return Err(CanonError::CheckError {
            message: format!("duplicate `package` directive in `{label}`"),
            span: decls[1].1.span,
        });
    }
    if idx != 0 {
        return Err(CanonError::CheckError {
            message: format!("the `package` directive must be the first declaration in `{label}`"),
            span: decl.span,
        });
    }
    let Some((ns, name, version)) = parse_package_coordinate(&decl.coordinate) else {
        return Err(CanonError::CheckError {
            message: format!(
                "malformed package coordinate `{}` in `{label}` (expected `\"<namespace>:<name>@<version>\"`)",
                decl.coordinate,
            ),
            span: decl.span,
        });
    };
    let key = format!("{ns}:{name}");
    if &key != expected_key {
        return Err(CanonError::CheckError {
            message: format!(
                "`package \"{}\"` in `{label}` does not match its directory `deps/{}/`",
                decl.coordinate,
                expected_key.replace(':', "/"),
            ),
            span: decl.span,
        });
    }
    match ctx.deps_versions.get(&key) {
        Some((seen_version, seen_label)) if seen_version != version => {
            Err(CanonError::CheckError {
                message: format!(
                    "vendored package `{key}` has conflicting versions: `{seen_version}` (in `{seen_label}`) and `{version}` (in `{label}`)",
                ),
                span: decl.span,
            })
        }
        Some(_) => Ok(()),
        None => {
            ctx.deps_versions
                .insert(key, (version.to_string(), label.clone()));
            Ok(())
        }
    }
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

fn load_source(
    source: &str,
    dir: &Path,
    wit_urn: Option<&str>,
    deps_pkg: Option<&(String, String)>,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    // Bindings rewrite runs before resolve_new_syntax so the produced
    // FunctionDefs go through the same PascalCase / trait-impl /
    // constructor normalisation as hand-written functions.
    apply_bindings_directive(&mut module.items);
    resolve_new_syntax(&mut module);
    validate_package_directives(&module.items, deps_pkg, ctx)?;

    let mut other_items = module.items;
    register_defined_names(&other_items, ctx);
    inject_json_prelude(&other_items, ctx)?;
    discover_references(&other_items, dir, ctx)?;
    if let Some(urn) = wit_urn {
        patch_extern_paths(&mut other_items, urn);
    }
    ctx.items.extend(other_items);
    Ok(())
}

/// Load a bundled file's source. References inside the source resolve
/// against the bundled registry only — the bundle is in-memory, so
/// there are no local-disk roots to search.
fn load_bundled_source(
    _pkg: &'static BundledPackage,
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

    let mut other_items = module.items;
    register_defined_names(&other_items, ctx);
    discover_bundled_references(&other_items, current, ctx)?;
    if let Some(urn) = current.wit_urn {
        patch_extern_paths(&mut other_items, urn);
    }
    ctx.items.extend(other_items);
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
