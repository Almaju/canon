use oneway::checker;
use oneway::codegen;
use oneway::error::OnewayError;
use oneway::lexer::Scanner;
use oneway::loader;
use oneway::parser::Parser;
use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: oneway <file.ow> [--tokens|--ast|--check|--emit-rust|--compile]");
        process::exit(1);
    }

    let mut file_path: Option<&str> = None;
    let mut mode = "default";

    for arg in &args[1..] {
        match arg.as_str() {
            "--tokens" => mode = "tokens",
            "--ast" => mode = "ast",
            "--check" => mode = "check",
            "--emit-rust" => mode = "emit-rust",
            "--compile" => mode = "compile",
            s if s.starts_with('-') => {
                eprintln!("Unknown flag: {}", s);
                process::exit(1);
            }
            s => {
                if file_path.is_some() {
                    eprintln!("Error: multiple input files are not supported");
                    process::exit(1);
                }
                file_path = Some(s);
            }
        }
    }

    let file_path = match file_path {
        Some(p) => p,
        None => {
            eprintln!("Error: no input file provided");
            process::exit(1);
        }
    };

    let source = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("Error: could not read '{}': {}", file_path, err);
            process::exit(1);
        }
    };

    let mut scanner = Scanner::new(&source);
    let tokens = match scanner.scan_tokens() {
        Ok(tokens) => tokens,
        Err(err) => {
            print_error(file_path, &err);
            process::exit(1);
        }
    };

    if mode == "tokens" {
        for token in &tokens {
            println!(
                "{:>4}:{:<4} {:<20} {:?}",
                token.span.line, token.span.column, token.kind, token.lexeme
            );
        }
        return;
    }

    let module = if mode == "tokens" || mode == "ast" {
        let mut parser = Parser::new(tokens);
        match parser.parse() {
            Ok(m) => m,
            Err(err) => {
                print_error(file_path, &err);
                process::exit(1);
            }
        }
    } else {
        match loader::load_module(Path::new(file_path)) {
            Ok(m) => m,
            Err(err) => {
                print_error(file_path, &err);
                process::exit(1);
            }
        }
    };

    if mode == "ast" {
        println!("{:#?}", module);
        return;
    }

    let errors = checker::check(&module);
    if !errors.is_empty() {
        for err in &errors {
            print_error(file_path, err);
        }
        eprintln!("\n{} error(s) found.", errors.len());
        process::exit(1);
    }

    if mode == "check" {
        println!("All checks passed.");
        return;
    }

    let generated = codegen::generate_with_meta(&module);

    if mode == "emit-rust" || mode == "default" {
        println!("{}", generated.source);
        return;
    }

    if mode == "compile" {
        let out_path = file_path.replace(".ow", "");
        if generated.is_async {
            compile_with_cargo(&out_path, &generated.source);
        } else {
            compile_with_rustc(&out_path, &generated.source);
        }
    }
}

fn compile_with_rustc(out_path: &str, source: &str) {
    let rs_path = format!("{}.rs", out_path);
    if let Err(err) = fs::write(&rs_path, source) {
        eprintln!("Error writing {}: {}", rs_path, err);
        process::exit(1);
    }
    let status = std::process::Command::new("rustc")
        .arg(&rs_path)
        .arg("-o")
        .arg(out_path)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("Compiled to: {}", out_path);
            let _ = fs::remove_file(&rs_path);
        }
        Ok(s) => {
            eprintln!("rustc failed with: {}", s);
            process::exit(1);
        }
        Err(err) => {
            eprintln!("Failed to run rustc: {}", err);
            process::exit(1);
        }
    }
}

fn compile_with_cargo(out_path: &str, source: &str) {
    let project_name = Path::new(out_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("oneway_build");
    let build_dir = format!("{}.cargo", out_path);
    let src_dir = format!("{}/src", build_dir);
    if let Err(err) = fs::create_dir_all(&src_dir) {
        eprintln!("Error creating {}: {}", src_dir, err);
        process::exit(1);
    }
    let cargo_toml = format!(
        "[package]\nname = \"{}\"\nedition = \"2021\"\nversion = \"0.0.0\"\n\n\
         [dependencies]\ntokio = {{ version = \"1\", features = [\"full\"] }}\n",
        sanitize_crate_name(project_name)
    );
    if let Err(err) = fs::write(format!("{}/Cargo.toml", build_dir), cargo_toml) {
        eprintln!("Error writing Cargo.toml: {}", err);
        process::exit(1);
    }
    if let Err(err) = fs::write(format!("{}/main.rs", src_dir), source) {
        eprintln!("Error writing main.rs: {}", err);
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
                eprintln!("Error copying binary: {}", err);
                process::exit(1);
            }
            println!("Compiled to: {}", out_path);
        }
        Ok(s) => {
            eprintln!("cargo build failed with: {}", s);
            process::exit(1);
        }
        Err(err) => {
            eprintln!("Failed to run cargo: {}", err);
            process::exit(1);
        }
    }
}

fn sanitize_crate_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        format!("_{}", s)
    } else if s.is_empty() {
        "oneway_build".to_string()
    } else {
        s
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
