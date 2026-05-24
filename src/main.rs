use oneway::ast::{resolve_new_syntax, FunctionDef, Item, TypeExpr};
use oneway::checker;
use oneway::codegen;
use oneway::error::OnewayError;
use oneway::formatter;
use oneway::lexer::Scanner;
use oneway::loader;
use oneway::manifest;
use oneway::parser::Parser;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_help();
        process::exit(1);
    }

    let cmd = args[1].as_str();
    let rest: Vec<String> = args[2..].to_vec();

    match cmd {
        "run" => cmd_run(&rest),
        "build" => cmd_build(&rest),
        "emit" => cmd_emit(&rest),
        "ast" => cmd_ast(&rest),
        "check" => cmd_check(&rest),
        "test" => cmd_test(&rest),
        "tokens" => cmd_tokens(&rest),
        "fmt" | "format" => cmd_fmt(&rest),
        "gen-bindings" => cmd_gen_bindings(&rest),
        "lsp" => oneway::lsp::run(),
        "upgrade" | "update" => cmd_upgrade(&rest),
        "version" | "--version" | "-V" => {
            println!("oneway {}", VERSION);
        }
        "help" | "--help" | "-h" => print_help(),
        other => {
            eprintln!("error: unknown command '{}'", other);
            eprintln!();
            print_help();
            process::exit(1);
        }
    }
}

fn print_help() {
    println!("oneway {} \u{2014} the Oneway language compiler", VERSION);
    println!();
    println!("Usage: oneway <command> [args]");
    println!();
    println!("A target is either a package directory (containing `oneway.toml`");
    println!("and `src/main.ow`), a workspace directory (manifest with a");
    println!("`[workspace]` table), or a single `.ow` file. When omitted, defaults");
    println!("to the current directory.");
    println!();
    println!("Commands:");
    println!("  run [target] [-p name] [args...]");
    println!("                            Compile and run an Oneway program");
    println!("  build [target] [-p name]  Compile to a WASM component (.wasm)");
    println!("  check [target] [-p name]  Check sort order and types");
    println!("  emit <file.ow>            Print generated WAT (WebAssembly Text)");
    println!("  ast <file.ow>             Print the parsed AST");
    println!("  test <file.ow>            Run `() -> TestResult` functions as tests");
    println!("  tokens <file.ow>          Print lexer tokens");
    println!("  fmt <file.ow> [--check]   Format an Oneway source file");
    println!("  gen-bindings <wit-or-wasm> [-o <dir>]");
    println!(
        "                            Generate Oneway bindings from a WIT package or WASM component"
    );
    println!("  lsp                       Start the Language Server Protocol server");
    println!("  upgrade [version]         Update oneway to the latest (or given) release");
    println!("  upgrade --check           Check whether a newer release is available");
    println!("  version                   Print version");
    println!("  help                      Print this message");
}

fn require_file(args: &[String]) -> &str {
    match args.first() {
        Some(f) => f.as_str(),
        None => {
            eprintln!("error: missing input file");
            process::exit(1);
        }
    }
}

fn read_source(file_path: &str) -> String {
    match fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error: could not read '{}': {}", file_path, err);
            process::exit(1);
        }
    }
}

/// A single buildable compilation target: a package (`oneway.toml` +
/// `src/main.ow`) or a loose `.ow` file in single-file mode.
struct BuildSpec {
    /// Entry `.ow` file the loader will read.
    entry: PathBuf,
    /// Where `build/` lives for this target. For a workspace member, this
    /// points at the workspace's shared `build/` (Cargo-style `target/`).
    output_dir: PathBuf,
    /// Stem used for output artifacts (`<stem>.wasm`, `<stem>.wit`). For a
    /// package it's the last `/`-separated segment of the manifest `name`
    /// (e.g. `oneway/std` -> `std`). For a loose file it's the file stem.
    output_stem: String,
    /// Path the user typed (or the workspace member's display path), used
    /// as the context in error messages.
    label: String,
    /// Full manifest `name` (e.g. `"oneway/std"`). Empty for file-mode
    /// targets. Used by `-p <name>` filtering.
    name: String,
}

