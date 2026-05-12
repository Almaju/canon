use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const CLIPPY_TOML: &str = include_str!("../templates/clippy.toml");
const RUSTFMT_TOML: &str = include_str!("../templates/rustfmt.toml");

const CLIPPY_DENY: &[&str] = &[
    "clippy::expect_used",
    "clippy::manual_filter_map",
    "clippy::manual_map",
    "clippy::manual_unwrap_or",
    "clippy::needless_return",
    "clippy::panic",
    "clippy::single_match",
    "clippy::todo",
    "clippy::too_many_arguments",
    "clippy::unimplemented",
    "clippy::uninlined_format_args",
    "clippy::unreachable",
    "clippy::unwrap_used",
    "clippy::wildcard_imports",
];

const DYLINT_GIT: &str = "https://github.com/Almaju/oneway";
const DYLINT_PATTERN: &str = "oneway-lints";
const DYLINT_LIB: &str = "oneway_lints";

fn user_args() -> Vec<String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("oneway") {
        args.remove(0);
    }
    args
}

fn write_config_dir() -> io::Result<PathBuf> {
    let dir = env::temp_dir().join(format!("cargo-oneway-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("clippy.toml"), CLIPPY_TOML)?;
    fs::write(dir.join("rustfmt.toml"), RUSTFMT_TOML)?;
    Ok(dir)
}

fn announce(cmd: &Command) {
    let program = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    eprintln!("$ {} {}", program, args.join(" "));
}

fn run(mut cmd: Command) -> io::Result<i32> {
    announce(&cmd);
    Ok(cmd.status()?.code().unwrap_or(1))
}

fn run_fmt(passthrough: &[String], check: bool) -> io::Result<i32> {
    let dir = write_config_dir()?;
    let mut cmd = Command::new("cargo");
    cmd.arg("fmt");
    cmd.args(passthrough);
    if check {
        cmd.arg("--check");
    }
    cmd.arg("--")
        .arg("--config-path")
        .arg(dir.join("rustfmt.toml"));
    run(cmd)
}

fn run_clippy(passthrough: &[String]) -> io::Result<i32> {
    let dir = write_config_dir()?;
    let mut cmd = Command::new("cargo");
    cmd.arg("clippy");
    cmd.args(passthrough);
    cmd.arg("--");
    for lint in CLIPPY_DENY {
        cmd.arg("-D").arg(lint);
    }
    cmd.env("CLIPPY_CONF_DIR", &dir);
    run(cmd)
}

fn run_dylint(_passthrough: &[String]) -> io::Result<i32> {
    let mut cmd = Command::new("cargo");
    cmd.arg("dylint")
        .arg("--git")
        .arg(DYLINT_GIT)
        .arg("--pattern")
        .arg(DYLINT_PATTERN)
        .arg("--lib")
        .arg(DYLINT_LIB);
    run(cmd)
}

fn run_lint(passthrough: &[String]) -> io::Result<i32> {
    let clippy = run_clippy(passthrough)?;
    let dylint = run_dylint(passthrough)?;
    Ok(if clippy != 0 { clippy } else { dylint })
}

fn run_all() -> io::Result<i32> {
    let fmt = run_fmt(&[], true)?;
    let clippy = run_clippy(&[])?;
    let dylint = run_dylint(&[])?;
    Ok([fmt, clippy, dylint]
        .into_iter()
        .find(|&c| c != 0)
        .unwrap_or(0))
}

fn print_help() {
    eprintln!(
        "cargo-oneway — opinionated lint + format runner

USAGE:
    cargo oneway [SUBCOMMAND] [CARGO_ARGS...]

SUBCOMMANDS:
    fmt     Apply Oneway rustfmt config to the workspace
    lint    Run clippy + oneway-lints with the Oneway lint set
    help    Print this message

With no subcommand, runs `fmt --check`, clippy, and oneway-lints — failing
if any step fails. CARGO_ARGS are forwarded to the underlying cargo command.

PREREQUISITES:
    cargo install cargo-dylint dylint-link
"
    );
}

fn dispatch() -> io::Result<i32> {
    let args = user_args();
    match args.first().map(String::as_str) {
        Some("fmt") => run_fmt(&args[1..], false),
        Some("lint") => run_lint(&args[1..]),
        Some("help") | Some("-h") | Some("--help") => {
            print_help();
            Ok(0)
        }
        None => run_all(),
        Some(other) => {
            eprintln!("cargo-oneway: unknown subcommand `{other}` — try `cargo oneway help`");
            Ok(2)
        }
    }
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code.clamp(1, 255) as u8),
        Err(e) => {
            eprintln!("cargo-oneway: {e}");
            ExitCode::FAILURE
        }
    }
}
