//! `canon doc` — the generated API reference
//! (docs/src/getting-started/building-and-running.md, § Doc).
//!
//! Canon has no comments, so there is no prose to extract: a Canon API
//! reference is the *declaration surface* and nothing else. That sounds
//! thinner than it is. Because the language has no methods — a "method
//! on `Map`" is just a constructor whose first input is a `Map` — the
//! interesting page for a type is the set of constructors that accept
//! it. That list is what a reader actually wants ("what can I pipe this
//! into?") and it is the one thing a hand-written page cannot keep
//! honest.
//!
//! Cross-linking is total. Every name is a type name, resolution is
//! global, and both shadowing and ambiguity are hard errors, so every
//! identifier in every rendered signature is an unambiguous link — no
//! path disambiguation, and the search index is a flat list.
//!
//! The model is built from a **raw** parse, deliberately: the loader's
//! `resolve_new_syntax` rewrites constructors into their codegen shape
//! (receiver set, name `Self`, `Unit` params stripped), which is not
//! what the source says. Docs render what was written.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Item, TypeExpr};
use crate::error::CanonError;
use crate::formatter::emit_type_expr;
use crate::lexer::Scanner;
use crate::parser::Parser;

// ── Model ───────────────────────────────────────────────────────────────────

/// One source file's worth of API: the unit of navigation, named by its
/// path under `src/` with the extension dropped (`time/date.can` →
/// `time/date`).
pub struct ModuleDoc {
    pub path: String,
    /// Types whose definition lives here, alphabetically.
    pub types: Vec<String>,
}

/// A type's definition — the `Name = body` half. Absent for a type that
/// only ever appears as a constructor's result.
struct TypeDoc {
    /// Canonical rendering of the body, or `None` for a definition-less
    /// type.
    body: Option<TypeExpr>,
    /// Every module declaring this name. Usually one; a structurally
    /// identical duplicate across files is one type, not a clash
    /// (`Length = Int` in both map.can and set.can), so it can be more.
    modules: BTreeSet<String>,
}

/// One constructor declaration, as written.
struct DeclDoc {
    /// The constructed type — the declaration's name.
    name: String,
    /// Inputs, left to right. The first is what a call pipes in.
    inputs: Vec<TypeExpr>,
    return_ty: TypeExpr,
    module: String,
}

/// Everything `canon doc` renders for one package.
pub struct Api {
    pub package: String,
    pub modules: Vec<ModuleDoc>,
    types: BTreeMap<String, TypeDoc>,
    decls: Vec<DeclDoc>,
}

impl Api {
    /// Type names with a page, alphabetically — the package's whole
    /// reserved-name surface.
    pub fn type_names(&self) -> impl Iterator<Item = &String> {
        self.types.keys()
    }
}

/// PascalCase is the type namespace; camelCase names are binding-file
/// FFI aliases (the language spec, § Types-Only Canon) and declare no
/// type, so they carry no documentation.
fn is_type_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// A constructor's inputs as the source wrote them. A multi-input
/// constructor parses its input product as a single `Product` param, so
/// flatten that one case back into a field per input.
fn decl_inputs(params: &[crate::ast::Param], receiver: Option<&str>) -> Vec<TypeExpr> {
    let mut inputs: Vec<TypeExpr> = Vec::new();
    if let Some(recv) = receiver {
        inputs.push(TypeExpr::Named {
            name: recv.to_string(),
            generics: Vec::new(),
            span: Default::default(),
        });
    }
    match params {
        [only] => match &only.ty {
            TypeExpr::Product { fields, .. } => inputs.extend(fields.iter().cloned()),
            ty => inputs.push(ty.clone()),
        },
        many => inputs.extend(many.iter().map(|p| p.ty.clone())),
    }
    inputs
}

