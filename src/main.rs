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
        "tokens" => cmd_tokens(&rest),
        "fmt" | "format" => cmd_fmt(&rest),
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
    println!("  build <file.ow>           Compile to a native binary");
    println!("  emit <file.ow>            Print generated Rust");
    println!("  ast <file.ow>             Print the parsed AST");
    println!("  check <file.ow>           Check sort order and types");
    println!("  tokens <file.ow>          Print lexer tokens");
    println!("  fmt <file.ow> [--check]   Format an Oneway source file");
    println!("  upgrade [version]         Update oneway to the latest (or given) release");
    println!("  upgrade --check           Check whether a newer release is available");
    println!("  version                   Print version");
    println!("  help                      Print this message");
    println!();
    println!("`run` and `build` require `rustc` (and `cargo` for async programs)");
    println!("to be installed on PATH.");
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
    let generated = codegen::generate_with_meta(&loaded.module);
    let source = combine_source(&loaded.rust_preludes, &generated.source);
    println!("{}", source);
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
    let generated = codegen::generate_with_meta(&loaded.module);
    let source = combine_source(&loaded.rust_preludes, &generated.source);
    let out_path = strip_ow_extension(file_path);
    if generated.is_async || !loaded.cargo_deps.is_empty() {
        compile_with_cargo(&out_path, &source, &loaded.cargo_deps);
    } else {
        compile_with_rustc(&out_path, &source);
    }
    println!("Compiled to: {}", out_path);
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
    let generated = codegen::generate_with_meta(&loaded.module);
    let source = combine_source(&loaded.rust_preludes, &generated.source);

    let tmp_dir = match tempdir_for_run() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("error: could not create temp dir: {}", err);
            process::exit(1);
        }
    };
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway_run");
    let out_path = tmp_dir.join(stem);
    let out_str = out_path.to_string_lossy().to_string();

    if generated.is_async || !loaded.cargo_deps.is_empty() {
        compile_with_cargo(&out_str, &source, &loaded.cargo_deps);
    } else {
        compile_with_rustc(&out_str, &source);
    }

    let status = std::process::Command::new(&out_path)
        .args(&program_args)
        .status();

    let _ = fs::remove_dir_all(&tmp_dir);

    match status {
        Ok(s) => process::exit(s.code().unwrap_or(1)),
        Err(err) => {
            eprintln!("error: failed to execute program: {}", err);
            process::exit(1);
        }
    }
}

fn combine_source(preludes: &[&'static str], body: &str) -> String {
    if preludes.is_empty() {
        return body.to_string();
    }
    let mut s = String::new();
    for p in preludes {
        s.push_str(p);
        if !p.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
    }
    s.push_str(body);
    s
}

fn strip_ow_extension(file_path: &str) -> String {
    if let Some(stripped) = file_path.strip_suffix(".ow") {
        stripped.to_string()
    } else {
        file_path.to_string()
    }
}

fn tempdir_for_run() -> std::io::Result<PathBuf> {
    let base = env::temp_dir();
    let unique = format!(
        "oneway-run-{}-{}",
        process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = base.join(unique);
    fs::create_dir_all(&dir)?;
    Ok(dir)
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

fn compile_with_rustc(out_path: &str, source: &str) {
    let rs_path = format!("{}.rs", out_path);
    if let Err(err) = fs::write(&rs_path, source) {
        eprintln!("error: writing {}: {}", rs_path, err);
        process::exit(1);
    }
    let status = std::process::Command::new("rustc")
        .arg(&rs_path)
        .arg("-o")
        .arg(out_path)
        .status();
    match status {
        Ok(s) if s.success() => {
            let _ = fs::remove_file(&rs_path);
        }
        Ok(s) => {
            eprintln!("error: rustc failed with: {}", s);
            process::exit(1);
        }
        Err(err) => {
            eprintln!("error: failed to run rustc: {}", err);
            eprintln!("       `rustc` must be installed and on PATH.");
            process::exit(1);
        }
    }
}

fn compile_with_cargo(out_path: &str, source: &str, cargo_deps: &[&oneway::loader::CargoDep]) {
    let project_name = Path::new(out_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("oneway_build");
    let build_dir = format!("{}.cargo", out_path);
    let src_dir = format!("{}/src", build_dir);
    if let Err(err) = fs::create_dir_all(&src_dir) {
        eprintln!("error: creating {}: {}", src_dir, err);
        process::exit(1);
    }
    let mut cargo_toml = format!(
        "[package]\nname = \"{}\"\nedition = \"2021\"\nversion = \"0.0.0\"\n\n[dependencies]\n",
        sanitize_crate_name(project_name)
    );
    let mut emitted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let emit_dep = |toml: &mut String, name: &str, version: &str, features: &[&str]| {
        toml.push_str(&format!("{} = {{ version = \"{}\"", name, version));
        if !features.is_empty() {
            toml.push_str(", features = [");
            for (i, f) in features.iter().enumerate() {
                if i > 0 {
                    toml.push_str(", ");
                }
                toml.push_str(&format!("\"{}\"", f));
            }
            toml.push(']');
        }
        toml.push_str(" }\n");
    };
    for dep in cargo_deps {
        if emitted.insert(dep.name) {
            emit_dep(&mut cargo_toml, dep.name, dep.version, dep.features);
        }
    }
    if !emitted.contains("tokio") && source.contains("#[tokio::main]") {
        emit_dep(&mut cargo_toml, "tokio", "1", &["full"]);
    }
    if let Err(err) = fs::write(format!("{}/Cargo.toml", build_dir), cargo_toml) {
        eprintln!("error: writing Cargo.toml: {}", err);
        process::exit(1);
    }
    if let Err(err) = fs::write(format!("{}/main.rs", src_dir), source) {
        eprintln!("error: writing main.rs: {}", err);
        process::exit(1);
    }
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--quiet")
        .current_dir(&build_dir)
        .status();
    match status {
        Ok(s) if s.success() => {
            let crate_name = sanitize_crate_name(project_name);
            let bin = format!("{}/target/release/{}", build_dir, crate_name);
            if let Err(err) = fs::copy(&bin, out_path) {
                eprintln!("error: copying binary: {}", err);
                process::exit(1);
            }
        }
        Ok(s) => {
            eprintln!("error: cargo build failed with: {}", s);
            process::exit(1);
        }
        Err(err) => {
            eprintln!("error: failed to run cargo: {}", err);
            eprintln!("       `cargo` must be installed and on PATH for programs with Rust deps.");
            process::exit(1);
        }
    }
}

fn sanitize_crate_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        format!("_{}", s)
    } else if s.is_empty() {
        "oneway_build".to_string()
    } else {
        s
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
