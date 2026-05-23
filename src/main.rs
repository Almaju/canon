use oneway::ast::{resolve_new_syntax, FunctionDef, Item, TypeExpr};
use oneway::checker;
use oneway::codegen;
use oneway::error::OnewayError;
use oneway::formatter;
use oneway::lexer::Scanner;
use oneway::loader;
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
    println!("oneway {} — the Oneway language compiler", VERSION);
    println!();
    println!("Usage: oneway <command> [args]");
    println!();
    println!("Commands:");
    println!("  run <file.ow> [args...]   Compile and run an Oneway program");
    println!("  build <file.ow>           Compile to a WASM component (.wasm)");
    println!("  emit <file.ow>            Print generated WAT (WebAssembly Text)");
    println!("  ast <file.ow>             Print the parsed AST");
    println!("  check <file.ow>           Check sort order and types");
    println!("  test <file.ow>            Run `() -> TestResult` functions as tests");
    println!("  tokens <file.ow>          Print lexer tokens");
    println!("  fmt <file.ow> [--check]   Format an Oneway source file");
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
    println!("All checks passed.");
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
    let component_bytes = codegen::generate(&loaded.module);
    let wit_text = codegen::generate_wit(&loaded.module);
    let build_dir = build_dir_for(file_path);
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out");
    let wasm_path = build_dir.join(format!("{}.wasm", stem));
    let wit_path = build_dir.join(format!("{}.wit", stem));
    fs::create_dir_all(&build_dir).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    fs::write(&wasm_path, &component_bytes).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    fs::write(&wit_path, wit_text.as_bytes()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    println!("Compiled to: {}", wasm_path.display());
    println!("WIT world : {}", wit_path.display());
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
    let file_path = require_file(args);
    let program_args: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();
    let loaded = load_or_exit(file_path);
    let errors = checker::check_with_entry(&loaded.module, loaded.entry_items_start);
    if !errors.is_empty() {
        for err in &errors {
            print_error(file_path, err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }
    let component_bytes = codegen::generate(&loaded.module);
    oneway::runtime::run_component(&component_bytes, &program_args);
}

fn build_dir_for(file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway");
    dir.join(".oneway").join(stem)
}

fn load_or_exit(file_path: &str) -> oneway::loader::LoadResult {
    match loader::load_module(Path::new(file_path)) {
        Ok(r) => r,
        Err(err) => {
            print_error(file_path, &err);
            process::exit(1);
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