impl BuildSpec {
    fn entry_str(&self) -> &str {
        self.entry.to_str().unwrap_or(&self.label)
    }
}

/// A resolved compile target.
///
/// `oneway run|build|check` accept any of:
///
/// - a **package directory** (containing `oneway.toml` and `src/main.ow`),
/// - a **single `.ow` file** (anonymous single-file package), or
/// - a **workspace directory** (containing `oneway.toml` with a
///   `[workspace]` table) which aggregates one or more member packages.
enum Target {
    /// One package or one loose file.
    Build(BuildSpec),
    /// A workspace and its already-resolved member specs (sorted
    /// alphabetically by label).
    Workspace {
        members: Vec<BuildSpec>,
        label: String,
    },
}

/// Parsed command-line args for the package-aware commands
/// (`build`/`check`/`run`).
struct ParsedTargetArgs {
    /// First positional argument: a target path. `None` means `.`.
    target_path: Option<String>,
    /// `-p`/`--package` value: select a single member by name within a
    /// workspace target.
    package: Option<String>,
    /// Remaining positional arguments after the target path. Only used
    /// by `oneway run` (passed through to the program).
    program_args: Vec<String>,
}

/// Parse a command's args into `(target_path, -p, program_args)`. When
/// `accept_program_args` is `false`, any positional beyond the target
/// path is an error.
fn parse_target_args(args: &[String], accept_program_args: bool) -> ParsedTargetArgs {
    let mut target_path: Option<String> = None;
    let mut package: Option<String> = None;
    let mut program_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-p" | "--package" => {
                if i + 1 >= args.len() {
                    eprintln!("error: `{}` requires a package name", a);
                    process::exit(1);
                }
                if package.is_some() {
                    eprintln!("error: `-p` given more than once");
                    process::exit(1);
                }
                package = Some(args[i + 1].clone());
                i += 2;
            }
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("error: unknown flag `{}`", other);
                process::exit(1);
            }
            _ => {
                if target_path.is_none() {
                    target_path = Some(a.clone());
                } else if accept_program_args {
                    program_args.push(a.clone());
                } else {
                    eprintln!("error: unexpected argument `{}`", a);
                    process::exit(1);
                }
                i += 1;
            }
        }
    }
    ParsedTargetArgs {
        target_path,
        package,
        program_args,
    }
}

/// Apply `-p <name>` to a resolved target, narrowing a workspace down
/// to a single member. Errors out if `-p` is given for a non-workspace
/// target or if no member matches.
fn apply_package_filter(target: Target, filter: Option<&str>) -> Target {
    let Some(want) = filter else {
        return target;
    };
    match target {
        Target::Build(_) => {
            eprintln!("error: `-p {}` is only valid with a workspace target", want);
            process::exit(1);
        }
        Target::Workspace { members, label } => {
            // Match against the full manifest name (`oneway/std`) or its
            // last segment (`std`). Workspace members in this repo use
            // flat names, but we accept both for parity with `cargo -p`.
            let matched: Vec<BuildSpec> = members
                .into_iter()
                .filter(|s| s.name == want || s.output_stem == want)
                .collect();
            match matched.len() {
                0 => {
                    eprintln!("error: no member `{}` in workspace `{}`", want, label);
                    process::exit(1);
                }
                1 => Target::Build(matched.into_iter().next().unwrap()),
                n => {
                    eprintln!(
                        "error: package name `{}` matched {} members of workspace `{}`",
                        want, n, label
                    );
                    process::exit(1);
                }
            }
        }
    }
}

/// Resolve the first positional argument to a `Target`. Defaults to `.`
/// (current directory) when no path is given.
fn resolve_target(path_arg: Option<&str>) -> Target {
    let arg = path_arg.unwrap_or(".");
    let path = Path::new(arg);

    if path.is_dir() {
        resolve_dir_target(path, arg)
    } else if path.is_file() {
        Target::Build(resolve_file_spec(path, arg))
    } else {
        eprintln!("error: `{}` is neither a file nor a directory", arg);
        process::exit(1);
    }
}