/// Build the API model for a package: every `.can` file under `root`,
/// parsed independently and merged. A library has no entry, so
/// reference-following (`loader::load_module`) is the wrong tool — the
/// walk is the whole package by definition.
///
/// Only `src/` is walked. `bindgen/` is derived and shadowed by `src/`,
/// the same rule the loader applies.
pub fn build(package: &str, root: &Path) -> Result<Api, (PathBuf, CanonError)> {
    let mut paths = Vec::new();
    collect_can_files(root, &mut paths);
    paths.sort();

    let mut types: BTreeMap<String, TypeDoc> = BTreeMap::new();
    let mut decls: Vec<DeclDoc> = Vec::new();
    let mut module_types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // A union's variants are types too, and a tag-only variant
    // (`Map = Empty + Node`, no `Empty = …` anywhere) is declared
    // nowhere else. Collected here and filed after the walk, so a
    // variant that *is* declared elsewhere keeps its own module.
    let mut variants: Vec<(String, String)> = Vec::new();

    for path in &paths {
        let module = module_path(root, path);
        let source = fs::read_to_string(path).map_err(|e| {
            (
                path.clone(),
                CanonError::CheckError {
                    message: format!("could not read: {}", e),
                    span: Default::default(),
                },
            )
        })?;
        let items = (|| -> Result<Vec<Item>, CanonError> {
            let tokens = Scanner::new(&source).scan_tokens()?;
            Ok(Parser::new(tokens).parse()?.items)
        })()
        .map_err(|err| (path.clone(), err))?;

        module_types.entry(module.clone()).or_default();
        for item in items {
            match item {
                Item::TypeDef(td) if is_type_name(&td.name.name) => {
                    if let TypeExpr::Union { variants: vs, .. } = &td.body {
                        for v in vs {
                            if let Some(n) = v.simple_name().filter(|n| is_type_name(n)) {
                                variants.push((n.to_string(), module.clone()));
                            }
                        }
                    }
                    let entry = types.entry(td.name.name.clone()).or_insert(TypeDoc {
                        body: None,
                        modules: BTreeSet::new(),
                    });
                    entry.body.get_or_insert(td.body);
                    entry.modules.insert(module.clone());
                    module_types
                        .entry(module.clone())
                        .or_default()
                        .insert(td.name.name);
                }
                Item::Function(f) if is_type_name(&f.name.name) => {
                    types.entry(f.name.name.clone()).or_insert(TypeDoc {
                        body: None,
                        modules: BTreeSet::new(),
                    });
                    decls.push(DeclDoc {
                        inputs: decl_inputs(&f.params, f.receiver.as_ref().map(|r| &*r.name)),
                        name: f.name.name,
                        return_ty: f.return_ty,
                        module: module.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    for (name, module) in variants {
        if !types.contains_key(&name) {
            types.insert(
                name.clone(),
                TypeDoc {
                    body: None,
                    modules: BTreeSet::from([module.clone()]),
                },
            );
            module_types.entry(module).or_default().insert(name);
        }
    }

    // A type with no definition of its own still needs a home: file it
    // under the module that constructs it.
    for decl in &decls {
        if let Some(t) = types.get_mut(&decl.name) {
            if t.modules.is_empty() {
                t.modules.insert(decl.module.clone());
                module_types
                    .entry(decl.module.clone())
                    .or_default()
                    .insert(decl.name.clone());
            }
        }
    }

    let modules = module_types
        .into_iter()
        .map(|(path, types)| ModuleDoc {
            path,
            types: types.into_iter().collect(),
        })
        .collect();

    Ok(Api {
        package: package.to_string(),
        modules,
        types,
        decls,
    })
}

fn collect_can_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_can_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "can") {
            out.push(path);
        }
    }
}

/// `<root>/time/date.can` → `time/date`.
fn module_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let s = rel.to_string_lossy().replace('\\', "/");
    s.strip_suffix(".can").unwrap_or(&s).to_string()
}

// ── Signature rendering ─────────────────────────────────────────────────────

/// Whether a call site needs the `?` that unwraps the result.
fn is_fallible(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Named { name, .. } if name == "Result")
}

/// The declaration form, as the source writes it: `Map * String * Value
/// => Inserted`, or `Unit => Now` for a nullary constructor.
fn declaration_form(decl: &DeclDoc) -> String {
    let inputs = if decl.inputs.is_empty() {
        "Unit".to_string()
    } else {
        decl.inputs
            .iter()
            .map(emit_type_expr)
            .collect::<Vec<_>>()
            .join(" * ")
    };
    format!("{} => {}", inputs, emit_type_expr(&decl.return_ty))
}

/// A command's message: the one input beside the type the declaration
/// gives back (`Map * Insert => Map` → `Insert`). The language spec,
/// § Functions: a command is applied by piping the value into it.
fn command_message(decl: &DeclDoc) -> Option<&TypeExpr> {
    let [a, b] = decl.inputs.as_slice() else {
        return None;
    };
    match (
        a.simple_name() == Some(&decl.name),
        b.simple_name() == Some(&decl.name),
    ) {
        (true, false) => Some(b),
        (false, true) => Some(a),
        _ => None,
    }
}

/// The call form, as a user writes it: values flow through the pipe and
/// the remaining inputs sit in the parens (CLAUDE.md, § canonical call
/// form). A fallible result takes the `?` that unwraps it. A command
/// pipes into its message, the parens holding the message value.
fn call_form(decl: &DeclDoc) -> String {
    let fallible = if is_fallible(&decl.return_ty) {
        "?"
    } else {
        ""
    };
    if let Some(message) = command_message(decl) {
        let message = emit_type_expr(message);
        return format!("{} -> {message}({message}){fallible}", decl.name);
    }
    match decl.inputs.split_first() {
        None => format!("{}(){}", decl.name, fallible),
        Some((first, [])) => format!("{} -> {}{}", emit_type_expr(first), decl.name, fallible),
        Some((first, rest)) => format!(
            "{} -> {}({}){}",
            emit_type_expr(first),
            decl.name,
            rest.iter()
                .map(emit_type_expr)
                .collect::<Vec<_>>()
                .join(" * "),
            fallible
        ),
    }
}

// ── HTML ────────────────────────────────────────────────────────────────────

fn escape_into(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(c),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        escape_into(c, &mut out);
    }
    out
}

