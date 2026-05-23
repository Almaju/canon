use crate::ast::{resolve_new_syntax, Item, Module};
use crate::error::{OnewayError, Result, Span};
use crate::lexer::Scanner;
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

struct StdlibEntry {
    name: &'static str,
    source: &'static str,
}

// The stdlib is structured as small, focused modules — each entry exposes a
// single public type (the loader looks up entries by the type name the user
// `use`s). Modules `use` each other to share supporting types; the loader
// de-duplicates so transitive imports are loaded once.
const STDLIB: &[StdlibEntry] = &[
    StdlibEntry {
        name: "Body",
        source: include_str!("../std/body.ow"),
    },
    // `Random` and `Clock` provide free functions (`randomInt`, `nowNanos`).
    // The loader keys them by their declared type but the modules don't
    // define a wrapper type — `use std/Clock` loads the clock externs and
    // `use std/Random` loads the random extern.
    StdlibEntry {
        name: "Clock",
        source: include_str!("../std/clock-wasm.ow"),
    },
    StdlibEntry {
        name: "File",
        source: include_str!("../std/filesystem-wasm.ow"),
    },
    StdlibEntry {
        name: "HttpError",
        source: include_str!("../std/http-error.ow"),
    },
    StdlibEntry {
        name: "HttpResponseBody",
        source: include_str!("../std/http-response-body.ow"),
    },
    StdlibEntry {
        name: "HttpServer",
        source: include_str!("../std/http-server-wasm.ow"),
    },
    StdlibEntry {
        name: "HttpStatus",
        source: include_str!("../std/http-status.ow"),
    },
    StdlibEntry {
        name: "InvalidUrl",
        source: include_str!("../std/url-wasm.ow"),
    },
    StdlibEntry {
        name: "IoError",
        source: include_str!("../std/io-error.ow"),
    },
    StdlibEntry {
        name: "Json",
        source: include_str!("../std/json-wasm.ow"),
    },
    StdlibEntry {
        name: "Now",
        source: include_str!("../std/now-wasm.ow"),
    },
    StdlibEntry {
        name: "Path",
        source: include_str!("../std/path-wasm.ow"),
    },
    StdlibEntry {
        name: "Port",
        source: include_str!("../std/port.ow"),
    },
    StdlibEntry {
        name: "Random",
        source: include_str!("../std/random-wasm.ow"),
    },
    StdlibEntry {
        name: "Request",
        source: include_str!("../std/request.ow"),
    },
    StdlibEntry {
        name: "RoutePath",
        source: include_str!("../std/route-path.ow"),
    },
    // Test framework: `use std/TestResult` brings in `TestResult`, `Fail`,
    // `Pass`, and the `assert` helper. The `oneway test` subcommand
    // discovers `() -> TestResult` functions and synthesises an entry point.
    StdlibEntry {
        name: "TestResult",
        source: include_str!("../std/test.ow"),
    },
    StdlibEntry {
        name: "Url",
        source: include_str!("../std/url-wasm.ow"),
    },
];

fn stdlib_entry(name: &str) -> Option<&'static StdlibEntry> {
    STDLIB.iter().find(|e| e.name == name)
}

pub struct LoadResult {
    pub module: Module,
    /// Index in `module.items` where items declared in the entry file
    /// begin. Items before this index were pulled in via `use` and are
    /// exempt from per-file ordering rules.
    pub entry_items_start: usize,
}

struct LoadCtx {
    seen: HashSet<PathBuf>,
    seen_stdlib: HashSet<String>,
    items: Vec<Item>,
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
        seen_stdlib: HashSet::new(),
        items: Vec::new(),
    };
    let source = fs::read_to_string(&canonical).map_err(|err| OnewayError::CheckError {
        message: format!("could not read `{}`: {}", canonical.display(), err),
        span: Span::default(),
    })?;
    ctx.seen.insert(canonical.to_path_buf());
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
    let segments: Vec<&str> = path_str.split('/').collect();
    let type_name = segments[segments.len() - 1];

    // `use std/TypeName` — look up in the embedded standard library.
    if segments.len() >= 2 && segments[0] == "std" {
        if let Some(entry) = stdlib_entry(type_name) {
            if ctx.seen_stdlib.insert(entry.name.to_string()) {
                let stdlib_dir = Path::new("<stdlib>");
                load_source(entry.source, stdlib_dir, ctx)?;
            }
            return Ok(());
        } else {
            return Err(OnewayError::CheckError {
                message: format!(
                    "`use std/{}` is not a known standard library type",
                    type_name
                ),
                span: u.span,
            });
        }
    }

    // Local file/module lookup only (no stdlib fallback for bare names).
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
        let hint = if stdlib_entry(type_name).is_some() {
            format!(" (for std library types, use `use std/{}`)", type_name)
        } else {
            String::new()
        };
        return Err(OnewayError::CheckError {
            message: format!(
                "`use {}` cannot find `{}`{}",
                u.name.name,
                candidate.display(),
                hint
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
