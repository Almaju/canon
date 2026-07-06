# Canon Language — Contributor Commands
#
# These are development helpers for working on the compiler itself.
# End users install the `canon` binary and invoke it directly.
#
# See README.md § Building from Source for details.

set quiet := true

default:
    @just --list

# Build a debug binary
build:
    cargo build

# Build and install the release binary to ~/.cargo/bin/canon
install:
    cargo install --path . --force

# Run the full test suite.
#
# `cargo test` runs every integration harness:
#   * tests/checker_fixtures.rs — checker ok/fail fixtures (golden .stderr)
#   * tests/runtime_fixtures.rs — full-pipeline programs (golden .stdout)
#   * tests/canon_tests.rs     — Canon-language tests (TestResult)
#   * tests/checker_api.rs      — Rust tests of compiler internals
#   * library unit tests        — anything inside src/
#
# Every layer fails the build on regression, so this single command is

# the canonical CI gate.
test:
    cargo test

# Regenerate every golden file (`.stderr` for checker/fail and `.stdout`
# for runtime/) from the current compiler's actual output. Review the
# resulting `git diff` before committing — that's the review surface
# for "did this output change in a sensible way?". Mirrors

# `TRYBUILD=overwrite` from Rust's trybuild crate.
update-fixtures:
    CANON_UPDATE_FIXTURES=1 cargo test --tests

# Lightweight convenience runner for Canon-language tests with pretty
# per-file output. The same tests run under `cargo test` via the
# `tests/canon_tests.rs` harness — use that for CI, use this for

# faster local iteration on a single test file.
test-can: build
    #!/usr/bin/env sh
    set -e
    pass=0; fail=0; files=0
    for f in tests/canon/*_test.can; do
        [ -f "$f" ] || continue
        files=$((files + 1))
        printf "\n=== %s ===\n" "$f"
        if cargo run --quiet -- test "$f"; then
            pass=$((pass + 1))
        else
            fail=$((fail + 1))
        fi
    done
    echo ""
    echo "${files} test file(s): ${pass} clean, ${fail} with failures"
    [ "$fail" -eq 0 ]

# Examples that don't compile yet because they're waiting on a tracked
# stdlib/codegen gap (see "Known codegen gaps" in CLAUDE.md), NOT a
# regression. `just examples` uses this list to tell the two apart: a
# checker failure for an example that IS listed here is a quiet, expected
# "known gap" skip; a checker failure for any example NOT listed here is
# reported loudly (and fails the run under CI) because it's a likely
# regression. Space-separated list of example directory names; keep it
# empty when every example compiles.
known_gaps := ""

# Run every example program under examples/ and report pass / fail / skip.
#
# Examples live in examples/ for documentation; they are not the test
# suite (`cargo test` is). This task is a smoke check that examples
# still compile and run end-to-end — useful when changing the compiler
# or stdlib, but not gated by CI.
#
# `examples/` is a workspace whose members are individual packages under
# `examples/<name>/`. Each member is built and run with a 5-second timeout
# so long-running examples (servers, fetch loops) don't block the smoke
# check.
#
# A checker failure is split two ways so a real regression can't hide
# behind "waiting on a stdlib gap":
#   * example listed in `known_gaps` → "skip — known gap" (expected)
#   * any other example             → "checker failed" (loud; under CI,
#                                       i.e. $CI set, the run exits 1)
# An example listed in `known_gaps` that now compiles is a stale entry
# and is flagged at the end so the list gets pruned.
examples: build
    #!/usr/bin/env sh
    known_gaps="{{ known_gaps }}"
    is_known_gap() {
        for g in $known_gaps; do
            [ "$g" = "$1" ] && return 0
        done
        return 1
    }
    pass=0; fail=0; skip=0; broken=0; stale=""
    for d in examples/*/; do
        [ -f "$d/canon.toml" ] || continue
        label=$(basename "$d")
        printf "%-20s" "$label"
        # Split checker failures: a tracked gap is an expected skip; any
        # other failure is loud (a likely regression).
        if ! cargo run --quiet -- check "$d" >/dev/null 2>&1; then
            if is_known_gap "$label"; then
                echo "·  (skip — known gap)"
                skip=$((skip + 1))
            else
                echo "✗  (checker failed — not a known gap)"
                broken=$((broken + 1))
            fi
            continue
        fi
        # Compiles but listed as a gap → the list is stale; still run it.
        if is_known_gap "$label"; then
            stale="$stale $label"
        fi
        # Run via the embedded WASM runtime with a 5s timeout
        tmpout=$(mktemp)
        cargo run --quiet -- run "$d" > "$tmpout" 2>&1 &
        pid=$!
        i=0; timed_out=0
        while [ $i -lt 5 ] && kill -0 "$pid" 2>/dev/null; do
            sleep 1; i=$((i + 1))
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
            timed_out=1
        else
            wait "$pid"; rc=$?
        fi
        output=$(cat "$tmpout"); rm -f "$tmpout"
        if [ "$timed_out" -eq 1 ]; then
            echo "·  (skip — long-running)"
            skip=$((skip + 1))
        elif [ "$rc" -eq 0 ]; then
            echo "✓  $output"
            pass=$((pass + 1))
        else
            echo "✗  (runtime error)"
            fail=$((fail + 1))
        fi
    done
    echo ""
    echo "${pass} passed, ${fail} failed, ${skip} skipped, ${broken} checker failure(s)"
    if [ -n "$stale" ]; then
        echo "note: known_gaps entries now compile —$stale — prune them from the justfile." >&2
    fi
    if [ "$broken" -gt 0 ]; then
        echo "error: checker failure(s) above are not on the known_gaps list — likely a regression." >&2
        if [ -n "$CI" ]; then
            exit 1
        fi
    fi