/// Escape `sig` for HTML, linking every identifier run that names a
/// documented type. Total by construction: names are global and
/// unambiguous, so an identifier either is a type in this package or is
/// a primitive with no page.
fn linkify(sig: &str, api: &Api, root: &str) -> String {
    let mut out = String::with_capacity(sig.len());
    let mut ident = String::new();
    let flush = |ident: &mut String, out: &mut String| {
        if ident.is_empty() {
            return;
        }
        if api.types.contains_key(ident.as_str()) {
            out.push_str(&format!(
                "<a href=\"{}type/{}.html\">{}</a>",
                root, ident, ident
            ));
        } else {
            out.push_str(&escape(ident));
        }
        ident.clear();
    };
    for c in sig.chars() {
        if c.is_ascii_alphanumeric() {
            ident.push(c);
        } else {
            flush(&mut ident, &mut out);
            escape_into(c, &mut out);
        }
    }
    flush(&mut ident, &mut out);
    out
}

/// The page shell every page shares: the sidebar (module tree) and the
/// search box the flat index feeds.
fn page(api: &Api, root: &str, title: &str, body: &str) -> String {
    let mut nav = String::new();
    for m in &api.modules {
        nav.push_str(&format!(
            "<li><a href=\"{}module/{}.html\">{}</a></li>",
            root,
            m.path,
            escape(&m.path)
        ));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{doctitle}</title>\n\
         <link rel=\"stylesheet\" href=\"{root}style.css\">\n</head>\n<body>\n\
         <aside class=\"side\">\n\
         <a class=\"brand\" href=\"{root}index.html\"><b>&#10033;</b> {pkg}</a>\n\
         <input class=\"search\" type=\"search\" placeholder=\"Search types…\" \
         aria-label=\"Search types\" autocomplete=\"off\">\n\
         <ul class=\"results\" hidden></ul>\n\
         <div class=\"sec\">Modules</div>\n<ul class=\"tree\">{nav}</ul>\n\
         </aside>\n<main class=\"main\">{body}</main>\n\
         <script src=\"{root}search.js\"></script>\n</body>\n</html>\n",
        doctitle = if title == api.package {
            escape(title)
        } else {
            format!("{} — {}", escape(title), escape(&api.package))
        },
        pkg = escape(&api.package),
        root = root,
        nav = nav,
        body = body,
    )
}

