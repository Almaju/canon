use crate::ast::{Item, Module};
use crate::error::{OnewayError, Result, Span};
use crate::lexer::Scanner;
use crate::parser::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn load_module(entry: &Path) -> Result<Module> {
    let canonical = entry.canonicalize().map_err(|err| OnewayError::CheckError {
        message: format!("could not resolve `{}`: {}", entry.display(), err),
        span: Span::default(),
    })?;
    let mut seen = HashSet::new();
    let mut combined_items = Vec::new();
    load_into(&canonical, &mut seen, &mut combined_items)?;
    let span = Span::default();
    Ok(Module {
        items: combined_items,
        span,
    })
}

fn load_into(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<Item>,
) -> Result<()> {
    if !seen.insert(path.to_path_buf()) {
        return Ok(());
    }

    let source = fs::read_to_string(path).map_err(|err| OnewayError::CheckError {
        message: format!("could not read `{}`: {}", path.display(), err),
        span: Span::default(),
    })?;
    let mut scanner = Scanner::new(&source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let module = parser.parse()?;

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
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
        if !candidate.exists() {
            return Err(OnewayError::CheckError {
                message: format!(
                    "`use {}` cannot find `{}`",
                    u.name.name,
                    candidate.display()
                ),
                span: u.span,
            });
        }
        let canonical = candidate
            .canonicalize()
            .map_err(|err| OnewayError::CheckError {
                message: format!("could not resolve `{}`: {}", candidate.display(), err),
                span: u.span,
            })?;
        load_into(&canonical, seen, out)?;
    }
    out.extend(other_items);
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
