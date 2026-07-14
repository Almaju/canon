//! `canon bindgen` — generate Canon source from a WIT package or a
//! WebAssembly Component.
//!
//! Entry points:
//!
//!   - [`generate_from_path`] — high-level: takes a `.wit` file or directory,
//!     or a `.wasm` Component, parses it, and returns the set of files to
//!     write.
//!   - [`run`] — CLI glue: parses args, writes files to disk, prints what
//!     was emitted. Used by `main.rs`.

mod emit;
pub mod naming;

use std::fs;
use std::path::{Path, PathBuf};

use wit_parser::{Resolve, UnresolvedPackageGroup};

pub use emit::EmittedFile;
pub use naming::camel_to_kebab;

/// Top-level error type for the bindgen pipeline. Stays a plain `String`
/// for now — there's no callers that need structured matching yet, and
/// the existing `CanonError` is span-bound which doesn't apply here.
#[derive(Debug)]
pub struct BindgenError(pub String);

impl std::fmt::Display for BindgenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BindgenError {}

impl From<String> for BindgenError {
    fn from(s: String) -> Self {
        BindgenError(s)
    }
}

impl From<&str> for BindgenError {
    fn from(s: &str) -> Self {
        BindgenError(s.to_string())
    }
}

/// Parse the given path and produce a list of files to write.
///
/// The path is dispatched by extension:
///
///   - `.wit`  — parsed as a single-file WIT package via `wit_parser`.
///   - `.wasm` — decoded as a Component; the embedded `component-type`
///     custom section is parsed via `wit_component::decode`.
///   - directory — pushed as a multi-file WIT package.
pub fn generate_from_path(input: &Path) -> Result<Vec<EmittedFile>, BindgenError> {
    let resolve = parse_input(input)?;
    Ok(emit::emit_all(&resolve))
}

/// Decode wasm bytes carrying a WIT package (the form package
/// registries serve) or a full Component, and produce the files to
/// write. Same decode as the `.wasm` arm of [`generate_from_path`],
/// minus the filesystem — used by the registry-backed `canon install`.
pub fn generate_from_wasm_bytes(bytes: &[u8]) -> Result<Vec<EmittedFile>, BindgenError> {
    let decoded = wit_component::decode(bytes)
        .map_err(|e| BindgenError(format!("failed to decode component: {e}")))?;
    match decoded {
        wit_component::DecodedWasm::Component(resolve, _)
        | wit_component::DecodedWasm::WitPackage(resolve, _) => Ok(emit::emit_all(&resolve)),
    }
}

fn parse_input(input: &Path) -> Result<Resolve, BindgenError> {
    if input.is_dir() {
        return parse_wit_directory(input);
    }

    let ext = input
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "wit" => {
            let mut resolve = Resolve::default();
            resolve
                .push_file(input)
                .map_err(|e| BindgenError(format!("failed to parse `{}`: {e}", input.display())))?;
            Ok(resolve)
        }
        "wasm" => {
            let bytes = fs::read(input)
                .map_err(|e| BindgenError(format!("could not read `{}`: {e}", input.display())))?;
            let decoded = wit_component::decode(&bytes)
                .map_err(|e| BindgenError(format!("failed to decode component: {e}")))?;
            // Both `Component` and `WitPackage` variants carry a `Resolve`
            // we can walk; we don't need to distinguish them for emission.
            match decoded {
                wit_component::DecodedWasm::Component(resolve, _) => Ok(resolve),
                wit_component::DecodedWasm::WitPackage(resolve, _) => Ok(resolve),
            }
        }
        other => Err(BindgenError(format!(
            "unsupported input extension `.{}` (expected `.wit` or `.wasm`)",
            other
        ))),
    }
}