# Run a single example by name (e.g. `just example clock`)
example name:
    #!/usr/bin/env sh
    set -e
    if [ ! -f "examples/{{ name }}/canon.toml" ]; then
        echo "No example package at examples/{{ name }}/ (need an canon.toml there)" >&2
        exit 1
    fi
    exec cargo run --quiet -- run "examples/{{ name }}"

# Regenerate the embedded WASI bindings from the vendored WIT files
# under wit-vendor/. Run after upgrading the WASI version or after
# changing the bindgen emitter. Commit the resulting
# packages/canon/std/bindgen/ tree.
regen-bindings: build
    cargo run --quiet -- install packages/canon/std

# Format compiler source
fmt:
    cargo fmt

# Lint compiler source
clippy:
    cargo clippy -- -W warnings

# Run all CI checks locally (mirrors ci.yml)
ci:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test

# Clean build artifacts and compiled examples
clean:
    #!/usr/bin/env sh
    cargo clean
    find examples packages -type d -name 'build' -prune -exec rm -rf {} +

# Install git hooks (pre-commit)
install-hooks:
    git config core.hooksPath githooks
    @echo "Git hooks installed (using githooks/ directory)"

# Uninstall git hooks
uninstall-hooks:
    git config --unset core.hooksPath
    @echo "Git hooks uninstalled (reverted to default .git/hooks)"

# Package the VS Code extension into a .vsix
build-vscode-extension:
    #!/usr/bin/env sh
    set -e
    cd editors/vscode-canon
    npm install
    npx vsce package --no-dependencies
    echo "Done. Install with: code --install-extension editors/vscode-canon/canon-lang-<version>.vsix"

# Build the Zed extension WASMs (requires Docker for the grammar WASM)
build-extension:
    #!/usr/bin/env sh
    set -e
    echo "Building grammar WASM (requires Docker)..."
    cd editors/tree-sitter-canon && tree-sitter build --wasm
    cp editors/tree-sitter-canon/tree-sitter-canon.wasm editors/zed-canon/grammars/canon.wasm
    echo "Building extension WASM..."
    cd editors/zed-canon && cargo build --release --target wasm32-wasip1
    cp editors/zed-canon/target/wasm32-wasip1/release/canon_zed.wasm editors/zed-canon/extension.wasm
    echo "Done. Commit editors/zed-canon/grammars/canon.wasm and editors/zed-canon/extension.wasm"