fn resolve_dir_target(path: &Path, arg: &str) -> Target {
    let manifest_path = path.join("oneway.toml");
    if !manifest_path.exists() {
        eprintln!("error: `{}` is a directory but has no `oneway.toml`", arg);
        eprintln!(
            "hint: a package directory must contain an `oneway.toml` manifest; \
             pass a `.ow` file directly to compile in single-file mode"
        );
        process::exit(1);
    }
    let m = read_manifest(&manifest_path);

    if let Some(ws) = &m.workspace {
        let members = resolve_workspace_members(path, arg, &ws.members);
        return Target::Workspace {
            members,
            label: arg.to_string(),
        };
    }

    // Plain package directory. If it lives inside a workspace, route its
    // artifacts to the workspace's shared `build/` (Cargo-style).
    let workspace_root = find_parent_workspace(path);
    Target::Build(resolve_package_spec(
        path,
        arg,
        &m,
        workspace_root.as_deref(),
    ))
}

/// Walk up from `start` (exclusive) looking for an ancestor whose
/// `oneway.toml` carries a `[workspace]` table. Returns the workspace
/// root directory, or `None` if there isn't one.
///
/// Failure to read or parse an ancestor's manifest is silent here: we
/// only care about the workspace-or-not flag. The full parse error will
/// surface when the user actually invokes a command on that path.
fn find_parent_workspace(start: &Path) -> Option<PathBuf> {
    let mut current = start.canonicalize().ok()?;
    while let Some(parent) = current.parent() {
        let parent = parent.to_path_buf();
        let manifest = parent.join("oneway.toml");
        if manifest.exists() {
            if let Ok(src) = fs::read_to_string(&manifest) {
                if let Ok(m) = manifest::parse(&src) {
                    if m.workspace.is_some() {
                        return Some(parent);
                    }
                }
            }
        }
        if parent == current {
            break;
        }
        current = parent;
    }
    None
}

fn read_manifest(manifest_path: &Path) -> manifest::Manifest {
    let src = match fs::read_to_string(manifest_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read `{}`: {}", manifest_path.display(), e);
            process::exit(1);
        }
    };
    match manifest::parse(&src) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: in `{}`: {}", manifest_path.display(), e);
            process::exit(1);
        }
    }
}

/// Build a `BuildSpec` for a package directory. When `workspace_root` is
/// `Some`, output is routed to `<workspace_root>/build/`; otherwise it
/// lands in `<pkg_root>/build/`.
fn resolve_package_spec(
    pkg_root: &Path,
    label: &str,
    m: &manifest::Manifest,
    workspace_root: Option<&Path>,
) -> BuildSpec {
    let entry = pkg_root.join("src").join("main.ow");
    if !entry.exists() {
        eprintln!(
            "error: package `{}` has no entry point at `{}`",
            if m.name.is_empty() { label } else { &m.name },
            entry.display()
        );
        eprintln!("hint: create `src/main.ow` with a `main` function");
        process::exit(1);
    }
    let output_stem = m.name.rsplit('/').next().unwrap_or(&m.name);
    let output_stem = if output_stem.is_empty() {
        // Workspace member with no name: fall back to the directory name.
        pkg_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("out")
            .to_string()
    } else {
        output_stem.to_string()
    };
    let output_dir = match workspace_root {
        Some(ws) => ws.join("build"),
        None => pkg_root.join("build"),
    };
    BuildSpec {
        entry,
        output_dir,
        output_stem,
        label: label.to_string(),
        name: m.name.clone(),
    }
}

fn resolve_file_spec(path: &Path, arg: &str) -> BuildSpec {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway")
        .to_string();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // File mode keeps a per-stem subdir so a directory full of `.ow`
    // files (e.g. `tests/runtime/`) doesn't have its artifacts collide.
    let output_dir = dir.join("build").join(&stem);
    BuildSpec {
        entry: path.to_path_buf(),
        output_dir,
        output_stem: stem,
        label: arg.to_string(),
        name: String::new(),
    }
}

