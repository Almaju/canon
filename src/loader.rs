use crate::ast::{resolve_new_syntax, Item, Module};
use crate::error::{OnewayError, Result, Span};
use crate::lexer::Scanner;
use crate::manifest::{self, Manifest};
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Bundled packages
// ---------------------------------------------------------------------------
//
// The shipped packages (`oneway/std`, `oneway/wasi`, …) are baked into the
// compiler binary at build time by `build.rs`, which walks `packages/` and
// emits a flat registry as `bundled_packages.rs`. The registry replaces what
// used to be hand-maintained `STDLIB` and `WASI_BINDINGS` arrays — drop a new
// file under `packages/<ns>/<pkg>/` and the next `cargo build` picks it up.

/// One package shipped with the compiler.
#[derive(Debug, Clone, Copy)]
pub struct BundledPackage {
    /// Canonical name, e.g. `"oneway/std"`. Matches the package's
    /// declared `name` in its `oneway.toml`.
    pub name: &'static str,
    /// The full `oneway.toml` source, parsed lazily on first use.
    pub manifest_src: &'static str,
    /// Every `.ow` file under the package root, sorted alphabetically by
    /// package-relative path.
    pub files: &'static [BundledFile],
}

/// One file inside a bundled package.
#[derive(Debug, Clone, Copy)]
pub struct BundledFile {
    /// Path relative to the package root, e.g. `"clocks/monotonic_clock.ow"`.
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

/// Find a bundled package by its canonical name (`"oneway/std"`).
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
/// kebab-casing the final type-name segment to find its `.ow` file.
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
    // file name and gets kebab-cased before we append `.ow`.
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
        // This covers `use oneway/wasi/clocks/monotonic_clock`.
        (*last).to_string()
    };
    rel.push_str(&stem);
    rel.push_str(".ow");

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

/// A user-authored Oneway source file as the loader saw it on disk.
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
}

pub fn load_module(entry: &Path) -> Result<LoadResult> {
    let canonical = entry
        .canonicalize()
        .map_err(|err| OnewayError::CheckError {
            message: format!("could not resolve `{}`: {}", entry.display(), err),
            span: Span::default(),
        })?;
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_bundled: HashSet::new(),
        items: Vec::new(),
        local_sources: Vec::new(),
    };
    let source = fs::read_to_string(&canonical).map_err(|err| OnewayError::CheckError {
        message: format!("could not read `{}`: {}", canonical.display(), err),
        span: Span::default(),
    })?;
    ctx.seen.insert(canonical.to_path_buf());
    ctx.local_sources.push(LoadedSource {
        path: canonical.to_path_buf(),
        source: source.clone(),
    });
    let dir = canonical.parent().unwrap_or_else(|| Path::new("."));
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
/// ordering rules to user-authored code.
fn load_entry_source(source: &str, dir: &Path, ctx: &mut LoadCtx) -> Result<usize> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
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
        process_use(&u, dir, ctx)?;
    }
    let start = ctx.items.len();
    ctx.items.extend(other_items);
    Ok(start)
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

    // Local file/module lookup.
    let segments: Vec<&str> = path_str.split('/').collect();
    let type_name = segments[segments.len() - 1];
    let file_stem = kebab_case(type_name);
    let mut file_dir = dir.to_path_buf();
    for seg in &segments[..segments.len() - 1] {
        file_dir = file_dir.join(seg);
    }

    let candidate = file_dir.join(format!("{}.ow", file_stem));
    let module_candidate = file_dir.join(&file_stem).join("main.ow");

    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|err| OnewayError::CheckError {
                message: format!("could not resolve `{}`: {}", candidate.display(), err),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
    } else if module_candidate.exists() {
        let canonical = module_candidate
            .canonicalize()
            .map_err(|err| OnewayError::CheckError {
                message: format!(
                    "could not resolve `{}`: {}",
                    module_candidate.display(),
                    err
                ),
                span: u.span,
            })?;
        load_into(&canonical, ctx)?;
    } else {
        return Err(OnewayError::CheckError {
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
    let source = fs::read_to_string(path).map_err(|err| OnewayError::CheckError {
        message: format!("could not read `{}`: {}", path.display(), err),
        span: Span::default(),
    })?;
    ctx.local_sources.push(LoadedSource {
        path: path.to_path_buf(),
        source: source.clone(),
    });
    load_source(
        &source,
        path.parent().unwrap_or_else(|| Path::new(".")),
        ctx,
    )
}

fn load_source(source: &str, dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
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
/// 2. Otherwise, walk the path relative to the importing file's own
///    directory within its package. `use Foo` → `<dir>/foo.ow`,
///    `use sub/Foo` → `<dir>/sub/foo.ow`.
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

    // Same-package, possibly nested. Compute the importing file's directory
    // within its package and prepend it to the use path.
    let current_dir = current
        .path
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default();

    let segments: Vec<&str> = path_str.split('/').collect();
    let (last, dirs) = segments.split_last().expect("split_last on non-empty Vec");
    let mut rel = current_dir;
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
    rel.push_str(".ow");

    let Some(file) = bundled_file(pkg, &rel) else {
        return Err(OnewayError::CheckError {
            message: format!(
                "`use {}` from package `{}` not found (looked for `{}`)",
                path_str, pkg.name, rel
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
