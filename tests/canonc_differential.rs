//! Differential testing of the self-hosted compiler against the reference.
//!
//! Every program under `tests/canonc/` is written in the subset `canonc`
//! compiles and ends in a nullary `Unit => Answer` declaration, with
//! `Answer = Int` or `Answer = String`. Each is compiled twice: `canonc`
//! emits a core module whose `answer` export evaluates the declaration,
//! and the reference compiler runs the same source with a `Unit =>
//! Program { Answer() -> Print }` entry appended. The two answers must
//! agree — the fixpoint (`canonc_self_hosts.rs`) proves `canonc` is
//! self-consistent, and this proves it computes what Canon means.
//!
//! Drop a `.can` file in the directory to extend the corpus.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn canon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_canon"))
}

fn scratch(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("canonc-differential");
    fs::create_dir_all(&path).expect("create tmpdir");
    path.push(name);
    path
}

/// What `canonc` computes for the program's `answer`, rendered the way
/// `Print` renders it — or why it could not.
fn canonc_answer(path: &Path, is_string: bool) -> Result<String, String> {
    let out = canon()
        .args(["run", "canonc", path.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run canonc");
    if !out.status.success() {
        return Err(format!(
            "canon run canonc failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let hex = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let hex = hex.trim();
    if !hex.starts_with("0061736d") {
        return Err(format!("canonc rejected it: {hex}"));
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"))
        .collect();
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, &bytes)
        .map_err(|e| format!("canonc's module does not validate: {e}"))?;
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .map_err(|e| format!("canonc's module does not instantiate: {e}"))?;
    if is_string {
        let (ptr, len) = instance
            .get_typed_func::<(), (i32, i32)>(&mut store, "answer")
            .map_err(|e| format!("no string `answer` export: {e}"))?
            .call(&mut store, ())
            .map_err(|e| format!("`answer` trapped: {e}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("canonc's module exports no memory")?;
        let mut out = vec![0u8; len as usize];
        memory
            .read(&store, ptr as usize, &mut out)
            .map_err(|e| format!("string out of bounds: {e}"))?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    } else {
        instance
            .get_typed_func::<(), i32>(&mut store, "answer")
            .map_err(|e| format!("no `answer` export: {e}"))?
            .call(&mut store, ())
            .map(|v| v.to_string())
            .map_err(|e| format!("`answer` trapped: {e}"))
    }
}

/// What the reference compiler prints for the same source with a
/// `Program` entry that prints the answer.
fn reference_answer(name: &str, source: &str) -> Result<String, String> {
    let path = scratch(name);
    fs::write(
        &path,
        format!("{source}\nUnit => Program {{\n    Answer() -> Print\n}}\n"),
    )
    .expect("write source");
    let out = canon()
        .args(["run", path.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run");
    if !out.status.success() {
        return Err(format!(
            "the reference compiler failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8(out.stdout)
        .expect("utf-8 stdout")
        .trim_end_matches('\n')
        .to_string())
}

#[test]
fn canonc_agrees_with_the_reference_compiler() {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.push("tests");
    dir.push("canonc");
    let mut programs: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("tests/canonc exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("can"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty(), "the corpus has programs");

    let mut disagreements = Vec::new();
    for path in &programs {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = fs::read_to_string(path).expect("read program");
        let is_string = source.contains("Answer = String");
        match (
            canonc_answer(path, is_string),
            reference_answer(&name, &source),
        ) {
            (Ok(ours), Ok(theirs)) if ours == theirs => {}
            (Ok(ours), Ok(theirs)) => disagreements.push(format!(
                "{name}: canonc says {ours:?}, the reference says {theirs:?}"
            )),
            (Err(e), _) | (_, Err(e)) => disagreements.push(format!("{name}: {e}")),
        }
    }
    assert!(
        disagreements.is_empty(),
        "{}/{} program(s) disagree:\n{}",
        disagreements.len(),
        programs.len(),
        disagreements.join("\n")
    );
}
