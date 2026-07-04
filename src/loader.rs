use crate::ast::{
    extract_receiver_from_params, resolve_new_syntax, Block, Expr, ExternWasm, FunctionDef, Item,
    Module, Param, TypeExpr,
};
use crate::bindgen;
use crate::error::{CanonError, Result, Span};
use crate::lexer::Scanner;
use crate::manifest::{self, Manifest};
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Walk `items` and rewrite function-type aliases into real
/// `Item::Function`s with `extern_wasm` populated, when the file is a
/// binding file — recognized by *shape and path*, never by a header
/// (PACKAGES.md slice 8): `seed_urn` is the WIT interface URN the
/// file's vendored path spells (`deps/<ns>/<name>@<ver>/<iface>.can`),
/// and `None` means the file is ordinary source and nothing is
/// rewritten.
///
/// Rewrite rules, given a seed:
///   * A camelCase alias whose first parameter is a resource declared
///     in the same file (`X = Handle`) is that resource's method —
///     `set = (Headers * String * String) -> Headers` derives
///     `#[method]headers.set`.
///   * A PascalCase alias named exactly like an in-file resource is
///     its constructor — `Headers = () -> Headers` derives
///     `#[constructor]headers`.
///   * Any other camelCase alias is a plain interface function
///     (`#<kebab(name)>`).
///   * Any other PascalCase function-type alias stays an ordinary
///     callback type (`Handler = (Request) -> Response`) — hijacking
///     it into an extern would corrupt vendored Canon-source packages.
///     The case distinction is already load-bearing in the language
///     (types vs. functions).
///   * The first product component becomes the receiver for camelCase
///     declarations; PascalCase declarations and zero-arg functions
///     skip the receiver extraction.
///   * Async-ness is read off the return type: a function whose return
///     is `Future<T>` is async (the canonical-ABI lowering uses
///     `[async-lower]`); everything else is sync. No `async` keyword.
///     This keeps the source consistent with the principle "types tell
///     the story" — the function's effect is visible in its signature.
///
/// Public so tests can drive the rewrite with a synthetic seed,
/// exactly as the loader does for a vendored path.
pub fn apply_bindings(items: &mut [Item], seed_urn: Option<&str>) {
    let Some(base) = seed_urn else { return };

    // Resource types declared in this file (`X = Handle` newtypes).
    // The rules consult them to spell WIT resource fragments
    // (`[method]`, `[constructor]`) mechanically.
    let resources: HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            Item::TypeDef(td) => match &td.body {
                TypeExpr::Named { name, generics, .. }
                    if name == "Handle" && generics.is_empty() =>
                {
                    Some(td.name.name.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    for item in items.iter_mut() {
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

        let starts_lower = td.name.name.chars().next().is_some_and(char::is_lowercase);

        let path = if starts_lower {
            // A resource-typed first parameter marks a resource method;
            // anything else is a plain interface function.
            let receiver_resource = params.first().and_then(|p| match p {
                TypeExpr::Named { name, .. } if resources.contains(name) => Some(name),
                _ => None,
            });
            let fragment = match receiver_resource {
                Some(res) => format!(
                    "[method]{}.{}",
                    bindgen::camel_to_kebab(res),
                    bindgen::camel_to_kebab(&td.name.name),
                ),
                None => bindgen::camel_to_kebab(&td.name.name),
            };
            format!("{base}#{fragment}")
        } else if resources.contains(&td.name.name) {
            // A PascalCase alias named like an in-file resource is its
            // constructor.
            format!(
                "{}#[constructor]{}",
                base,
                bindgen::camel_to_kebab(&td.name.name),
            )
        } else {
            // Any other PascalCase alias stays a type alias (the
            // ordinary Canon callback-type syntax).
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

        let (receiver, recv_mut, final_params) = if !starts_lower || new_params.is_empty() {
            // PascalCase declarations (constructors) and zero-arg
            // functions don't take a receiver — the parser does the
            // same for ordinary FunctionDefs.
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

/// Find a bundled file, resolving version-carrying directory names:
/// `use wasi/random/random` (source never states versions) matches the
/// on-disk `wasi/random@0.3.0-rc-…/random.can`. An exact match wins;
/// otherwise a path whose directory segments differ only by an
/// `@<version>` suffix matches. The bundled tree ships with the
/// compiler, so at most one version of a package exists by
/// construction and the first match is the match.
fn bundled_file_versioned(pkg: &BundledPackage, rel_path: &str) -> Option<&'static BundledFile> {
    bundled_file(pkg, rel_path).or_else(|| {
        let want: Vec<&str> = rel_path.split('/').collect();
        pkg.files.iter().find(|f| {
            let actual: Vec<&str> = f.path.split('/').collect();
            actual.len() == want.len()
                && actual
                    .iter()
                    .zip(&want)
                    .all(|(a, w)| a == w || a.split_once('@').is_some_and(|(left, _)| left == *w))
        })
    })
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

struct LoadCtx {
    seen: HashSet<PathBuf>,
    /// Deduplicates bundled imports so a single package is loaded once even
    /// when multiple files transitively `use` it. Keyed by absolute bundled
    /// file path (`pkg.name + "/" + file.path`).
    seen_bundled: HashSet<String>,
    items: Vec<Item>,
    /// User-authored sources accumulated during load (entry + transitive
    /// local `use` imports). Mirrors `seen` but keeps each file's full
    /// text so callers can validate canonical formatting later.
    local_sources: Vec<LoadedSource>,
    /// Canonicalized path of the project's vendored-dependency tree
    /// (`<root>/deps/`, see PACKAGES.md), when it exists on disk. The
    /// root is the project root when there is one, otherwise the entry
    /// file's directory — so manifest-free projects (the PACKAGES.md
    /// end state) resolve `deps/` without an `canon.toml` marker.
    ///
    /// `Some` enables two things: `process_use` resolves use paths
    /// against `deps/<ns>/<name>@<version>/…` (the directory name is
    /// the pin — identity lives in the path, not in a directive), and
    /// `load_into` recognizes files under this prefix as vendored,
    /// deriving each top-level file's binding URN from its path.
    deps_dir: Option<PathBuf>,
    /// Canonicalized `<project_root>/bindgen/` — the directory the
    /// manifest-driven `canon install` materializes into. Same
    /// versioned layout and path-derived binding rules as `deps_dir`;
    /// the separate root survives only until PACKAGES.md slice 6
    /// deletes the manifest flow.
    bindgen_dir: Option<PathBuf>,
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
    let deps_dir = project_root
        .as_deref()
        .unwrap_or(dir)
        .join("deps")
        .canonicalize()
        .ok();
    let bindgen_dir = project_root
        .as_ref()
        .map(|root| root.join("bindgen"))
        .and_then(|p| p.canonicalize().ok());
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_bundled: HashSet::new(),
        items: Vec::new(),
        local_sources: Vec::new(),
        deps_dir,
        bindgen_dir,
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
    // The entry file is never a binding file (binding files live in
    // vendored package directories), so no bindings rewrite applies.
    resolve_new_syntax(&mut module);

    let mut use_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Use(u) => use_items.push(u),
            other => other_items.push(other),
        }
    }
    inject_json_prelude(&use_items, &other_items, dir, ctx)?;
    for u in use_items {
        process_use(&u, dir, ctx)?;
    }
    let start = ctx.items.len();
    ctx.items.extend(other_items);
    Ok(start)
}

/// JSON prelude: `canon/std/Json` loads automatically — like Rust's
/// prelude, JSON support doesn't need an explicit import. A fully static
/// JSON literal is constant-folded and needs nothing at all (the checker
/// knows `Json = String` intrinsically), so the stdlib module is pulled
/// in only when the program actually reaches for its machinery:
/// interpolation inside a literal (`{"n":Int}` converts via `ToJson`),
/// the validating `Json(...)` constructor, or an explicit `.ToJson()` /
/// `.Json()` call. Skipped when the file imports or defines `Json`
/// itself.
fn inject_json_prelude(
    use_items: &[crate::ast::UseDecl],
    other_items: &[Item],
    dir: &Path,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let already_in_scope = use_items
        .iter()
        .any(|u| u.name.name == "Json" || u.name.name.ends_with("/Json"))
        || other_items.iter().any(|item| match item {
            Item::TypeDef(td) => td.name.name == "Json",
            Item::Function(f) => f.name.name == "Json" && f.extern_wasm.is_some(),
            _ => false,
        });
    if already_in_scope || !items_use_json_machinery(other_items) {
        return Ok(());
    }
    let synthetic = crate::ast::UseDecl {
        name: crate::ast::Ident {
            name: "canon/std/Json".to_string(),
            span: Span::default(),
        },
        span: Span::default(),
    };
    process_use(&synthetic, dir, ctx)
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

    // Vendored-dependency lookup (PACKAGES.md slices 1 + 7). A use path
    // with at least three segments (`<ns>/<name>/…`, mirroring the
    // bundled rule) also resolves against the versioned package
    // directory `<deps>/<ns>/<name>@<version>/…` — the version lives in
    // the directory name (never in source), remaining segments as
    // directory names, the final type-name segment kebab-cased to its
    // file stem.
    let deps_candidate: Option<PathBuf> =
        resolve_vendored_use(ctx.deps_dir.as_deref(), path_str, u.span)?;

    // Project `bindgen/` lookup. When the entry file lives inside a
    // project (an ancestor directory has `canon.toml`), `use` paths
    // also resolve against `<project_root>/bindgen/` — the directory
    // where the manifest-driven `canon install` materializes external
    // bindings declared in `[imports]`. Same versioned layout and
    // resolution rule as `deps/`. Sits between the bundled-package
    // check and the local-relative fallback so bindgen output is
    // reachable from any source file in the project without overriding
    // either bundled std lookups or in-project sibling files.
    let bindgen_candidate: Option<PathBuf> =
        resolve_vendored_use(ctx.bindgen_dir.as_deref(), path_str, u.span)?;

    // Local file/module candidates.
    let segments: Vec<&str> = path_str.split('/').collect();
    let type_name = segments[segments.len() - 1];
    let file_stem = kebab_case(type_name);
    let mut file_dir = dir.to_path_buf();
    for seg in &segments[..segments.len() - 1] {
        file_dir = file_dir.join(seg);
    }

    let candidate = file_dir.join(format!("{}.can", file_stem));
    let module_candidate = file_dir.join(&file_stem).join("main.can");

    // A `deps/` hit must be the *only* hit: PACKAGES.md's resolution
    // rule is "ambiguity is a hard error, not a precedence", and slice 1
    // applies it wherever a vendored package is involved. (The
    // pre-existing bindgen-before-local precedence for paths that never
    // touch `deps/` is unchanged; the full no-precedence rule lands with
    // the `use` removal.)
    if let Some(deps_file) = deps_candidate {
        let mut clashes: Vec<String> = Vec::new();
        if let Some(b) = &bindgen_candidate {
            clashes.push(b.display().to_string());
        }
        if candidate.exists() {
            clashes.push(candidate.display().to_string());
        }
        if module_candidate.exists() {
            clashes.push(module_candidate.display().to_string());
        }
        if !clashes.is_empty() {
            return Err(CanonError::CheckError {
                message: format!(
                    "`use {}` is ambiguous: it resolves to the vendored `{}` and also to `{}`",
                    path_str,
                    deps_file.display(),
                    clashes.join("`, `"),
                ),
                span: u.span,
            });
        }
        let canonical = deps_file
            .canonicalize()
            .map_err(|err| CanonError::CheckError {
                message: format!("could not resolve `{}`: {}", deps_file.display(), err),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
        return Ok(());
    }

    if let Some(bindgen_file) = bindgen_candidate {
        let canonical = bindgen_file
            .canonicalize()
            .map_err(|err| CanonError::CheckError {
                message: format!("could not resolve `{}`: {}", bindgen_file.display(), err),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
        return Ok(());
    }

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
    let deps_pkg = deps_pkg_for_file(path, ctx)?;
    load_source(
        &source,
        path.parent().unwrap_or_else(|| Path::new(".")),
        deps_pkg.as_ref(),
        ctx,
    )
}

/// A file's vendored-package context (it lives under `deps/`).
struct DepsPkg {
    /// The path-derived WIT interface URN,
    /// `"<ns>:<name>/<iface>@<version>"`, for a file sitting *directly*
    /// in the package directory. WIT interfaces are flat within a
    /// package, so only top-level files are binding files; files in
    /// nested directories are ordinary vendored source and get `None`.
    urn_base: Option<String>,
}

/// True for a lowercase-kebab package namespace or name segment
/// (matching the OCI / wkg package-name grammar PACKAGES.md adopts).
fn valid_pkg_seg(seg: &str) -> bool {
    !seg.is_empty()
        && seg
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// True for a non-empty semver-ish version string (`1.2.3`,
/// `0.3.0-rc-2026-03-15`, …).
fn valid_pkg_version(version: &str) -> bool {
    !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
}

/// Resolve a `use <ns>/<name>/…` path against a vendored tree — the
/// project's `deps/` or its `bindgen/` (PACKAGES.md slices 1 + 7 + 8).
/// The package directory carries the version
/// (`<root>/<ns>/<name>@<version>/`), so the lookup scans
/// `<root>/<ns>/` for the directory whose name-before-`@` matches:
///
///   * no match → `Ok(None)`, the caller falls through to the other
///     search locations;
///   * exactly one versioned match → the remaining `use` segments
///     resolve inside it, final segment kebab-cased to its file stem;
///   * a directory named `<name>` with no `@<version>` → hard error
///     (an unversioned vendor directory has no pin);
///   * more than one versioned match → hard error (at most one version
///     of a package per project — previously structural in the
///     unversioned layout, now a detected sibling conflict).
fn resolve_vendored_use(
    root: Option<&Path>,
    path_str: &str,
    span: Span,
) -> Result<Option<PathBuf>> {
    let Some(root) = root else {
        return Ok(None);
    };
    let label = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "deps".to_string());
    let segments: Vec<&str> = path_str.split('/').collect();
    if segments.len() < 3 {
        return Ok(None);
    }
    let (ns, name) = (segments[0], segments[1]);
    let ns_dir = root.join(ns);
    let Ok(entries) = fs::read_dir(&ns_dir) else {
        return Ok(None);
    };
    let mut unversioned = false;
    let mut matches: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        if dir_name == name {
            unversioned = true;
        } else if dir_name
            .split_once('@')
            .is_some_and(|(left, _)| left == name)
        {
            matches.push(dir_name);
        }
    }
    if unversioned {
        return Err(CanonError::CheckError {
            message: format!(
                "vendored package directory `{label}/{ns}/{name}/` is missing its version (expected `{label}/{ns}/{name}@<version>/`)",
            ),
            span,
        });
    }
    if matches.len() > 1 {
        matches.sort();
        return Err(CanonError::CheckError {
            message: format!(
                "vendored package `{ns}:{name}` is present at more than one version: `{label}/{ns}/{}` (at most one version of a package per project; remove the stale directory)",
                matches.join(&format!("/`, `{label}/{ns}/")),
            ),
            span,
        });
    }
    let Some(dir_name) = matches.into_iter().next() else {
        return Ok(None);
    };
    let rest = &segments[2..];
    let (last, dirs) = rest.split_last().expect("segments.len() >= 3");
    let mut p = ns_dir.join(dir_name);
    for d in dirs {
        p = p.join(d);
    }
    let stem = if last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        kebab_case(last)
    } else {
        (*last).to_string()
    };
    p = p.join(format!("{stem}.can"));
    Ok(p.is_file().then_some(p))
}

/// When `path` lives under one of the project's vendored trees
/// (`deps/` or `bindgen/`), parse the versioned package directory it
/// belongs to and derive the file's binding URN (see [`DepsPkg`]).
/// Identity is validated where it lives — the path — so a malformed
/// vendor layout is an error here, with nothing left for file contents
/// to get wrong. Returns `None` for ordinary project files.
///
/// Both `path` and the roots are canonicalized by their producers, so
/// the prefix test is a plain component comparison.
fn deps_pkg_for_file(path: &Path, ctx: &LoadCtx) -> Result<Option<DepsPkg>> {
    for root in [ctx.deps_dir.as_deref(), ctx.bindgen_dir.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Some(pkg) = vendored_pkg_for_file(path, root)? {
            return Ok(Some(pkg));
        }
    }
    Ok(None)
}

/// The per-root half of [`deps_pkg_for_file`].
fn vendored_pkg_for_file(path: &Path, root: &Path) -> Result<Option<DepsPkg>> {
    let Ok(rel) = path.strip_prefix(root) else {
        return Ok(None);
    };
    let root_label = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "deps".to_string());
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let label = format!("{root_label}/{rel_str}");
    let components: Vec<&str> = rel_str.split('/').collect();
    if components.len() < 3 {
        // `deps/foo.can` or `deps/acme/foo.can` — no package directory
        // to belong to, so no identity can be derived.
        return Err(CanonError::CheckError {
            message: format!(
                "vendored file `{label}` must live under `{root_label}/<namespace>/<name>@<version>/`"
            ),
            span: Span::default(),
        });
    }
    let (ns, dir) = (components[0], components[1]);
    let Some((name, version)) = dir.split_once('@') else {
        return Err(CanonError::CheckError {
            message: format!(
                "vendored package directory `{root_label}/{ns}/{dir}/` is missing its version (expected `{root_label}/{ns}/{dir}@<version>/`)",
            ),
            span: Span::default(),
        });
    };
    if !valid_pkg_seg(ns) || !valid_pkg_seg(name) || !valid_pkg_version(version) {
        return Err(CanonError::CheckError {
            message: format!(
                "malformed vendored package directory `{root_label}/{ns}/{dir}/` (expected `{root_label}/<namespace>/<name>@<version>/` with a lowercase kebab name and a semver-ish version)",
            ),
            span: Span::default(),
        });
    }
    let urn_base = (components.len() == 3).then(|| {
        let stem = components[2].trim_end_matches(".can").replace('_', "-");
        format!("{ns}:{name}/{stem}@{version}")
    });
    Ok(Some(DepsPkg { urn_base }))
}

/// Derive the binding-base URN a *bundled* file's package-relative path
/// spells, when it has the versioned shape
/// `<ns>/<name>@<version>/<iface>.can`. Lenient counterpart of
/// [`vendored_pkg_for_file`]: the bundled tree ships with the compiler,
/// so a non-versioned path is simply an ordinary source file, never an
/// error.
pub(crate) fn urn_base_for_bundled_path(rel: &str) -> Option<String> {
    let components: Vec<&str> = rel.split('/').collect();
    if components.len() != 3 {
        return None;
    }
    let (ns, dir, file) = (components[0], components[1], components[2]);
    let (name, version) = dir.split_once('@')?;
    if !valid_pkg_seg(ns) || !valid_pkg_seg(name) || !valid_pkg_version(version) {
        return None;
    }
    let stem = file.strip_suffix(".can")?.replace('_', "-");
    Some(format!("{ns}:{name}/{stem}@{version}"))
}

fn load_source(
    source: &str,
    dir: &Path,
    deps_pkg: Option<&DepsPkg>,
    ctx: &mut LoadCtx,
) -> Result<()> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    // Bindings rewrite runs before resolve_new_syntax so the produced
    // FunctionDefs go through the same PascalCase / trait-impl /
    // constructor normalisation as hand-written functions. A vendored
    // top-level file's path seeds the base URN — a binding file is
    // recognized by shape, not by a header.
    let seed_urn = deps_pkg.and_then(|p| p.urn_base.as_deref());
    apply_bindings(&mut module.items, seed_urn);
    resolve_new_syntax(&mut module);

    let mut use_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Use(u) => use_items.push(u),
            other => other_items.push(other),
        }
    }
    inject_json_prelude(&use_items, &other_items, dir, ctx)?;
    for u in use_items {
        process_use(&u, dir, ctx)?;
    }
    ctx.items.extend(other_items);
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
    // PascalCase normalisation pass. A bundled file at a versioned
    // package path (`wasi/random@0.3.0/random.can`) seeds its
    // path-derived URN, exactly like a vendored file under `deps/`.
    let seed_urn = urn_base_for_bundled_path(current.path);
    apply_bindings(&mut module.items, seed_urn.as_deref());
    resolve_new_syntax(&mut module);

    let mut use_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Use(u) => use_items.push(u),
            other => other_items.push(other),
        }
    }
    for u in use_items {
        process_bundled_use(pkg, current, &u, ctx)?;
    }
    ctx.items.extend(other_items);
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

    let file = bundled_file_versioned(pkg, &sibling_rel)
        .or_else(|| bundled_file_versioned(pkg, &root_rel));

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