/// Resolve a workspace's `members = [...]` directive to concrete
/// `BuildSpec`s. A single literal `"*"` expands to every immediate
/// subdirectory of the workspace root that contains an `oneway.toml`.
/// Otherwise each entry is treated as a relative path from the workspace
/// root. Members are returned sorted alphabetically by label.
fn resolve_workspace_members(ws_root: &Path, ws_label: &str, members: &[String]) -> Vec<BuildSpec> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut had_glob = false;

    for entry in members {
        if entry == "*" {
            had_glob = true;
            let read = match fs::read_dir(ws_root) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: could not list `{}`: {}", ws_root.display(), e);
                    process::exit(1);
                }
            };
            for d in read.flatten() {
                let p = d.path();
                if p.is_dir() && p.join("oneway.toml").exists() {
                    paths.push(p);
                }
            }
        } else {
            let p = ws_root.join(entry);
            if !p.is_dir() {
                eprintln!(
                    "error: workspace member `{}` is not a directory (looked at `{}`)",
                    entry,
                    p.display()
                );
                process::exit(1);
            }
            if !p.join("oneway.toml").exists() {
                eprintln!(
                    "error: workspace member `{}` has no `oneway.toml`",
                    p.display()
                );
                process::exit(1);
            }
            paths.push(p);
        }
    }

    paths.sort();
    paths.dedup();

    if paths.is_empty() {
        if had_glob {
            eprintln!(
                "warning: workspace `{}` matched no members (no subdir of `{}` contains an `oneway.toml`)",
                ws_label,
                ws_root.display()
            );
        } else {
            eprintln!("warning: workspace `{}` has no members", ws_label);
        }
    }

    paths
        .into_iter()
        .map(|p| {
            let label = p.to_string_lossy().into_owned();
            let manifest_path = p.join("oneway.toml");
            let m = read_manifest(&manifest_path);
            if m.workspace.is_some() {
                eprintln!(
                    "error: nested workspaces are not supported (`{}` is also a workspace)",
                    p.display()
                );
                process::exit(1);
            }
            // All members share the workspace's `build/` (Cargo-style).
            resolve_package_spec(&p, &label, &m, Some(ws_root))
        })
        .collect()
}

fn cmd_tokens(args: &[String]) {
    let file_path = require_file(args);
    let source = read_source(file_path);
    let mut scanner = Scanner::new(&source);
    let tokens = match scanner.scan_tokens() {
        Ok(t) => t,
        Err(err) => {
            print_error(file_path, &err);
            process::exit(1);
        }
    };
    for token in &tokens {
        println!(
            "{:>4}:{:<4} {:<20} {:?}",
            token.span.line, token.span.column, token.kind, token.lexeme
        );
    }
}

fn cmd_gen_bindings(args: &[String]) {
    let mut input: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-o" | "--out" => match iter.next() {
                Some(v) => out_dir = Some(v.clone()),
                None => {
                    eprintln!("error: `-o` requires a directory argument");
                    process::exit(1);
                }
            },
            "--help" | "-h" => {
                println!("Usage: oneway gen-bindings <wit-or-wasm> [-o <dir>]");
                println!();
                println!("  <wit-or-wasm>   A `.wit` file, a directory of `.wit` files, or a");
                println!("                  WebAssembly Component `.wasm` whose embedded WIT will");
                println!("                  be extracted.");
                println!("  -o <dir>        Output root (default: current directory).");
                println!();
                println!("Bindings are written as `<dir>/<namespace>/<package>/<interface>.ow`,");
                println!("e.g. `wasi/clocks/monotonic_clock.ow`.");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown gen-bindings flag '{}'", other);
                process::exit(1);
            }
            other => {
                if input.is_some() {
                    eprintln!(
                        "error: multiple input paths given ('{}' and '{}')",
                        input.as_deref().unwrap(),
                        other
                    );
                    process::exit(1);
                }
                input = Some(other.to_string());
            }
        }
    }

    let input = match input {
        Some(p) => p,
        None => {
            eprintln!("error: missing input path (expected a `.wit` file or `.wasm` component)");
            process::exit(1);
        }
    };
    let out_path = out_dir.as_deref().map(Path::new);
    match oneway::bindgen::run(Path::new(&input), out_path) {
        Ok(outcome) => {
            if outcome.written.is_empty() {
                eprintln!("warning: no interfaces found in `{}`", input);
            } else {
                for p in &outcome.written {
                    println!("wrote: {}", p.display());
                }
            }
            for note in &outcome.skipped {
                eprintln!("skipped: {}", note);
            }
        }
        Err(err) => {
            eprintln!("error: {}", err);
            process::exit(1);
        }
    }
}

