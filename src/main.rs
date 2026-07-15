use canon::ast::{resolve_new_syntax, FunctionDef, Item, TypeDef, TypeExpr};
use canon::checker;
use canon::codegen;
use canon::error::CanonError;
use canon::formatter;
use canon::lexer::Scanner;
use canon::loader::{self, LoadResult};
use canon::parser::Parser;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    // Toolchain launcher: when this binary is the installed launcher
    // (`~/.canon/bin/canon`), resolve the active toolchain and hand off to it.
    // `launch` strips a leading `stable`/`nightly` channel word; if the
    // resolved toolchain differs from this binary it execs it and never
    // returns. `canon use` and directly-run binaries (dev builds, an exec'd
    // toolchain) fall through and run in-process.
    let args = toolchain::launch(env::args().collect());

    if args.len() < 2 {
        print_help();
        process::exit(1);
    }

    let cmd = args[1].as_str();
    let rest: Vec<String> = args[2..].to_vec();

    match cmd {
        "run" => cmd_run(&rest),
        "build" => cmd_build(&rest),
        "check" => cmd_check(&rest),
        "test" => cmd_test(&rest),
        "inspect" => cmd_inspect(&rest),
        "bindgen" => cmd_bindgen(&rest),
        "install" => cmd_install(&rest),
        "publish" => cmd_publish(&rest),
        "lsp" => canon::lsp::run(),
        "upgrade" | "update" => cmd_upgrade(&rest),
        "use" => toolchain::cmd_use(&rest),
        "--version" | "-V" => {
            println!("canon {}", VERSION);
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
    println!("canon {} - the Canon language compiler", VERSION);
    println!();
    println!("Usage: canon <command> [args]");
    println!();
    println!("A target is either a package directory (containing `src/main.can`),");
    println!("a workspace directory (one whose immediate subdirectories are");
    println!("packages), or a single `.can` file. When omitted, defaults");
    println!("to the current directory.");
    println!();
    println!("Commands:");
    println!("  run [target] [-p name] [--addr <ip:port>] [args...]");
    println!("                            Compile and run a Canon program.");
    println!(
        "                            With `--addr`, serves a `wasi:http/handler` program over HTTP."
    );
    println!("  build [target] [-p name]  Compile to a WASM component (.wasm)");
    println!("  check [target] [-p name] [--fix]");
    println!("                            Check canonical form, sort order, and types.");
    println!("                            With `--fix`, rewrites what is mechanically");
    println!("                            fixable (formatting, ordering) in place first.");
    println!("  test <file.can | dir>     Run tests (`X = TestResult` + `Unit => X`). A");
    println!("                            directory runs every `*_test.can` file under it");
    println!("                            in one process, sharing setup across files.");
    println!("  inspect <stage> <file.can> Print an intermediate pipeline stage");
    println!("                              stages: tokens | ast");
    println!("  bindgen <wit-or-wasm> [-o <dir>]");
    println!(
        "                            Generate Canon bindings from a WIT package or WASM component"
    );
    println!("  install [target]          Materialize the WIT sources under `<target>/wit/`");
    println!(
        "                            into `<target>/bindgen/`. Target defaults to the current directory."
    );
    println!("  install <ns>:<name>[@ver] Fetch a package from its registry and vendor it");
    println!("                            under `deps/<ns>/<name>/`");
    println!("  publish <ns>:<name>[@ver] Publish the current directory's package to its");
    println!("                            registry. Without a version, patch-bumps the");
    println!("                            newest release (first publish is 0.1.0)");
    println!("  lsp                       Start the Language Server Protocol server");
    println!("  update [--check]          Update the active toolchain (alias: upgrade)");
    println!("  use [stable|nightly]      Show the active toolchain, or make this");
    println!("                            directory (and below) use one — installing it");
    println!("                            if needed. Run in ~ to set it for everything.");
    println!("  stable|nightly <command>  Run one command with that toolchain");
    println!("                            (e.g. `canon nightly run app.can`)");
    println!("  --version, -V             Print version");
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

/// A single buildable compilation target: a package (a directory with
/// `src/main.can`) or a loose `.can` file in single-file mode.
struct BuildSpec {
    /// Entry `.can` file the loader will read.
    entry: PathBuf,
    /// Where `build/` lives for this target — always the package's own
    /// `build/` (or, for a loose file, a per-stem subdir of the file's
    /// directory).
    output_dir: PathBuf,
    /// Stem used for output artifacts (`<stem>.wasm`, `<stem>.wit`). For a
    /// package it's the directory name; for a loose file it's the file
    /// stem.
    output_stem: String,
    /// Path the user typed (or the workspace member's display path), used
    /// as the context in error messages.
    label: String,
    /// Package name — the package directory's name. Empty for file-mode
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
/// `canon run|build|check` accept any of:
///
/// - a **package directory** (containing `src/main.can`),
/// - a **single `.can` file** (anonymous single-file package), or
/// - a **workspace directory** (one that is not itself a package but
///   whose immediate subdirectories include packages). Structure is the
///   declaration — there is no manifest or member list.
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
    /// by `canon run` (passed through to the program).
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
            // A member's name is its directory name.
            let matched: Vec<BuildSpec> = members.into_iter().filter(|s| s.name == want).collect();
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
    if path.join("src").join("main.can").is_file() {
        return Target::Build(resolve_package_spec(path, arg));
    }

    // Not a package itself: a directory whose immediate subdirectories
    // include packages is a workspace over them.
    let members = resolve_workspace_members(path, arg);
    if members.is_empty() {
        eprintln!(
            "error: `{}` is not a Canon package (no `src/main.can`) and none of \
             its subdirectories is one",
            arg
        );
        eprintln!(
            "hint: a package is a directory containing `src/main.can`; \
             pass a `.can` file directly to compile in single-file mode"
        );
        process::exit(1);
    }
    Target::Workspace {
        members,
        label: arg.to_string(),
    }
}

/// Build a `BuildSpec` for a package directory. The package's name is
/// its directory name and its artifacts land in its own `build/`.
fn resolve_package_spec(pkg_root: &Path, label: &str) -> BuildSpec {
    let entry = pkg_root.join("src").join("main.can");
    // `.` and `..` have no usable `file_name`; canonicalize to recover
    // the real directory name.
    let name = pkg_root
        .file_name()
        .map(|s| s.to_os_string())
        .or_else(|| {
            pkg_root
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_os_string()))
        })
        .and_then(|s| s.to_str().map(str::to_string))
        .unwrap_or_else(|| "out".to_string());
    BuildSpec {
        entry,
        output_dir: pkg_root.join("build"),
        output_stem: name.clone(),
        label: label.to_string(),
        name,
    }
}

