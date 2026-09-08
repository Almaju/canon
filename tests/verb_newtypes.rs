//! Commands take messages (docs/src/spec/functions.md): an arrow that gives
//! its input back is `T * M => T`, never `… T … => X` with `X = T`. The
//! checker cannot enforce the spelling — `Contents * Path => Written` is the
//! evidence a write returns, `Json * String => Field` looks a field up, and
//! json's `ParseStep * String => ParsedObjectTail` continues a parse: all
//! queries, all the same syntax as a command in disguise. So the corpus is
//! pinned instead. Every arrow under `packages/`, `examples/`, `docs/src/`,
//! and `canonc/` that returns a newtype of one of its own non-scalar inputs
//! is listed below with the reason it is a query; a new one is a review
//! question, answered by turning it into a command or adding it here.
//!
//! Scalar-rooted receivers (`Json = String`, `Seed = Int`, the markdown
//! cursor helpers on `Int * String`) are values, not state, and stay out
//! of the pin: `String => Uppercased` produces a new string, it does not
//! change one.

use canon::ast::{Item, TypeExpr};
use canon::lexer::Scanner;
use canon::parser::Parser;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const PINNED: &[&str] = &[
    // canonc walks its token cons-list by returning the rest of it — a
    // suffix, not the list changed — and threads its scope tables and
    // parameter lists through result newtypes. It compiles itself, so it
    // spells commands the message way only once it can compile them.
    "canonc/src/compiled.can: Depth * Tokens => Skipped",
    "canonc/src/compiled.can: Params * Tokens * Wasname => Gathered",
    "canonc/src/compiled.can: Tables => Pushed",
    "canonc/src/compiled.can: Tokens * Wanted => Rhseq",
    "canonc/src/compiled.can: Tokens * Wanted => Rhsname",
    "canonc/src/compiled.can: Tokens * Wanted => Rhstokens",
    // The JSON parser's continuation steps: the parse step after the
    // piece each one consumed, a position or a failure, never the step
    // it was handed changed.
    "packages/canon/src/json.can: ParseStep * String => CheckedTrailing",
    "packages/canon/src/json.can: ParseStep * String => ParsedArrayTail",
    "packages/canon/src/json.can: ParseStep * String => ParsedExpAfter",
    "packages/canon/src/json.can: ParseStep * String => ParsedObjectColon",
    "packages/canon/src/json.can: ParseStep * String => ParsedObjectTail",
];

const SCALARS: &[&str] = &["Bool", "Float", "Int", "String", "Unit"];

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if name != "deps" && name != "bindgen" && name != "build" && name != "target" {
                collect(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "can") {
            out.push(path);
        }
    }
}

fn component_names(ty: &TypeExpr, out: &mut Vec<String>) {
    match ty {
        TypeExpr::Named { name, .. } => out.push(name.clone()),
        TypeExpr::Product { fields, .. } => fields.iter().for_each(|f| component_names(f, out)),
        TypeExpr::Repeat { ty, .. } => component_names(ty, out),
        _ => {}
    }
}

fn constructed(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Named { name, generics, .. }
            if matches!(name.as_str(), "Result" | "Option" | "Future") =>
        {
            constructed(generics.first()?)
        }
        TypeExpr::Named { name, .. } => Some(name),
        _ => None,
    }
}

fn spell(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { name, generics, .. } if generics.is_empty() => name.clone(),
        TypeExpr::Named { name, generics, .. } => format!(
            "{name}<{}>",
            generics.iter().map(spell).collect::<Vec<_>>().join(", ")
        ),
        TypeExpr::Product { fields, .. } => {
            fields.iter().map(spell).collect::<Vec<_>>().join(" * ")
        }
        TypeExpr::Repeat { ty, count, .. } => format!("{}^{count}", spell(ty)),
        _ => "…".to_string(),
    }
}

#[test]
fn every_result_newtype_of_a_state_input_is_pinned() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["packages", "examples", "docs/src", "canonc/src"] {
        collect(&root.join(dir), &mut files);
    }
    // Names resolve the way the loader resolves them: a package's own
    // files first (`Body = Parsed` in canonc, `Body = String` in the
    // prelude), then the prelude. A package is the parent of a `src/`.
    let prelude = Path::new("packages/canon");
    let mut modules = Vec::new();
    let mut aliases: HashMap<PathBuf, HashMap<String, TypeExpr>> = HashMap::new();
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let tokens = Scanner::new(&source).scan_tokens().unwrap();
        let module = Parser::new(tokens).parse().unwrap();
        let path = path.strip_prefix(root).unwrap().to_path_buf();
        let package = path
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "src"))
            .and_then(Path::parent)
            .unwrap_or(&path)
            .to_path_buf();
        let scope = aliases.entry(package.clone()).or_default();
        for item in &module.items {
            if let Item::TypeDef(td) = item {
                scope.insert(td.name.name.clone(), td.body.clone());
            }
        }
        modules.push((path, package, module));
    }
    let lookup = |package: &Path, name: &str| -> Option<&TypeExpr> {
        aliases
            .get(package)
            .and_then(|scope| scope.get(name))
            .or_else(|| aliases.get(prelude).and_then(|scope| scope.get(name)))
    };
    // The newtype chain from `name`, `name` itself excluded.
    let chain = |package: &Path, name: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut current = name.to_string();
        for _ in 0..20 {
            match lookup(package, &current) {
                Some(TypeExpr::Named { name, generics, .. }) if generics.is_empty() => {
                    current = name.clone();
                    out.push(current.clone());
                }
                _ => break,
            }
        }
        out
    };
    let scalar_rooted = |package: &Path, name: &str| -> bool {
        let root = chain(package, name)
            .pop()
            .unwrap_or_else(|| name.to_string());
        SCALARS.contains(&root.as_str())
    };

    let mut found = BTreeSet::new();
    for (path, package, module) in &modules {
        for item in &module.items {
            let Item::Function(f) = item else { continue };
            let Some(constructed) = constructed(&f.return_ty) else {
                continue;
            };
            let mut inputs = Vec::new();
            f.params
                .iter()
                .for_each(|p| component_names(&p.ty, &mut inputs));
            let hit = chain(package, constructed)
                .iter()
                .any(|base| inputs.contains(base) && !scalar_rooted(package, base));
            if hit {
                let inputs = f.params.iter().map(|p| spell(&p.ty)).collect::<Vec<_>>();
                found.insert(format!(
                    "{}: {} => {}",
                    path.display(),
                    inputs.join(" * "),
                    spell(&f.return_ty)
                ));
            }
        }
    }
    let pinned: BTreeSet<String> = PINNED.iter().map(|s| s.to_string()).collect();
    let unpinned: Vec<_> = found.difference(&pinned).collect();
    let stale: Vec<_> = pinned.difference(&found).collect();
    assert!(
        unpinned.is_empty() && stale.is_empty(),
        "result newtypes of a state input:\n  not pinned (a command in disguise? spell it \
         `T * M => T`, or pin it with its reason):\n    {}\n  pinned but gone (drop the pin):\n    {}",
        unpinned.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n    "),
        stale.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n    ")
    );
}