/// Load every `.wit` file from a flat directory into a single `Resolve`.
///
/// `Resolve::push_dir` expects a single-package layout with a nested
/// `deps/` directory — not what we have. The stdlib's vendored
/// `wit/wasi/` is a flat
/// collection of independent packages (`cli.wit`, `clocks.wit`, …) that
/// cross-reference each other through `import wasi:<pkg>/<iface>`
/// declarations in their world definitions.
///
/// `UnresolvedPackageGroup::parse_dir` parses every `*.wit` file in the
/// directory into a single unresolved group without touching the
/// `Resolve`, and `push_group` then resolves them as a batch (with
/// topological sort + cycle detection). That gives us a one-shot load
/// with no partial-state retries.
fn parse_wit_directory(dir: &Path) -> Result<Resolve, BindgenError> {
    // Collect every `.wit` file under the directory into its own
    // `UnresolvedPackageGroup`. `parse_dir` would fail here because it
    // assumes the directory holds one package; we hold many (cli, clocks,
    // …) in a flat layout.
    let mut wit_files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| BindgenError(format!("could not read `{}`: {e}", dir.display())))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("wit"))
        .collect();
    if wit_files.is_empty() {
        return Err(BindgenError(format!(
            "no `.wit` files found in `{}`",
            dir.display()
        )));
    }
    wit_files.sort();

    let mut groups = Vec::with_capacity(wit_files.len());
    for path in &wit_files {
        let contents = fs::read_to_string(path)
            .map_err(|e| BindgenError(format!("could not read `{}`: {e}", path.display())))?;
        let group = UnresolvedPackageGroup::parse(path, &contents)
            .map_err(|e| BindgenError(format!("failed to parse `{}`: {e}", path.display())))?;
        groups.push(group);
    }

    // `push_groups` topologically sorts internally, so the choice of which
    // group plays the "main" role is purely formal — every package gets
    // loaded either way. We arbitrarily promote the last one (alphabetical
    // sort puts `sockets.wit` last for WASI, but the order doesn't matter).
    // Opt into `@unstable` items (e.g. `wasi:cli/exit#exit-with-code`).
    // Without `all_features`, the resolver silently drops everything
    // marked `@unstable(feature = …)`, which today includes the most
    // useful exit form. We're shipping a 0.3 RC stdlib anyway; gating on
    // stability again would be needless ceremony.
    let mut resolve = Resolve {
        all_features: true,
        ..Resolve::default()
    };
    let main = groups
        .pop()
        .expect("non-empty after the wit_files.is_empty() check");
    resolve.push_groups(main, groups).map_err(|e| {
        BindgenError(format!(
            "failed to resolve WIT directory `{}`: {e}",
            dir.display()
        ))
    })?;
    Ok(resolve)
}

/// Outcome of a successful `canon bindgen` invocation.
pub struct RunOutcome {
    /// Paths that were written.
    pub written: Vec<PathBuf>,
    /// Items the generator skipped because their shape isn't yet
    /// representable in Canon. Surfaced to stderr by the CLI.
    pub skipped: Vec<String>,
}

/// CLI entry point. Reads `<input>` and writes files to `<out_dir>` (or
/// the current directory if `out_dir` is `None`).
pub fn run(input: &Path, out_dir: Option<&Path>) -> Result<RunOutcome, BindgenError> {
    let emitted = generate_from_path(input)?;
    let base: PathBuf = out_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for file in emitted {
        // Skip interfaces that produced no real Canon output. Two cases
        // count as "no output":
        //   * Truly empty content (everything was filtered).
        //   * Content that is *only* `use` lines — those get accumulated
        //     from `use types.{…}` annotations on the WIT interface, but
        //     if every type/fn that would have referenced them was
        //     filtered (resources, list returns, …), the file ends up
        //     just importing a sibling and declaring nothing of its own.
        //     Writing such a file would produce a dead import that the
        //     checker would (rightly) complain about.
        if has_no_decls(&file.content) {
            skipped.extend(file.skipped);
            continue;
        }
        let path = base.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                BindgenError(format!("could not create `{}`: {e}", parent.display()))
            })?;
        }
        // Run the emitted Canon source through the same formatter the
        // compiler enforces on every build. Keeps regenerated bindings
        // canonical even when the emitter's whitespace heuristics drift.
        // If the emitted source somehow fails to parse, fall back to the
        // raw content rather than refusing to write — the user will see
        // the failure on the next `canon check`.
        let content =
            crate::formatter::format(&file.content).unwrap_or_else(|_| file.content.clone());
        fs::write(&path, content.as_bytes())
            .map_err(|e| BindgenError(format!("could not write `{}`: {e}", path.display())))?;
        written.push(path);
        skipped.extend(file.skipped);
    }
    Ok(RunOutcome { written, skipped })
}

/// True when the given file contents have no top-level declarations —
/// only blank lines and `use` directives. See the caller's comment for
/// why those files are treated as empty.
fn has_no_decls(content: &str) -> bool {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .all(|l| l.starts_with("use "))
}