fn resolve_file_spec(path: &Path, arg: &str) -> BuildSpec {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("canon")
        .to_string();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // File mode keeps a per-stem subdir so a directory full of `.can`
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

/// Discover a workspace's members: every immediate subdirectory of
/// `ws_root` that is a package (contains `src/main.can`). There is no
/// member list to declare — the directory structure is the workspace.
/// Members are returned sorted alphabetically by path; each builds into
/// its own `build/`.
fn resolve_workspace_members(ws_root: &Path, ws_label: &str) -> Vec<BuildSpec> {
    let read = match fs::read_dir(ws_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "error: could not list `{}` ({}): {}",
                ws_label,
                ws_root.display(),
                e
            );
            process::exit(1);
        }
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|d| d.path())
        .filter(|p| p.join("src").join("main.can").is_file())
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|p| {
            let label = p.to_string_lossy().into_owned();
            resolve_package_spec(&p, &label)
        })
        .collect()
}

/// `canon inspect <stage> <file.can>` — print one intermediate pipeline
/// stage to stdout. Replaces the old `tokens` / `ast` / `emit` triple:
/// each command was the same shape (load file, run pipeline up to a
/// point, dump it) so they collapse cleanly into a single verb with a
/// `stage` selector.
fn cmd_inspect(args: &[String]) {
    let mut stage: Option<&str> = None;
    let mut file_path: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print_inspect_help();
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown inspect flag '{}'", other);
                process::exit(1);
            }
            other if stage.is_none() => stage = Some(other),
            other if file_path.is_none() => file_path = Some(other),
            other => {
                eprintln!("error: unexpected argument '{}'", other);
                process::exit(1);
            }
        }
    }

    let stage = match stage {
        Some(s) => s,
        None => {
            print_inspect_help();
            process::exit(1);
        }
    };
    let file_path = match file_path {
        Some(p) => p,
        None => {
            eprintln!("error: missing <file.can>");
            print_inspect_help();
            process::exit(1);
        }
    };

    match stage {
        "tokens" => inspect_tokens(file_path),
        "ast" => inspect_ast(file_path),
        other => {
            eprintln!(
                "error: unknown stage '{}' (expected `tokens` or `ast`)",
                other
            );
            process::exit(1);
        }
    }
}

fn print_inspect_help() {
    println!("Usage: canon inspect <stage> <file.can>");
    println!();
    println!("  <stage>     One of:");
    println!("                tokens    Lexer output");
    println!("                ast       Parser output (Module debug dump)");
    println!("  <file.can>   Source file to inspect.");
}

fn inspect_tokens(file_path: &str) {
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

fn inspect_ast(file_path: &str) {
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

fn cmd_bindgen(args: &[String]) {
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
                println!("Usage: canon bindgen <wit-or-wasm> [-o <dir>]");
                println!();
                println!("  <wit-or-wasm>   A `.wit` file, a directory of `.wit` files, or a");
                println!("                  WebAssembly Component `.wasm` whose embedded WIT will");
                println!("                  be extracted.");
                println!("  -o <dir>        Output root (default: current directory).");
                println!();
                println!("Bindings are written as `<dir>/<namespace>/<package>/<interface>.can`,");
                println!("e.g. `wasi/clocks/monotonic_clock.can`.");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown bindgen flag '{}'", other);
                process::exit(1);
            }
            other => {
                if let Some(existing) = input.as_deref() {
                    eprintln!(
                        "error: multiple input paths given ('{}' and '{}')",
                        existing, other
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
    match canon::bindgen::run(Path::new(&input), out_path) {
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

fn cmd_install(args: &[String]) {
    let mut target: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("Usage: canon install [target | <namespace>:<name>[@<version>]]");
                println!();
                println!("  <ns>:<name>[@ver]   Fetch a package from its registry and vendor the");
                println!("               generated bindings under `deps/<ns>/<name>@<version>/`");
                println!("               of the current project. Without a");
                println!("               version, the newest release is installed; a prefix like");
                println!("               `@0.3` picks the newest matching release. Registries");
                println!("               resolve through the standard `wasm-pkg` config file");
                println!("               (shared with `wkg`); set CANON_REGISTRY_CONFIG to use");
                println!("               an alternate config.");
                println!();
                println!("  target       The project directory (containing `wit/`).");
                println!("               Defaults to the current directory.");
                println!();
                println!("For every WIT source under `<target>/wit/` (a `.wit` file or a");
                println!("directory of them), materializes the corresponding Canon bindings");
                println!("into `<target>/bindgen/` under `<ns>/<pkg>@<ver>/<iface>.can`.");
                println!("Wasm-component sources (`*.wasm`) are recorded as deferred.");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown install flag '{}'", other);
                process::exit(1);
            }
            other => {
                if let Some(existing) = target.as_deref() {
                    eprintln!(
                        "error: multiple targets given ('{}' and '{}')",
                        existing, other
                    );
                    process::exit(1);
                }
                target = Some(other.to_string());
            }
        }
    }

    // A `:` marks a registry spec (`<ns>:<name>[@ver]`) — paths can't
    // contain one in the position the grammar requires. Everything else
    // stays the local `wit/`-driven install.
    if let Some(spec_str) = target.as_deref().filter(|t| t.contains(':')) {
        let spec = match canon::registry::parse_spec(spec_str) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("error: {}", err);
                process::exit(1);
            }
        };
        // Vendor into the enclosing project when there is one, else
        // treat the current directory as the project root — the same
        // fallback the loader's `deps/` lookup uses.
        let cwd = PathBuf::from(".");
        let root = canon::install::find_project_root(&cwd).unwrap_or(cwd);
        match canon::registry::install_from_registry(&spec, &root) {
            Ok(outcome) => {
                for p in &outcome.written {
                    println!("wrote: {}", p.display());
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
        return;
    }

    let target_path = target
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match canon::install::install(&target_path) {
        Ok(outcome) => {
            if outcome.written.is_empty() && outcome.skipped.is_empty() {
                println!(
                    "no WIT sources under `{}/wit/` - nothing to install",
                    target_path.display()
                );
            } else {
                for p in &outcome.written {
                    println!("wrote: {}", p.display());
                }
                for note in &outcome.skipped {
                    eprintln!("skipped: {}", note);
                }
            }
        }
        Err(err) => {
            eprintln!("error: {}", err);
            process::exit(1);
        }
    }
}

fn cmd_publish(args: &[String]) {
    let mut spec_arg: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("Usage: canon publish <namespace>:<name>[@<version>]");
                println!();
                println!("Publishes the package rooted at the current directory to its");
                println!("registry: every `.can` file except the vendored `deps/` tree and");
                println!("derived directories, wrapped as a Canon source artifact. The");
                println!("dependency list is recorded from the `deps/` directory names.");
                println!();
                println!("Without `@<version>`, the newest published release is patch-bumped");
                println!("(a package with no releases starts at 0.1.0). Registries resolve");
                println!("through the standard `wasm-pkg` config file (shared with `wkg`);");
                println!("set CANON_REGISTRY_CONFIG to use an alternate config.");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown publish flag '{}'", other);
                process::exit(1);
            }
            other => {
                if let Some(existing) = spec_arg.as_deref() {
                    eprintln!(
                        "error: multiple specs given ('{}' and '{}')",
                        existing, other
                    );
                    process::exit(1);
                }
                spec_arg = Some(other.to_string());
            }
        }
    }
    let Some(spec_str) = spec_arg else {
        eprintln!("error: missing package spec (`canon publish <namespace>:<name>[@<version>]`)");
        process::exit(1);
    };
    let spec = match canon::registry::parse_spec(&spec_str) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error: {}", err);
            process::exit(1);
        }
    };
    let cwd = PathBuf::from(".");
    let root = canon::install::find_project_root(&cwd).unwrap_or(cwd);
    match canon::registry::publish_to_registry(&spec, &root) {
        Ok(outcome) => {
            println!("published: {}", outcome.coordinate);
            for f in &outcome.files {
                println!("  + {}", f);
            }
        }
        Err(err) => {
            eprintln!("error: {}", err);
            process::exit(1);
        }
    }
}