/// One list of declarations, rendered as call-form signatures with the
/// result and defining module alongside.
fn decl_list(api: &Api, root: &str, heading: &str, decls: &[&DeclDoc], call: bool) -> String {
    if decls.is_empty() {
        return String::new();
    }
    // Wherever ordering is discretionary, Canon sorts (CLAUDE.md,
    // § Key conventions). A reader looking a constructor up should not
    // have to know which file it lives in.
    let mut decls = decls.to_vec();
    decls.sort_by(|a, b| (&a.name, &a.module).cmp(&(&b.name, &b.module)));

    let mut out = format!("<h2>{}</h2>\n<ul class=\"decls\">", escape(heading));
    for d in &decls {
        // In call form the constructor's name already spells the plain
        // result, so only a wrapped or generic one (`Result<File,
        // IoError>`, `Option<Value>`) is worth repeating.
        let ret = &emit_type_expr(&d.return_ty);
        let sig = if call {
            let mut s = call_form(d);
            if ret != &d.name {
                s.push_str(" → ");
                s.push_str(ret);
            }
            s
        } else {
            declaration_form(d)
        };
        out.push_str(&format!(
            "<li><code class=\"sig\">{}</code>\
             <span class=\"meta\"><a href=\"{}module/{}.html\">{}</a></span></li>",
            linkify(&sig, api, root),
            root,
            d.module,
            escape(&d.module)
        ));
    }
    out.push_str("</ul>\n");
    out
}

fn type_page(api: &Api, name: &str, doc: &TypeDoc) -> String {
    let root = "../";
    let mut body = format!(
        "<h1><span class=\"kind\">type</span> {}</h1>\n",
        escape(name)
    );

    let modules: Vec<String> = doc
        .modules
        .iter()
        .map(|m| format!("<a href=\"{}module/{}.html\">{}</a>", root, m, escape(m)))
        .collect();
    if !modules.is_empty() {
        body.push_str(&format!(
            "<p class=\"meta\">declared in {}</p>\n",
            modules.join(", ")
        ));
    }

    if let Some(ty) = &doc.body {
        body.push_str(&format!(
            "<pre class=\"def\"><code>{} = {}</code></pre>\n",
            escape(name),
            linkify(&emit_type_expr(ty), api, root)
        ));
        let parts = match ty {
            TypeExpr::Union { variants, .. } => Some(("Variants", variants)),
            TypeExpr::Product { fields, .. } => Some(("Fields", fields)),
            _ => None,
        };
        if let Some((heading, parts)) = parts {
            body.push_str(&format!("<h2>{}</h2>\n<ul class=\"decls\">", heading));
            for p in parts {
                body.push_str(&format!(
                    "<li><code class=\"sig\">{}</code></li>",
                    linkify(&emit_type_expr(p), api, root)
                ));
            }
            body.push_str("</ul>\n");
        }
    }

    // Constructors produce this type; the pipe menu consumes it. The
    // second is the list with no hand-written equivalent.
    // A command constructs its own receiver, so it belongs to the pipe
    // menu, not the constructor list.
    let pipes_in = |d: &DeclDoc| d.inputs.first().and_then(TypeExpr::simple_name) == Some(name);
    let is_command = |d: &DeclDoc| d.name == name && command_message(d).is_some();
    let constructors: Vec<&DeclDoc> = api
        .decls
        .iter()
        .filter(|d| d.name == name && !is_command(d))
        .collect();
    let piped: Vec<&DeclDoc> = api
        .decls
        .iter()
        .filter(|d| (d.name != name && pipes_in(d)) || is_command(d))
        .collect();
    let used: Vec<&DeclDoc> = api
        .decls
        .iter()
        .filter(|d| d.name != name && !pipes_in(d) && d.inputs.iter().any(|i| references(i, name)))
        .collect();

    body.push_str(&decl_list(api, root, "Constructors", &constructors, false));
    body.push_str(&decl_list(api, root, "Pipes into", &piped, true));
    body.push_str(&decl_list(api, root, "Also accepted by", &used, false));
    page(api, root, name, &body)
}

/// Whether `name` appears anywhere inside `ty`, generics included.
fn references(ty: &TypeExpr, name: &str) -> bool {
    match ty {
        TypeExpr::Named {
            name: n, generics, ..
        } => n == name || generics.iter().any(|g| references(g, name)),
        TypeExpr::Union { variants, .. } => variants.iter().any(|v| references(v, name)),
        TypeExpr::Product { fields, .. } => fields.iter().any(|f| references(f, name)),
        TypeExpr::Repeat { ty, .. } | TypeExpr::Spread { ty, .. } => references(ty, name),
        TypeExpr::Function {
            params, return_ty, ..
        } => params.iter().any(|p| references(p, name)) || references(return_ty, name),
    }
}