fn cmd_fmt(args: &[String]) {
    let mut check_only = false;
    let mut files: Vec<String> = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--check" | "-c" => check_only = true,
            "--help" | "-h" => {
                println!("Usage: oneway fmt <file.ow> [--check]");
                println!();
                println!("  --check      Check whether files are formatted (exit 1 if not).");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown fmt flag '{}'", other);
                process::exit(1);
            }
            _ => files.push(arg.clone()),
        }
    }

    if files.is_empty() {
        eprintln!("error: missing input file(s)");
        process::exit(1);
    }

    let mut any_unformatted = false;

    for file_path in &files {
        let source = read_source(file_path);
        match formatter::format(&source) {
            Ok(formatted) => {
                if source == formatted {
                    continue;
                }
                any_unformatted = true;
                if check_only {
                    eprintln!("{}: not formatted", file_path);
                } else {
                    if let Err(err) = fs::write(file_path, &formatted) {
                        eprintln!("error: could not write '{}': {}", file_path, err);
                        process::exit(1);
                    }
                    println!("formatted: {}", file_path);
                }
            }
            Err(err) => {
                print_error(file_path, &err);
                process::exit(1);
            }
        }
    }

    if check_only && any_unformatted {
        process::exit(1);
    }
}

fn cmd_ast(args: &[String]) {
    let file_path = require_file(args);
    let source = read_source(file_path);
    let mut scanner = Scanner::new(&source);
    let tokens = match scanner.scan_tokens() {
        Ok(t) => t,
        Err(err) => {
            print_error(file_path, &err);
            process::exit(1);
        }
    };
    let mut parser = Parser::new(tokens);
    match parser.parse() {
        Ok(module) => println!("{:#?}", module),
        Err(err) => {
            print_error(file_path, &err);
            process::exit(1);
        }
    }
}

fn cmd_check(args: &[String]) {
    let parsed = parse_target_args(args, false);
    let target = resolve_target(parsed.target_path.as_deref());
    let target = apply_package_filter(target, parsed.package.as_deref());
    match target {
        Target::Build(spec) => {
            if !check_spec(&spec) {
                process::exit(1);
            }
        }
        Target::Workspace { members, label, .. } => {
            println!(
                "checking workspace `{}` ({} member(s))",
                label,
                members.len()
            );
            let mut failures = 0usize;
            for spec in &members {
                println!("\n-- {} --", spec.label);
                if !check_spec(spec) {
                    failures += 1;
                }
            }
            if failures > 0 {
                eprintln!("\n{}/{} member(s) failed.", failures, members.len());
                process::exit(1);
            }
            println!("\nAll {} member(s) checked clean.", members.len());
        }
    }
}

/// Run the checker on one buildable target. Returns `true` on success,
/// `false` if any errors were printed.
fn check_spec(spec: &BuildSpec) -> bool {
    let Some(loaded) = load_or_print(spec.entry_str()) else {
        return false;
    };
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(spec.entry_str(), err);
        }
        eprintln!("{} error(s) found.", errors.len());
        return false;
    }
    println!("All checks passed.");
    true
}