fn cmd_check(args: &[String]) {
    let mut fix = false;
    let filtered: Vec<String> = args
        .iter()
        .filter(|a| {
            if a.as_str() == "--fix" {
                fix = true;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    let parsed = parse_target_args(&filtered, false);
    let target = resolve_target(parsed.target_path.as_deref());
    let target = apply_package_filter(target, parsed.package.as_deref());
    match target {
        Target::Build(spec) => {
            if !check_spec(&spec, fix) {
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
                if !check_spec(spec, fix) {
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
///
/// With `fix`, every mechanically fixable error is repaired in place
/// before checking: the loaded sources are rewritten to canonical form
/// (which also resolves sort-order violations — the formatter sorts),
/// and the target is re-loaded so the checker sees what's now on disk.
/// Whatever the fixer can't repair is then reported as usual.
fn check_spec(spec: &BuildSpec, fix: bool) -> bool {
    let Some(mut loaded) = load_or_print(spec.entry_str()) else {
        return false;
    };
    if fix && apply_fixes(&loaded) {
        let Some(reloaded) = load_or_print(spec.entry_str()) else {
            return false;
        };
        loaded = reloaded;
    }
    let errors = checker::check_loaded(&loaded);
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
    let errors = checker::check_loaded(&loaded);
    if !errors.is_empty() {
        for err in &errors {
            print_error(spec.entry_str(), err);
        }
        eprintln!("{} error(s) found.", errors.len());
        return false;
    }
    let component_bytes = codegen::generate(&loaded.module);

    // Web apps get the three-file bundle instead of a `.wasm` + `.wit`
    // pair — the output is a directory you can serve as-is (or open
    // via `canon run`, which serves it for you).
    if canon::ast::find_web_entry(&loaded.module.items).is_some() {
        if let Err(e) =
            canon::webhost::write_bundle(&spec.output_dir, &spec.output_stem, &component_bytes)
        {
            eprintln!("error: {e}");
            return false;
        }
        println!(
            "Compiled to: {}",
            spec.output_dir
                .join(format!("{}.wasm", spec.output_stem))
                .display()
        );
        println!(
            "Web bundle : {} (index.html + canon-web.js; serve the directory, or `canon run`)",
            spec.output_dir.display()
        );
        return true;
    }

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

/// `canon test <file.can>` — discover and run every test declared in the
/// entry file.
///
/// Test files look like normal Canon modules. A test is a **result
/// newtype of `TestResult`** together with its nullary constructor — the
/// name is a type name (checked, sorted, resolvable) and the arrow stays
/// anonymous, like every other constructor in the language:
///
/// ```text
/// SumAddsOperands = TestResult
///
/// Unit => SumAddsOperands {
///     1 -> Sum(2) -> Eq(3) -> TestResult
/// }
/// ```
///
/// We load the module via the regular loader (referencing `TestResult`
/// pulls in `Fail`, `Pass`, and the `TestResult` constructor), collect
/// every entry-file newtype `X = TestResult` that has a nullary `Unit => X`
/// constructor, then synthesise a `main` that dispatches each test result
/// to a pass/fail line. The synthesised `main` is parsed from a generated
/// source string and appended to the module before checking, so it travels
/// through the existing checker / codegen / runtime pipeline unchanged.
fn cmd_test(args: &[String]) {
    let target = require_file(args);

    // A directory argument runs every `*_test.can` file underneath it in
    // one process, sharing the stdlib parse, the wasmtime engine, and the
    // tokio runtime across files. A file argument keeps the original
    // single-file behaviour.
    if Path::new(target).is_dir() {
        cmd_test_dir(target);
        return;
    }

    let Some((count, component_bytes)) = compile_test_file(target) else {
        process::exit(1);
    };
    println!("running {} test(s) from {}", count, target);
    canon::runtime::run_component(&component_bytes, &[]);
}

/// Discover every `*_test.can` file under `dir`, compile each to a
/// component, then run them all on one shared engine + runtime. The
/// stdlib is parsed once per file (loader-level memoisation) instead of
/// once per process, and the runtime/engine/linker are built once
/// instead of once per file — the two costs that dominated the old
/// process-per-file harness. Exits 1 if any file fails to compile or
/// reports a failing test.
fn cmd_test_dir(dir: &str) {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with("_test.can"))
            })
            .collect(),
        Err(err) => {
            eprintln!("error: could not read `{}`: {}", dir, err);
            process::exit(1);
        }
    };
    files.sort();

    if files.is_empty() {
        eprintln!(
            "error: no `*_test.can` files found under `{}`: a test file ends in `_test.can`",
            dir
        );
        process::exit(1);
    }

    let mut components: Vec<(String, Vec<u8>)> = Vec::new();
    let mut compile_failures = 0usize;
    for file in &files {
        let path = file.to_string_lossy();
        match compile_test_file(&path) {
            Some((count, bytes)) => {
                components.push((format!("running {} test(s) from {}", count, path), bytes));
            }
            None => compile_failures += 1,
        }
    }

    let run_failures = if components.is_empty() {
        0
    } else {
        canon::runtime::run_components(&components)
    };

    let clean = components.len() - run_failures;
    println!(
        "\n{} test file(s): {} clean, {} with failures",
        files.len(),
        clean,
        run_failures + compile_failures
    );
    if compile_failures > 0 || run_failures > 0 {
        process::exit(1);
    }
}

/// Load, check, and codegen one `*_test.can` file into a runnable
/// component, returning `(test count, component bytes)` — or `None`
/// after printing diagnostics. Shared by single-file `canon test <file>`
/// and the `canon test <dir>` batch runner, which is why load failures
/// print and return rather than exiting: one bad file in a directory
/// shouldn't abort the rest.
fn compile_test_file(file_path: &str) -> Option<(usize, Vec<u8>)> {
    let mut loaded = load_or_print(file_path)?;

    // Reject test files that try to define their own `main` — we synthesise it.
    if let Some(idx) = loaded.module.items[loaded.entry_items_start..]
        .iter()
        .position(|item| matches!(item, Item::Function(f) if f.name.name == "main"))
    {
        let item = &loaded.module.items[loaded.entry_items_start + idx];
        if let Item::Function(f) = item {
            eprintln!(
                "error[{}:{}:{}]: test files must not define `main`: `canon test` synthesises one",
                file_path, f.span.line, f.span.column
            );
        }
        return None;
    }

    let test_types: HashSet<String> = loaded.module.items[loaded.entry_items_start..]
        .iter()
        .filter_map(|item| match item {
            Item::TypeDef(t) if is_test_newtype(t) => Some(t.name.name.clone()),
            _ => None,
        })
        .collect();
    let tests: Vec<String> = loaded.module.items[loaded.entry_items_start..]
        .iter()
        .filter_map(|item| match item {
            Item::Function(f) if is_test_constructor(f, &test_types) => Some(
                f.receiver
                    .as_ref()
                    .expect("Self ctor has receiver")
                    .name
                    .clone(),
            ),
            _ => None,
        })
        .collect();

    if tests.is_empty() {
        eprintln!(
            "error: no tests found in `{}`: a test is a result newtype of `TestResult` \
             (`SumIsThree = TestResult`) with a nullary constructor (`Unit => SumIsThree {{ … }}`)",
            file_path
        );
        return None;
    }

    // Synthesise a `main` that runs each test, parse it, and splice the
    // resulting items into the loaded module. Parsing the synthesised
    // source (rather than building AST by hand) keeps this code small
    // and means the runtime sees ordinary Canon expressions.
    //
    // The harness also needs `wasi:cli/exit#exit-with-code` so a
    // failing run terminates the process with exit code 1. The binding
    // is synthesised as source too and inserted into the *import
    // region* of the module (before `entry_items_start`), where the
    // alphabetical-ordering rule doesn't apply to it.
    let exit_binding = "ExitWithCode = Unit\n\nInt => ExitWithCode {\n    \"exit-with-code\"\n}\n";
    let mut exit_items = match parse_binding(exit_binding) {
        Ok(items) => items,
        Err(err) => {
            eprintln!(
                "internal error: synthesised exit binding failed to parse: {}",
                err.message()
            );
            return None;
        }
    };
    canon::loader::apply_bindings(&mut exit_items, Some("wasi:cli/exit@0.3.0-rc-2026-03-15"));
    for item in exit_items.into_iter().rev() {
        loaded.module.items.insert(0, item);
        loaded.entry_items_start += 1;
    }

    let synthesised = synthesise_test_main(&tests);
    let mut synth_items = match parse_synthesised(&synthesised) {
        Ok(items) => items,
        Err(err) => {
            eprintln!(
                "internal error: synthesised test harness failed to parse: {}",
                err.message()
            );
            eprintln!("---\n{}\n---", synthesised);
            return None;
        }
    };
    // The harness main is the compiler's own, not user source — mark it
    // anonymous so the checker's "entries are anonymous" rule sees it
    // exactly like a user-written `Unit => Program { … }`.
    for item in &mut synth_items {
        if let Item::Function(f) = item {
            if f.name.name == "main" {
                f.anonymous = true;
            }
        }
    }
    loaded.module.items.extend(synth_items);

    let errors = checker::check_loaded(&loaded);
    if !errors.is_empty() {
        for err in &errors {
            print_error(file_path, err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        return None;
    }

    Some((tests.len(), codegen::generate(&loaded.module)))
}

/// A test's identity is a result newtype of `TestResult` — a plain,
/// non-generic alias `X = TestResult`. The name is a type name, so it is
/// checked and sorted like every other name in the language.
fn is_test_newtype(t: &TypeDef) -> bool {
    t.generic_params.is_empty()
        && matches!(
            &t.body,
            TypeExpr::Named { name, generics, .. } if name == "TestResult" && generics.is_empty()
        )
}

/// …and a test's body is the newtype's nullary constructor
/// (`Unit => X { … }`). After `resolve_new_syntax` a constructor carries
/// its constructed type as its *receiver* and the name `Self` (the lone
/// `Unit` input is already stripped to zero params), so discovery is:
/// a zero-param `Self` constructor whose receiver is a test newtype.
fn is_test_constructor(f: &FunctionDef, test_types: &HashSet<String>) -> bool {
    f.name.name == "Self"
        && f.params.is_empty()
        && f.receiver
            .as_ref()
            .is_some_and(|r| test_types.contains(&r.name))
}

fn synthesise_test_main(tests: &[String]) -> String {
    // ASCII markers keep the generated source clean of multi-byte escapes.
    //
    // Each test's result is dispatched on: the `Fail` arm prints a
    // `[FAIL] testName: message` line (the assertion message is the
    // `String` payload of `Fail = String`) and yields 1; the `Pass` arm
    // prints `[ ok ] testName` and yields 0. The per-test values are
    // summed with `.add` and the total failure count drives
    // `exit-with-code`: any failure exits 1, all-pass exits 0.
    let mut src = String::from("main = () => Unit {\n    ");
    for (i, name) in tests.iter().enumerate() {
        if i > 0 {
            src.push_str(".add(");
        }
        src.push_str(&format!("{}() -> (\n", name));
        src.push_str(&format!(
            "        * Fail => Int {{ \"[FAIL] {}: \".concat(Fail.String).print() 1 }}\n",
            name
        ));
        src.push_str(&format!(
            "        * Pass => Int {{ \"[ ok ] {}\".print() 0 }}\n",
            name
        ));
        src.push_str("    )");
        if i > 0 {
            src.push(')');
        }
    }
    src.push_str(".eq(0) -> (\n");
    src.push_str("        * False => Unit { ExitWithCode(1) }\n");
    src.push_str("        * True => Unit { ExitWithCode(0) }\n");
    src.push_str("    )\n");
    src.push_str("}\n");
    src
}

fn parse_synthesised(source: &str) -> Result<Vec<Item>, CanonError> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    let mut module = parser.parse()?;
    resolve_new_syntax(&mut module);
    Ok(module.items)
}

/// Parse a synthesised *binding* source without `resolve_new_syntax`:
/// as in the loader pipeline, `apply_bindings` must see the raw
/// anonymous constructor to lift its string-anchored WIT fragment (and
/// leaves it fully normalised, so no later resolution pass is needed).
fn parse_binding(source: &str) -> Result<Vec<Item>, CanonError> {
    let mut scanner = Scanner::new(source);
    let tokens = scanner.scan_tokens()?;
    let mut parser = Parser::new(tokens);
    Ok(parser.parse()?.items)
}

/// `canon run [target] [-p name] [--addr <ip:port>] [args...]`
///
/// Compiles the target Canon package or file, then either:
///
///   * runs it as a `wasi:cli/command` (the default), forwarding any
///     trailing arguments as program arguments; or
///   * serves it as a `wasi:http/handler` over HTTP when `--addr` is
///     given. The runtime opens a TCP listener at the given `ip:port`
///     and dispatches each incoming HTTP/1.1 request to the guest's
///     `handle` export through `wasmtime-wasi-http`.
///
/// Until the codegen learns to emit a `wasi:http/service` world (see
/// the compilation spec, docs/src/spec/compilation.md), the `--addr` mode will
/// fail at component-instantiation time — the diagnostic surfaces the
/// expected exports so users know what's missing.
fn cmd_run(args: &[String]) {
    // Peel off `--addr <ip:port>` (or `--addr=<ip:port>`) before the
    // rest of the arg parser, which expects `-p name`, a target path,
    // and then program args after `--`. `--addr` is a *runner* flag,
    // not a program arg, so it has to come out first.
    let mut addr: Option<String> = None;
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => {
                if i + 1 >= args.len() {
                    eprintln!("error: `--addr` requires an `ip:port` argument");
                    process::exit(1);
                }
                addr = Some(args[i + 1].clone());
                i += 2;
            }
            other if other.starts_with("--addr=") => {
                addr = Some(other["--addr=".len()..].to_string());
                i += 1;
            }
            _ => {
                filtered.push(args[i].clone());
                i += 1;
            }
        }
    }

    // Program args are only meaningful for command-style runs. In HTTP
    // mode there's no `argv` to thread — the guest is invoked per
    // request, not once at startup.
    let allow_program_args = addr.is_none();
    let parsed = parse_target_args(&filtered, allow_program_args);
    let target = resolve_target(parsed.target_path.as_deref());
    let target = apply_package_filter(target, parsed.package.as_deref());
    let program_args: Vec<&str> = parsed.program_args.iter().map(|s| s.as_str()).collect();
    let spec = match target {
        Target::Build(spec) => spec,
        Target::Workspace { label, members, .. } => {
            eprintln!(
                "error: `canon run` on workspace `{}` is ambiguous: pick a member",
                label
            );
            if !members.is_empty() {
                eprintln!("hint: try one of:");
                for m in &members {
                    eprintln!("  canon run {}", m.label);
                }
            }
            process::exit(1);
        }
    };
    let loaded = load_or_exit(spec.entry_str());
    let errors = checker::check_loaded(&loaded);
    if !errors.is_empty() {
        for err in &errors {
            print_error(spec.entry_str(), err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let component_bytes = codegen::generate(&loaded.module);

    // Web-app programs (the `init`/`update`/`view` triple, see
    // the web target, docs/src/reference/web-target.md) compile to a browser-run core module — nothing
    // for the embedded wasmtime to execute. Serve the three-file
    // bundle over HTTP instead so the browser can load it.
    if canon::ast::find_web_entry(&loaded.module.items).is_some() {
        let bind_addr: std::net::SocketAddr = match &addr {
            Some(raw) => raw.parse().unwrap_or_else(|e| {
                eprintln!("error: invalid `--addr` value `{}`: {}", raw, e);
                process::exit(1);
            }),
            None => "127.0.0.1:8080".parse().expect("static addr"),
        };
        if addr.is_none() {
            eprintln!("web app detected: serving on http://{bind_addr} (override with `canon run … --addr <ip:port>`)");
        }
        canon::webhost::serve_bundle(bind_addr, &spec.output_stem, component_bytes);
    }

    // HTTP-entry programs (a free `(Request) -> Response` function)
    // compile to a `wasi:http/service` component — there is no
    // `wasi:cli/run` to invoke. Serve them instead: on `--addr` when
    // given, else on a default local address so plain `canon run`
    // does the obvious thing.
    let is_http = loaded.module.items.iter().any(|item| match item {
        Item::Function(func) => {
            func.receiver.is_none()
                && canon::ast::entry_world_of(&func.return_ty) == Some(canon::ast::EntryWorld::Http)
        }
        _ => false,
    });

    match addr {
        Some(raw) => {
            let bind_addr: std::net::SocketAddr = raw.parse().unwrap_or_else(|e| {
                eprintln!("error: invalid `--addr` value `{}`: {}", raw, e);
                process::exit(1);
            });
            canon::runtime::serve_component(&component_bytes, bind_addr);
        }
        None if is_http => {
            let bind_addr: std::net::SocketAddr = "127.0.0.1:8080".parse().expect("static addr");
            eprintln!("HTTP handler detected: serving on http://{bind_addr} (override with `canon run … --addr <ip:port>`)");
            canon::runtime::serve_component(&component_bytes, bind_addr);
        }
        None => {
            canon::runtime::run_component(&component_bytes, &program_args);
        }
    }
}

fn load_or_exit(file_path: &str) -> LoadResult {
    match load_or_print(file_path) {
        Some(r) => r,
        None => process::exit(1),
    }
}

/// Like `load_or_exit`, but prints the error and returns `None` rather
/// than exiting. Used by workspace iteration so one member's load failure
/// doesn't terminate the whole run.
fn load_or_print(file_path: &str) -> Option<LoadResult> {
    // Auto-install: if the file lives inside a Canon project whose
    // `[imports]` are out-of-date with what's materialized under
    // `bindgen/`, run `canon install` first so the binding files exist
    // before the loader looks for them. This is what makes `canon run`
    // / `canon check` / `canon build` work without a separate
    // `canon install` step in normal use. Errors during the auto-step
    // are printed to stderr and treated as load failures.
    if !auto_install(file_path) {
        return None;
    }
    match loader::load_module(Path::new(file_path)) {
        Ok(r) => Some(r),
        Err(err) => {
            print_error(file_path, &err);
            None
        }
    }
}

/// Run `install::ensure_installed` for the project the given path lives
/// in. Returns `true` on success (including the no-project and
/// up-to-date cases); `false` if install was needed and failed. When an
/// install was actually run we print a brief note to stderr so the user
/// knows what happened.
fn auto_install(file_path: &str) -> bool {
    match canon::install::ensure_installed(Path::new(file_path)) {
        Ok(canon::install::EnsureOutcome::NoProject) => true,
        Ok(canon::install::EnsureOutcome::UpToDate) => true,
        Ok(canon::install::EnsureOutcome::Installed(outcome)) => {
            if !outcome.written.is_empty() {
                eprintln!(
                    "installed {} binding file(s) into bindgen/",
                    outcome.written.len()
                );
            }
            for note in &outcome.skipped {
                eprintln!("skipped: {}", note);
            }
            true
        }
        Err(err) => {
            eprintln!("error: install failed: {}", err);
            false
        }
    }
}

/// `canon check --fix`: rewrite every user-authored source that has
/// drifted from canonical form back into it — the write-side mirror of
/// the compiler's format phase (`checker::check_loaded`), repairing
/// exactly the errors that phase reports (formatting, including
/// sort-order violations). Returns whether anything was written, so
/// the caller knows to re-load. Same skips as the phase: `.md` assets
/// (synthesized Canon the author never edits), bundled packages, and
/// files that don't parse (the checker owns that diagnostic).
fn apply_fixes(loaded: &LoadResult) -> bool {
    let mut wrote = false;
    for src in &loaded.local_sources {
        if src.path.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        let Ok(canonical) = formatter::format(&src.source) else {
            continue;
        };
        if canonical == src.source {
            continue;
        }
        if let Err(err) = fs::write(&src.path, &canonical) {
            eprintln!("error: could not write '{}': {}", src.path.display(), err);
            process::exit(1);
        }
        println!("fixed: {}", src.path.display());
        wrote = true;
    }
    wrote
}

const INSTALL_URL: &str = "https://raw.githubusercontent.com/almaju/canon/main/install.sh";
const RELEASES_LATEST_URL: &str = "https://github.com/almaju/canon/releases/latest";

fn cmd_upgrade(args: &[String]) {
    let mut check_only = false;
    for a in args {
        match a.as_str() {
            "--check" | "-c" => check_only = true,
            "--help" | "-h" => {
                println!("Usage: canon update [--check]   (alias: upgrade)");
                println!();
                println!("  Updates the active toolchain to the latest build on its channel.");
                println!("  --check   Only check whether a newer stable release is available.");
                println!();
                println!("  To switch toolchains, see `canon use` or run a single command");
                println!("  with `canon stable <cmd>` / `canon nightly <cmd>`.");
                return;
            }
            other => {
                eprintln!("error: unknown upgrade flag '{}'", other);
                process::exit(1);
            }
        }
    }

    // The toolchain the launcher resolved us to (set on exec), else whatever the
    // current directory / default resolves to.
    let channel = env::var("CANON_RESOLVED")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(toolchain::active_channel);

    if check_only {
        if channel == toolchain::NIGHTLY {
            println!(
                "The nightly toolchain is published continuously (current: {}).\n\
                 Run `canon upgrade` to pull the latest nightly.",
                VERSION
            );
            return;
        }
        let latest = match fetch_latest_tag() {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error: could not check for latest release: {}", err);
                process::exit(1);
            }
        };
        let current = format!("v{}", VERSION);
        if normalize_tag(&latest) == normalize_tag(&current) {
            println!("canon is up to date ({})", current);
        } else {
            println!(
                "A new version is available: {} (current: {})\nRun `canon upgrade` to update.",
                latest, current
            );
        }
        return;
    }

    println!("Updating the '{}' toolchain…", channel);
    toolchain::install(&channel);
}

/// Toolchain management, canon-style: two concepts, nothing else.
///
/// One installation holds both channels under `<install>/toolchains/<name>/`,
/// and the on-PATH `<install>/bin/canon` is a thin launcher that resolves the
/// active toolchain and execs it.
///
/// 1. `canon use [stable|nightly]` — scoped by where you run it: records
///    "this directory and below use X" in a central registry (nothing in the
///    project). Run it at `~` and it is your global default. Using a channel
///    that isn't installed installs it first.
/// 2. `canon stable <cmd>` / `canon nightly <cmd>` — one-shot: the first word
///    picks the toolchain, like a dispatch arm.
///
/// Resolution: explicit channel word → `CANON_TOOLCHAIN` env (CI escape
/// hatch) → nearest `use` ancestor → `stable`. When the fallback lands on a
/// channel that isn't installed, the sole installed toolchain (or the
/// launcher binary itself) runs instead — there is no separate "default"
/// state to configure.
mod toolchain {
    use super::{shell_escape, which, INSTALL_URL};
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;

    pub const STABLE: &str = "stable";
    pub const NIGHTLY: &str = "nightly";

    /// Install prefix, matching install.sh: `$CANON_INSTALL` or `$HOME/.canon`.
    fn install_dir() -> Option<PathBuf> {
        if let Some(dir) = env::var_os("CANON_INSTALL") {
            return Some(PathBuf::from(dir));
        }
        env::var_os("HOME").map(|home| PathBuf::from(home).join(".canon"))
    }

    fn toolchains_dir() -> Option<PathBuf> {
        install_dir().map(|d| d.join("toolchains"))
    }
    fn uses_file() -> Option<PathBuf> {
        install_dir().map(|d| d.join("uses"))
    }

    fn exe_name() -> &'static str {
        if cfg!(windows) {
            "canon.exe"
        } else {
            "canon"
        }
    }

    /// The toolchain binary path, if that toolchain is installed.
    fn toolchain_bin(name: &str) -> Option<PathBuf> {
        let p = toolchains_dir()?.join(name).join(exe_name());
        p.is_file().then_some(p)
    }

    /// True when the running binary is the installed launcher (`<install>/bin`).
    fn is_launcher() -> bool {
        let exe = match env::current_exe() {
            Ok(e) => e,
            Err(_) => return false,
        };
        let parent = match exe.parent() {
            Some(p) => p,
            None => return false,
        };
        // `current_exe` is canonical (via /proc/self/exe on Linux); canonicalize
        // the bin dir too so a symlinked HOME (or /tmp) still matches.
        let bindir = match install_dir() {
            Some(d) => d.join("bin"),
            None => return false,
        };
        let bindir = bindir.canonicalize().unwrap_or(bindir);
        parent == bindir
    }

    fn env_toolchain() -> Option<String> {
        env::var("CANON_TOOLCHAIN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// The nearest `canon use` ancestor covering the current directory:
    /// longest-prefix match wins, so a deeper `use` shadows one above it.
    fn nearest_use() -> Option<(String, String)> {
        let cwd = env::current_dir().ok()?;
        let cwd = cwd.canonicalize().unwrap_or(cwd);
        let mut best: Option<(usize, String, String)> = None;
        for (path, tc) in read_uses() {
            if cwd.starts_with(&path) {
                let len = Path::new(&path).components().count();
                if best.as_ref().map(|(l, _, _)| len > *l).unwrap_or(true) {
                    best = Some((len, path, tc));
                }
            }
        }
        best.map(|(_, path, tc)| (path, tc))
    }

    /// Where the active toolchain choice came from.
    enum Source {
        Word,
        Env,
        Use(String),
        Fallback,
    }

    fn resolve(word: Option<&str>) -> (String, Source) {
        if let Some(w) = word {
            return (w.to_string(), Source::Word);
        }
        if let Some(e) = env_toolchain() {
            return (e, Source::Env);
        }
        if let Some((path, tc)) = nearest_use() {
            return (tc, Source::Use(path));
        }
        (STABLE.to_string(), Source::Fallback)
    }

    /// The channel a bare `canon` would use here (no channel word).
    pub fn active_channel() -> String {
        resolve(None).0
    }

    /// Front door from `main`: when we are the launcher, hand off to the
    /// resolved toolchain. Returns args with any leading channel word removed.
    pub fn launch(mut args: Vec<String>) -> Vec<String> {
        // A toolchain we exec sets CANON_RESOLVED; never re-dispatch then.
        if env::var_os("CANON_RESOLVED").is_some() {
            take_channel_word(&mut args);
            return args;
        }
        let word = take_channel_word(&mut args);
        // `canon use` manages the launcher's own state; run it in-process.
        let is_mgmt = matches!(args.get(1).map(String::as_str), Some("use"));
        if !is_launcher() || is_mgmt {
            return args;
        }
        let (requested, source) = resolve(word.as_deref());
        match toolchain_bin(&requested) {
            Some(bin) => {
                if env::current_exe().ok().as_ref() != Some(&bin) {
                    exec_toolchain(&bin, &requested, &args);
                }
                args
            }
            None => {
                if matches!(source, Source::Fallback) {
                    // Nothing was chosen and stable isn't on disk. If exactly
                    // one toolchain is installed there is no ambiguity — run
                    // it; otherwise the launcher binary itself is a full
                    // toolchain, so run in-process.
                    let installed = installed_toolchains();
                    if let [only] = installed.as_slice() {
                        if let Some(bin) = toolchain_bin(only) {
                            exec_toolchain(&bin, only, &args);
                        }
                    }
                    return args;
                }
                eprintln!(
                    "error: toolchain '{}' is not installed.\n       \
                     Install and select it with: canon use {}",
                    requested, requested
                );
                process::exit(1);
            }
        }
    }

    /// Remove and return a leading `stable`/`nightly` channel word, if present.
    fn take_channel_word(args: &mut Vec<String>) -> Option<String> {
        if args.len() >= 2 && is_channel(&args[1]) {
            return Some(args.remove(1));
        }
        None
    }

    fn exec_toolchain(bin: &Path, name: &str, args: &[String]) -> ! {
        let mut cmd = process::Command::new(bin);
        cmd.args(&args[1..]);
        cmd.env("CANON_RESOLVED", name);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = cmd.exec();
            eprintln!("error: failed to launch toolchain '{}': {}", name, err);
            process::exit(1);
        }
        #[cfg(not(unix))]
        {
            match cmd.status() {
                Ok(s) => process::exit(s.code().unwrap_or(1)),
                Err(e) => {
                    eprintln!("error: failed to launch toolchain '{}': {}", name, e);
                    process::exit(1);
                }
            }
        }
    }

    fn is_channel(name: &str) -> bool {
        name == STABLE || name == NIGHTLY
    }

    /// Download and install a channel's toolchain via install.sh.
    pub fn install(channel: &str) {
        let curl = which("curl");
        let wget = which("wget");
        if curl.is_none() && wget.is_none() {
            eprintln!("error: installing a toolchain requires `curl` or `wget`");
            process::exit(1);
        }
        let fetch = if curl.is_some() {
            format!("curl -fsSL {}", INSTALL_URL)
        } else {
            format!("wget -qO- {}", INSTALL_URL)
        };
        let pipeline = format!("{} | CANON_CHANNEL={} sh", fetch, shell_escape(channel));
        let status = process::Command::new("sh")
            .arg("-c")
            .arg(&pipeline)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("error: toolchain install failed with status: {}", s);
                process::exit(s.code().unwrap_or(1));
            }
            Err(err) => {
                eprintln!("error: failed to run installer: {}", err);
                process::exit(1);
            }
        }
    }

    fn installed_toolchains() -> Vec<String> {
        let mut names = Vec::new();
        if let Some(dir) = toolchains_dir() {
            if let Ok(rd) = fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    if entry.path().is_dir() {
                        if let Some(n) = entry.file_name().to_str() {
                            names.push(n.to_string());
                        }
                    }
                }
            }
        }
        names.sort();
        names
    }

    fn read_uses() -> Vec<(String, String)> {
        uses_file()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|c| {
                c.lines()
                    .filter_map(|l| {
                        l.trim()
                            .rsplit_once('\t')
                            .map(|(a, b)| (a.to_string(), b.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn write_uses(entries: &[(String, String)]) -> Result<(), String> {
        let path = uses_file().ok_or("could not determine install directory")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let body: String = entries
            .iter()
            .map(|(p, tc)| format!("{}\t{}\n", p, tc))
            .collect();
        fs::write(&path, body).map_err(|e| e.to_string())
    }

    fn cwd_key() -> String {
        let cwd = env::current_dir().unwrap_or_default();
        cwd.canonicalize()
            .unwrap_or(cwd)
            .to_string_lossy()
            .into_owned()
    }

    /// `canon use` — show the active toolchain, or select one for this
    /// directory tree (installing it first when it isn't on disk).
    pub fn cmd_use(args: &[String]) {
        match args.first().map(String::as_str) {
            None => {
                let (active, source) = resolve(None);
                match source {
                    Source::Word => unreachable!("no channel word reaches cmd_use"),
                    Source::Env => println!("{} (CANON_TOOLCHAIN)", active),
                    Source::Use(path) => println!("{} (canon use in {})", active, path),
                    Source::Fallback => println!("{}", active),
                }
                let installed = installed_toolchains();
                if !installed.is_empty() {
                    println!();
                    println!("installed:");
                    for n in installed {
                        println!("  {}", n);
                    }
                }
            }
            Some("--help") | Some("-h") => {
                println!("Usage: canon use [stable|nightly]");
                println!();
                println!("  With no argument, shows the active toolchain and where the");
                println!("  choice comes from.");
                println!("  With a channel, this directory (and everything below it) uses");
                println!("  that toolchain — installing it first if needed. Run it in your");
                println!("  home directory to set the toolchain for everything.");
            }
            Some(ch) if is_channel(ch) => {
                if toolchain_bin(ch).is_none() {
                    println!("Toolchain '{}' is not installed — installing…", ch);
                    install(ch);
                    if toolchain_bin(ch).is_none() {
                        eprintln!(
                            "error: install finished but toolchain '{}' was not found",
                            ch
                        );
                        process::exit(1);
                    }
                }
                let key = cwd_key();
                let mut entries = read_uses();
                entries.retain(|(p, _)| p != &key);
                entries.push((key.clone(), ch.to_string()));
                entries.sort();
                if let Err(e) = write_uses(&entries) {
                    eprintln!("error: could not record the selection: {}", e);
                    process::exit(1);
                }
                println!("Using {} in {} (and below).", ch, key);
            }
            Some(other) => {
                eprintln!(
                    "error: unknown toolchain '{}' (expected 'stable' or 'nightly')",
                    other
                );
                process::exit(1);
            }
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

fn print_error(file_path: &str, err: &CanonError) {
    let span = err.span();
    // Format errors carry the offending file themselves — in a
    // multi-file load their span points into that file, not the entry.
    let path = match err {
        CanonError::FormatError { path, .. } => path.as_str(),
        _ => file_path,
    };
    eprintln!(
        "error[{}:{}:{}]: {}",
        path,
        span.line,
        span.column,
        err.message()
    );
}