fn module_page(api: &Api, module: &ModuleDoc) -> String {
    let depth = module.path.matches('/').count() + 1;
    let root = "../".repeat(depth);
    let mut body = format!(
        "<h1><span class=\"kind\">module</span> {}</h1>\n",
        escape(&module.path)
    );
    if module.types.is_empty() {
        body.push_str("<p class=\"meta\">No types.</p>\n");
    } else {
        body.push_str("<h2>Types</h2>\n<ul class=\"index\">");
        for name in &module.types {
            body.push_str(&format!(
                "<li><a href=\"{}type/{}.html\">{}</a>{}</li>",
                root,
                name,
                escape(name),
                api.types
                    .get(name)
                    .and_then(|t| t.body.as_ref())
                    .map(|b| format!(
                        " <code class=\"sig\">= {}</code>",
                        linkify(&emit_type_expr(b), api, &root)
                    ))
                    .unwrap_or_default()
            ));
        }
        body.push_str("</ul>\n");
    }
    page(api, &root, &module.path, &body)
}

fn index_page(api: &Api) -> String {
    let root = "";
    let mut body = format!(
        "<h1><span class=\"kind\">package</span> {}</h1>\n\
         <p class=\"meta\">{} types across {} modules. Canon has no comments, so \
         this reference is the declaration surface: every type, what constructs \
         it, and what it pipes into.</p>\n",
        escape(&api.package),
        api.types.len(),
        api.modules.len()
    );

    body.push_str("<h2>Modules</h2>\n<ul class=\"index\">");
    for m in &api.modules {
        body.push_str(&format!(
            "<li><a href=\"module/{}.html\">{}</a> <span class=\"meta\">{} type{}</span></li>",
            m.path,
            escape(&m.path),
            m.types.len(),
            if m.types.len() == 1 { "" } else { "s" }
        ));
    }
    body.push_str("</ul>\n");

    // Every name the package claims. In Canon these are global — a
    // user type of the same name is a compile error — so this list is
    // also the "check before you name something" list.
    body.push_str("<h2>All Types</h2>\n<ul class=\"all\">");
    for name in api.types.keys() {
        body.push_str(&format!(
            "<li><a href=\"type/{}.html\">{}</a></li>",
            name,
            escape(name)
        ));
    }
    body.push_str("</ul>\n");
    page(api, root, &api.package, &body)
}

/// Dark-first, in the docs site's palette — the generated `canon`
/// reference is served under the docs deploy, so it has to look native
/// there. The light override keeps a standalone `canon doc` readable.
const STYLE_CSS: &str = r#"/* generated by `canon doc`; do not edit */
:root {
  --bg: #0a0a0a; --panel: #0e0e0f; --fg: #ededed; --body: #c9c9cf;
  --dim: #63636b; --line: #1e1e21; --line-hi: #2c2c31; --accent: #e8a24b;
  --sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, Helvetica, Arial, sans-serif;
  --mono: ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace;
}
@media (prefers-color-scheme: light) {
  :root {
    --bg: #fdfdfc; --panel: #f5f5f2; --fg: #16161a; --body: #33333a;
    --dim: #77777f; --line: #e4e4e0; --line-hi: #d4d4cf; --accent: #9a6a10;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--body);
  font: 15px/1.7 var(--sans); -webkit-font-smoothing: antialiased;
  display: grid; grid-template-columns: 17rem minmax(0, 1fr);
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
code, .sig, pre { font-family: var(--mono); }

.side {
  border-right: 1px solid var(--line); padding: 22px 14px 40px;
  height: 100vh; position: sticky; top: 0; overflow-y: auto; scrollbar-width: thin;
}
.brand {
  display: block; font-family: var(--mono); font-size: 15px; font-weight: 600;
  letter-spacing: .04em; color: var(--fg); padding: 4px 10px 16px;
}
.brand b { color: var(--accent); margin-right: .4em; }
.search {
  width: 100%; background: var(--panel); color: var(--fg); border: 1px solid var(--line);
  border-radius: 7px; padding: 7px 10px; font: 13px/1.4 var(--sans); outline: none;
}
.search::placeholder { color: var(--dim); }
.search:focus { border-color: var(--accent); }
.sec {
  font-family: var(--mono); font-size: 11px; font-weight: 500; letter-spacing: .14em;
  text-transform: uppercase; color: var(--accent); margin: 18px 0 5px; padding: 0 10px;
}
.tree, .results { list-style: none; margin: 0; padding: 0; }
.tree li, .results li { margin: 1px 0; }
.tree a, .results a {
  display: block; border-radius: 6px; padding: 5px 10px;
  color: var(--dim); font-size: 13.5px; word-break: break-all;
}
.tree a:hover, .results a:hover {
  color: var(--fg); background: rgba(127, 127, 127, .12); text-decoration: none;
}
.results { margin-top: 8px; max-height: 60vh; overflow-y: auto; }

.main { padding: 56px 40px 96px; max-width: 62rem; min-width: 0; }
h1 { font-size: 30px; font-weight: 700; letter-spacing: -.03em; margin: 0 0 .4em; color: var(--fg); }
h2 {
  font-family: var(--mono); font-size: 11px; font-weight: 500; letter-spacing: .14em;
  text-transform: uppercase; color: var(--accent); margin: 2.4em 0 .6em;
}
.kind { color: var(--dim); font-weight: 400; }
.meta { color: var(--dim); font-size: 13px; }

.def {
  margin: 1.4em 0; padding: 16px 20px; border: 1px solid var(--line-hi); border-radius: 12px;
  background: var(--panel); overflow-x: auto;
}
.def code { color: var(--fg); font-size: 13.5px; }
.decls, .index { list-style: none; margin: 0; padding: 0; }
.decls li, .index li {
  padding: 9px 2px; border-top: 1px solid var(--line);
  display: flex; gap: 1.5rem; justify-content: space-between; align-items: baseline;
}
.decls li:first-child, .index li:first-child { border-top: 0; }
.sig { color: var(--fg); font-size: 13.5px; white-space: pre-wrap; }
.all {
  list-style: none; margin: 0; padding: 0; display: grid;
  grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr)); gap: 2px 1rem;
}
.all li { font-size: 13.5px; }