fn cmd_emit(args: &[String]) {
    let file_path = require_file(args);
    let loaded = load_or_exit(file_path);
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(file_path, err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let wat = codegen::generate_wat(&loaded.module);
    println!("{}", wat);
}

fn cmd_build(args: &[String]) {
    let parsed = parse_target_args(args, false);
    let target = resolve_target(parsed.target_path.as_deref());
    let target = apply_package_filter(target, parsed.package.as_deref());
    match target {
        Target::Build(spec) => {
            if !build_spec(&spec) {
                process::exit(1);
            }
        }
        Target::Workspace { members, label, .. } => {
            println!(
                "building workspace `{}` ({} member(s))",
                label,
                members.len()
            );
            let mut failures = 0usize;
            for spec in &members {
                println!("\n-- {} --", spec.label);
                if !build_spec(spec) {
                    failures += 1;
                }
            }
            if failures > 0 {
                eprintln!("\n{}/{} member(s) failed.", failures, members.len());
                process::exit(1);
            }
            println!("\nAll {} member(s) built successfully.", members.len());
        }
    }
}

/// Compile one buildable target to `<output_dir>/<stem>.{wasm,wit}`.
/// Returns `true` on success, `false` if the checker or filesystem errored.
fn build_spec(spec: &BuildSpec) -> bool {
    let Some(loaded) = load_or_print(spec.entry_str()) else {
        return false;
    };
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(spec.entry_str(), err);
        }
        eprintln!("{} error(s) found.", errors.len());
        return false;
    }
    let component_bytes = codegen::generate(&loaded.module);
    let wit_text = codegen::generate_wit(&loaded.module);
    let wasm_path = spec.output_dir.join(format!("{}.wasm", spec.output_stem));
    let wit_path = spec.output_dir.join(format!("{}.wit", spec.output_stem));
    if let Err(e) = fs::create_dir_all(&spec.output_dir) {
        eprintln!("error: {e}");
        return false;
    }
    if let Err(e) = fs::write(&wasm_path, &component_bytes) {
        eprintln!("error: {e}");
        return false;
    }
    if let Err(e) = fs::write(&wit_path, wit_text.as_bytes()) {
        eprintln!("error: {e}");
        return false;
    }
    println!("Compiled to: {}", wasm_path.display());
    println!("WIT world : {}", wit_path.display());
    true
}

/// `oneway test <file.ow>` — discover and run all `() -> TestResult`
/// functions defined in the entry file.
///
/// Test files look like normal Oneway modules:
///
/// ```text
/// use std/TestResult
///
/// testAdd = () -> TestResult {
///     Int(1).add(Int(2)).eq(Int(3)).assert("1 + 2 != 3")
/// }
/// ```
///
/// We load the module via the regular loader (so `use std/TestResult`
/// pulls in `Fail`, `Pass`, `assert`), collect every entry-file function
/// with a zero-arg `() -> TestResult` signature, then synthesise a `main`
/// that dispatches each test result to a pass/fail line. The synthesised
/// `main` is parsed from a generated source string and appended to the
/// module before checking, so it travels through the existing checker /
/// codegen / runtime pipeline unchanged.
fn cmd_test(args: &[String]) {
    let file_path = require_file(args);
    let mut loaded = load_or_exit(file_path);

    // Reject test files that try to define their own `main` — we synthesise it.
    if let Some(idx) = loaded.module.items[loaded.entry_items_start..]
        .iter()
        .position(|item| matches!(item, Item::Function(f) if f.name.name == "main"))
    {
        let item = &loaded.module.items[loaded.entry_items_start + idx];
        if let Item::Function(f) = item {
            eprintln!(
                "error[{}:{}:{}]: test files must not define `main` — `oneway test` synthesises one",
                file_path, f.span.line, f.span.column
            );
        }
        process::exit(1);
    }

    let tests: Vec<String> = loaded.module.items[loaded.entry_items_start..]
        .iter()
        .filter_map(|item| match item {
            Item::Function(f) if is_test_function(f) => Some(f.name.name.clone()),
            _ => None,
        })
        .collect();

    if tests.is_empty() {
        eprintln!(
            "error: no tests found in `{}` — a test is a function with signature `() -> TestResult`",
            file_path
        );
        process::exit(1);
    }

    // Synthesise a `main` that runs each test, parse it, and splice the
    // resulting items into the loaded module. Parsing the synthesised
    // source (rather than building AST by hand) keeps this code small
    // and means the runtime sees ordinary Oneway expressions.
    let synthesised = synthesise_test_main(&tests);
    let synth_items = match parse_synthesised(&synthesised) {
        Ok(items) => items,
        Err(err) => {
            eprintln!(
                "internal error: synthesised test harness failed to parse: {}",
                err.message()
            );
            eprintln!("---\n{}\n---", synthesised);
            process::exit(1);
        }
    };
    loaded.module.items.extend(synth_items);

    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(file_path, err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }

    println!("running {} test(s) from {}", tests.len(), file_path);
    let component_bytes = codegen::generate(&loaded.module);
    oneway::runtime::run_component(&component_bytes, &[]);
}

