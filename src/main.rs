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
    println!("  build <file.ow>           Compile to a native binary");
    println!("  emit <file.ow>            Print generated Rust");
    println!("  ast <file.ow>             Print the parsed AST");
    println!("  check <file.ow>           Check sort order and types");
    println!("  tokens <file.ow>          Print lexer tokens");
    println!("  fmt <file.ow> [--check]   Format an Oneway source file");
    println!("  lsp                       Start the Language Server Protocol server");
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
    let build_dir = build_dir_for(file_path);
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway_build");
    // Binary lives inside .oneway/<stem>/<stem> — never next to the source file.
    let bin_path = build_dir.join(stem);
    let out_path = bin_path.to_string_lossy().to_string();
    let is_cargo = generated.is_async || !loaded.cargo_deps.is_empty();
    if source_is_cached(&build_dir, is_cargo, &source, &bin_path) {
        println!("Up to date: {}", out_path);
        return;
    }
    if is_cargo {
        compile_with_cargo(
            &build_dir,
            stem,
            &out_path,
            &source,
            &loaded.cargo_deps,
            true,
        );
    } else {
        compile_with_rustc(&build_dir, &out_path, &source, true);
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

    let build_dir = build_dir_for(file_path);
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway_run");
    let bin_path = build_dir.join("bin");
    let bin_str = bin_path.to_string_lossy().to_string();
    let is_cargo = generated.is_async || !loaded.cargo_deps.is_empty();

    if !source_is_cached(&build_dir, is_cargo, &source, &bin_path) {
        if is_cargo {
            compile_with_cargo(
                &build_dir,
                stem,
                &bin_str,
                &source,
                &loaded.cargo_deps,
                false,
            );
        } else {
            compile_with_rustc(&build_dir, &bin_str, &source, false);
        }
    }

    let status = std::process::Command::new(&bin_path)
        .args(&program_args)
        .status();

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

fn build_dir_for(file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("oneway");
    dir.join(".oneway").join(stem)
}

fn source_is_cached(build_dir: &Path, is_cargo: bool, source: &str, bin_path: &Path) -> bool {
    if !bin_path.exists() {
        return false;
    }
    let stored = if is_cargo {
        build_dir.join("cargo").join("src").join("main.rs")
    } else {
        build_dir.join("source.rs")
    };
    fs::read_to_string(&stored)
        .map(|s| s == source)
        .unwrap_or(false)
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

fn compile_with_rustc(build_dir: &Path, out_path: &str, source: &str, release: bool) {
    if let Err(err) = fs::create_dir_all(build_dir) {
        eprintln!("error: creating build dir {}: {}", build_dir.display(), err);
        process::exit(1);
    }
    let rs_path = build_dir.join("source.rs");
    if let Err(err) = fs::write(&rs_path, source) {
        eprintln!("error: writing {}: {}", rs_path.display(), err);
        process::exit(1);
    }
    let incremental_dir = build_dir.join("incremental");
    let mut cmd = std::process::Command::new("rustc");
    cmd.arg(&rs_path)
        .arg("-o")
        .arg(out_path)
        .arg(format!("-Cincremental={}", incremental_dir.display()));
    if release {
        cmd.arg("-Copt-level=3");
    } else {
        cmd.arg("-Copt-level=0");
    }
    match cmd.status() {
        Ok(s) if s.success() => {}
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

fn compile_with_cargo(
    build_dir: &Path,
    project_name: &str,
    out_path: &str,
    source: &str,
    cargo_deps: &[&oneway::loader::CargoDep],
    release: bool,
) {
    let cargo_dir = build_dir.join("cargo");
    let src_dir = cargo_dir.join("src");
    if let Err(err) = fs::create_dir_all(&src_dir) {
        eprintln!("error: creating {}: {}", src_dir.display(), err);
        process::exit(1);
    }
    let crate_name = sanitize_crate_name(project_name);
    let mut cargo_toml = format!(
        "[package]\nname = \"{}\"\nedition = \"2021\"\nversion = \"0.0.0\"\n\n[dependencies]\n",
        crate_name
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
    if let Err(err) = fs::write(cargo_dir.join("Cargo.toml"), &cargo_toml) {
        eprintln!("error: writing Cargo.toml: {}", err);
        process::exit(1);
    }
    if let Err(err) = fs::write(src_dir.join("main.rs"), source) {
        eprintln!("error: writing main.rs: {}", err);
        process::exit(1);
    }
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build").arg("--quiet").current_dir(&cargo_dir);
    if release {
        cmd.arg("--release");
    }
    match cmd.status() {
        Ok(s) if s.success() => {
            let profile = if release { "release" } else { "debug" };
            let bin = cargo_dir.join("target").join(profile).join(&crate_name);
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
            eprintln!("       `cargo` must be installed and on PATH.");
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