@media (max-width: 52rem) {
  body { grid-template-columns: 1fr; }
  .side {
    height: auto; position: static; border-right: 0;
    border-bottom: 1px solid var(--line); padding: 14px 12px;
  }
  .main { padding: 32px 22px 72px; }
  h1 { font-size: 26px; }
  .decls li, .index li { flex-direction: column; gap: 2px; }
}
"#;

/// The search index plus the ~30 lines that filter it. Names are
/// globally unique, so the index is a flat array and matching is a
/// substring test — no scoring, no disambiguation.
fn search_js(api: &Api) -> String {
    let names: Vec<String> = api.types.keys().map(|n| format!("\"{}\"", n)).collect();
    format!(
        r#""use strict";
// generated by `canon doc`; do not edit
(function () {{
  const NAMES = [{names}];
  const root = document.currentScript.src.replace(/search\.js$/, "");
  const box = document.querySelector(".search");
  const out = document.querySelector(".results");
  if (!box || !out) return;
  box.addEventListener("input", function () {{
    const q = box.value.trim().toLowerCase();
    if (!q) {{ out.hidden = true; out.innerHTML = ""; return; }}
    const hits = NAMES.filter((n) => n.toLowerCase().includes(q)).slice(0, 40);
    out.innerHTML = hits
      .map((n) => `<li><a href="${{root}}type/${{n}}.html">${{n}}</a></li>`)
      .join("");
    out.hidden = hits.length === 0;
  }});
}})();
"#,
        names = names.join(", ")
    )
}

/// Render `api` into `out_dir`, returning the number of pages written.
pub fn render(api: &Api, out_dir: &Path) -> std::io::Result<usize> {
    // A stale page for a since-deleted type would linger and stay
    // reachable by URL, so the tree is rebuilt from empty.
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)?;
    }
    fs::create_dir_all(out_dir.join("type"))?;

    fs::write(out_dir.join("style.css"), STYLE_CSS)?;
    fs::write(out_dir.join("search.js"), search_js(api))?;
    fs::write(out_dir.join("index.html"), index_page(api))?;
    let mut pages = 1;

    for (name, doc) in &api.types {
        fs::write(
            out_dir.join("type").join(format!("{}.html", name)),
            type_page(api, name, doc),
        )?;
        pages += 1;
    }
    for module in &api.modules {
        let path = out_dir.join("module").join(format!("{}.html", module.path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, module_page(api, module))?;
        pages += 1;
    }
    Ok(pages)
}
