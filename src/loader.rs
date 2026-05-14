use crate::ast::{Item, Module};
use crate::error::{OnewayError, Result, Span};
use crate::lexer::Scanner;
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CargoDep {
    pub name: &'static str,
    pub version: &'static str,
    pub features: &'static [&'static str],
}

struct StdlibEntry {
    name: &'static str,
    source: &'static str,
    cargo_deps: &'static [CargoDep],
    rust_prelude: Option<&'static str>,
}

const STDLIB: &[StdlibEntry] = &[
    StdlibEntry {
        name: "Clock",
        source: include_str!("../std/clock.ow"),
        cargo_deps: &[CargoDep {
            name: "chrono",
            version: "0.4",
            features: &[],
        }],
        rust_prelude: None,
    },
    StdlibEntry {
        name: "Datetime",
        source: include_str!("../std/datetime.ow"),
        cargo_deps: &[CargoDep {
            name: "chrono",
            version: "0.4",
            features: &[],
        }],
        rust_prelude: None,
    },
    StdlibEntry {
        name: "Filesystem",
        source: include_str!("../std/filesystem.ow"),
        cargo_deps: &[CargoDep {
            name: "tokio",
            version: "1",
            features: &["full"],
        }],
        rust_prelude: None,
    },
    StdlibEntry {
        name: "HttpClient",
        source: include_str!("../std/http_client.ow"),
        cargo_deps: &[
            CargoDep {
                name: "reqwest",
                version: "0.12",
                features: &[],
            },
            CargoDep {
                name: "tokio",
                version: "1",
                features: &["full"],
            },
        ],
        rust_prelude: Some(include_str!("../std/http_client.rs")),
    },
    StdlibEntry {
        name: "Json",
        source: include_str!("../std/json.ow"),
        cargo_deps: &[
            CargoDep {
                name: "serde",
                version: "1",
                features: &["derive"],
            },
            CargoDep {
                name: "serde_json",
                version: "1",
                features: &[],
            },
        ],
        rust_prelude: Some(include_str!("../std/json.rs")),
    },
    StdlibEntry {
        name: "Path",
        source: include_str!("../std/path.ow"),
        cargo_deps: &[],
        rust_prelude: Some(include_str!("../std/path.rs")),
    },
    StdlibEntry {
        name: "Url",
        source: include_str!("../std/url.ow"),
        cargo_deps: &[],
        rust_prelude: Some(include_str!("../std/url.rs")),
    },
];

fn stdlib_entry(name: &str) -> Option<&'static StdlibEntry> {
    STDLIB.iter().find(|e| e.name == name)
}

pub struct LoadResult {
    pub module: Module,
    pub cargo_deps: Vec<&'static CargoDep>,
    pub rust_preludes: Vec<&'static str>,
}

struct LoadCtx {
    seen: HashSet<PathBuf>,
    seen_stdlib: HashSet<String>,
    items: Vec<Item>,
    cargo_deps: Vec<&'static CargoDep>,
    rust_preludes: Vec<&'static str>,
}

pub fn load_module(entry: &Path) -> Result<LoadResult> {
    let canonical = entry.canonicalize().map_err(|err| OnewayError::CheckError {
        message: format!("could not resolve `{}`: {}", entry.display(), err),
        span: Span::default(),
    })?;
    let mut ctx = LoadCtx {
        seen: HashSet::new(),
        seen_stdlib: HashSet::new(),
        items: Vec::new(),
        cargo_deps: Vec::new(),
        rust_preludes: Vec::new(),
    };
    load_into(&canonical, &mut ctx)?;
    let span = Span::default();
    Ok(LoadResult {
        module: Module {
            items: ctx.items,
            span,
        },
        cargo_deps: ctx.cargo_deps,
        rust_preludes: ctx.rust_preludes,
    })
}

fn load_into(path: &Path, ctx: &mut LoadCtx) -> Result<()> {
    if !ctx.seen.insert(path.to_path_buf()) {
        return Ok(());
    }
    let source = fs::read_to_string(path).map_err(|err| OnewayError::CheckError {
        message: format!("could not read `{}`: {}", path.display(), err),
        span: Span::default(),
    })?;
    load_source(&source, path.parent().unwrap_or_else(|| Path::new(".")), ctx)
}

fn load_source(source: &str, dir: &Path, ctx: &mut LoadCtx) -> Result<()> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let module = parser.parse()?;

    let mut use_items = Vec::new();
    let mut other_items = Vec::new();
    for item in module.items {
        match item {
            Item::Use(u) => use_items.push(u),
            other => other_items.push(other),
        }
    }
    for u in use_items {
        let file_name = snake_case(&u.name.name);
        let candidate = dir.join(format!("{}.ow", file_name));
        if candidate.exists() {
            let canonical = candidate
                .canonicalize()
                .map_err(|err| OnewayError::CheckError {
                    message: format!("could not resolve `{}`: {}", candidate.display(), err),
                    span: u.span,
                })?;
            load_into(&canonical, ctx)?;
        } else if let Some(entry) = stdlib_entry(&u.name.name) {
            if ctx.seen_stdlib.insert(u.name.name.clone()) {
                for dep in entry.cargo_deps {
                    ctx.cargo_deps.push(dep);
                }
                if let Some(prelude) = entry.rust_prelude {
                    ctx.rust_preludes.push(prelude);
                }
                let stdlib_dir = Path::new("<stdlib>");
                load_source(entry.source, stdlib_dir, ctx)?;
            }
        } else {
            return Err(OnewayError::CheckError {
                message: format!(
                    "`use {}` cannot find `{}` (not in current directory and not a shipped binding)",
                    u.name.name,
                    candidate.display()
                ),
                span: u.span,
            });
        }
    }
    ctx.items.extend(other_items);
    Ok(())
}

fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}