/// A test is a free, zero-arg function whose return type is the named
/// type `TestResult` (no generics). We deliberately don't require a name
/// prefix — the type itself is the marker.
fn is_test_function(f: &FunctionDef) -> bool {
    if f.receiver.is_some() || !f.params.is_empty() || f.name.name == "main" {
        return false;
    }
    matches!(
        &f.return_ty,
        TypeExpr::Named { name, generics, .. } if name == "TestResult" && generics.is_empty()
    )
}

fn synthesise_test_main(tests: &[String]) -> String {
    // ASCII markers keep the generated source clean of multi-byte escapes.
    //
    // Each test's result is dispatched on. The `Fail` arm prints a single
    // `[FAIL] testName: message` line by concatenating the per-test
    // banner with the assertion's message (the `String` payload of
    // `Fail = String`, unwrapped via `.String`). The `Pass` arm just
    // prints `[ ok ] testName`.
    let mut src = String::from("main = () -> Unit {\n");
    for name in tests {
        src.push_str(&format!("    {}().(\n", name));
        src.push_str(&format!(
            "        * (Fail) -> Unit {{ \"[FAIL] {}: \".concat(Fail.String).print() }}\n",
            name
        ));
        src.push_str(&format!(
            "        * (Pass) -> Unit {{ \"[ ok ] {}\".print() }}\n",
            name
        ));
        src.push_str("    )\n");
    }
    src.push_str("}\n");
    src
}

fn parse_synthesised(source: &str) -> Result<Vec<Item>, OnewayError> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    resolve_new_syntax(&mut module);
    Ok(module.items)
}

fn cmd_run(args: &[String]) {
    let parsed = parse_target_args(args, true);
    let target = resolve_target(parsed.target_path.as_deref());
    let target = apply_package_filter(target, parsed.package.as_deref());
    let program_args: Vec<&str> = parsed.program_args.iter().map(|s| s.as_str()).collect();
    let spec = match target {
        Target::Build(spec) => spec,
        Target::Workspace { label, members, .. } => {
            eprintln!(
                "error: `oneway run` on workspace `{}` is ambiguous \u{2014} pick a member",
                label
            );
            if !members.is_empty() {
                eprintln!("hint: try one of:");
                for m in &members {
                    eprintln!("  oneway run {}", m.label);
                }
            }
            process::exit(1);
        }
    };
    let loaded = load_or_exit(spec.entry_str());
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(spec.entry_str(), err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let component_bytes = codegen::generate(&loaded.module);
    oneway::runtime::run_component(&component_bytes, &program_args);
}

fn load_or_exit(file_path: &str) -> oneway::loader::LoadResult {
    match load_or_print(file_path) {
        Some(r) => r,
        None => process::exit(1),
    }
}

/// Like `load_or_exit`, but prints the error and returns `None` rather
/// than exiting. Used by workspace iteration so one member's load failure
/// doesn't terminate the whole run.
fn load_or_print(file_path: &str) -> Option<oneway::loader::LoadResult> {
    match loader::load_module(Path::new(file_path)) {
        Ok(r) => Some(r),
        Err(err) => {
            print_error(file_path, &err);
            None
        }
    }
}

const INSTALL_URL: &str = "https://raw.githubusercontent.com/almaju/oneway/main/install.sh";
const RELEASES_LATEST_URL: &str = "https://github.com/almaju/oneway/releases/latest";

fn cmd_upgrade(args: &[String]) {
    let mut check_only = false;
    let mut requested_version: Option<String> = None;
    for a in args {
        match a.as_str() {
            "--check" | "-c" => check_only = true,
            "--help" | "-h" => {
                println!("Usage: oneway upgrade [version] [--check]");
                println!();
                println!(
                    "  version      Install a specific release (e.g. v0.2.0). Defaults to latest."
                );
                println!("  --check      Only check whether a newer release is available.");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown upgrade flag '{}'", other);
                process::exit(1);
            }
            other => {
                if requested_version.is_some() {
                    eprintln!("error: upgrade accepts at most one version argument");
                    process::exit(1);
                }
                requested_version = Some(other.to_string());
            }
        }
    }

    if check_only {
        let latest = match fetch_latest_tag() {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error: could not check for latest release: {}", err);
                process::exit(1);
            }
        };
        let current = format!("v{}", VERSION);
        if normalize_tag(&latest) == normalize_tag(&current) {
            println!("oneway is up to date ({})", current);
        } else {
            println!(
                "A new version is available: {} (current: {})\nRun `oneway upgrade` to update.",
                latest, current
            );
        }
        return;
    }

    let curl = which("curl");
    let wget = which("wget");
    if curl.is_none() && wget.is_none() {
        eprintln!("error: `oneway upgrade` requires `curl` or `wget` to be installed");
        process::exit(1);
    }

    let fetch_cmd = if curl.is_some() {
        format!("curl -fsSL {}", INSTALL_URL)
    } else {
        format!("wget -qO- {}", INSTALL_URL)
    };
    let sh_args = match &requested_version {
        Some(v) => format!("sh -s -- {}", shell_escape(v)),
        None => "sh".to_string(),
    };
    let pipeline = format!("{} | {}", fetch_cmd, sh_args);

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("error: upgrade failed with exit status: {}", s);
            process::exit(s.code().unwrap_or(1));
        }
        Err(err) => {
            eprintln!("error: failed to run upgrade: {}", err);
            process::exit(1);
        }
    }
}

fn fetch_latest_tag() -> Result<String, String> {
    if which("curl").is_some() {
        let out = std::process::Command::new("curl")
            .args([
                "-fsSLI",
                "-o",
                "/dev/null",
                "-w",
                "%{url_effective}",
                RELEASES_LATEST_URL,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("curl exited with {}", out.status));
        }
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let tag = url.rsplit('/').next().unwrap_or("").to_string();
        if !looks_like_version_tag(&tag) {
            return Err("no published releases found".to_string());
        }
        return Ok(tag);
    }
    if which("wget").is_some() {
        let out = std::process::Command::new("wget")
            .args([
                "--max-redirect=10",
                "--server-response",
                "--spider",
                RELEASES_LATEST_URL,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let mut location: Option<String> = None;
        for line in combined.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("Location: ") {
                location = Some(rest.split_whitespace().next().unwrap_or("").to_string());
            }
        }
        if let Some(url) = location {
            let tag = url.rsplit('/').next().unwrap_or("").to_string();
            if looks_like_version_tag(&tag) {
                return Ok(tag);
            }
        }
        return Err("no published releases found".to_string());
    }
    Err("neither curl nor wget is available".to_string())
}

fn normalize_tag(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

fn looks_like_version_tag(tag: &str) -> bool {
    let rest = tag.strip_prefix('v').unwrap_or(tag);
    let mut chars = rest.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c.is_ascii_alphanumeric())
}

fn which(cmd: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    {
        s.to_string()
    } else {
        let escaped = s.replace('\'', "'\\''");
        format!("'{}'", escaped)
    }
}

fn print_error(file_path: &str, err: &OnewayError) {
    let span = err.span();
    eprintln!(
        "error[{}:{}:{}]: {}",
        file_path,
        span.line,
        span.column,
        err.message()
    );
}
