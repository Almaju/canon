# Oneway Language — Contributor Commands
#
# These are development helpers for working on the compiler itself.
# End users install the `oneway` binary and invoke it directly.
#
# See README.md § Building from Source for details.

set quiet := true

default:
    @just --list

# Build a debug binary
build:
    cargo build

# Build and install the release binary to ~/.cargo/bin/oneway
install:
    cargo install --path . --force

# Run the full test suite.
#
# `cargo test` runs every integration harness:
#   * tests/checker_fixtures.rs — checker ok/fail fixtures (golden .stderr)
#   * tests/runtime_fixtures.rs — full-pipeline programs (golden .stdout)
#   * tests/oneway_tests.rs     — Oneway-language tests (TestResult)
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
    ONEWAY_UPDATE_FIXTURES=1 cargo test --tests

# Lightweight convenience runner for Oneway-language tests with pretty
# per-file output. The same tests run under `cargo test` via the
# `tests/oneway_tests.rs` harness — use that for CI, use this for
# faster local iteration on a single test file.
test-ow: build
    #!/usr/bin/env sh
    set -e
    pass=0; fail=0; files=0
    for f in tests/oneway/*_test.ow; do
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

# Run every example program under examples/ and report pass / fail / skip.
#
# Examples live in examples/ for documentation; they are not the test
# suite (`cargo test` is). This task is a smoke check that examples
# still compile and run end-to-end — useful when changing the compiler
# or stdlib, but not gated by CI.
examples: build
    #!/usr/bin/env sh
    pass=0; fail=0; skip=0
    for f in examples/*.ow examples/*/main.ow; do
        [ -f "$f" ] || continue
        if [ "$(basename "$f")" = "main.ow" ]; then
            label=$(basename "$(dirname "$f")")
        else
            label=$(basename "$f" .ow)
        fi
        printf "%-20s" "$label"
        # Skip examples that do not pass the checker (stdlib not yet wired)
        if ! cargo run --quiet -- check "$f" >/dev/null 2>&1; then
            echo "·  (skip — does not compile yet)"
            skip=$((skip + 1))
            continue
        fi
        # Run via the embedded WASM runtime with a 5s timeout
        tmpout=$(mktemp)
        cargo run --quiet -- run "$f" > "$tmpout" 2>&1 &
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
    echo "${pass} passed, ${fail} failed, ${skip} skipped"

# Run a single example by name (e.g. `just example hello`)
example name:
    #!/usr/bin/env sh
    set -e
    for path in "examples/{{ name }}.ow" "examples/{{ name }}/main.ow"; do
        if [ -f "$path" ]; then
            exec cargo run --quiet -- run "$path"
        fi
    done
    echo "No example found at examples/{{ name }}.ow or examples/{{ name }}/main.ow" >&2
    exit 1

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
    find examples -type d -name '.oneway' -exec rm -rf {} +

# Install git hooks (pre-commit)
install-hooks:
    git config core.hooksPath githooks
    @echo "Git hooks installed (using githooks/ directory)"

# Uninstall git hooks
uninstall-hooks:
    git config --unset core.hooksPath
    @echo "Git hooks uninstalled (reverted to default .git/hooks)"

# Build the Zed extension WASMs (requires Docker for the grammar WASM)
build-extension:
    #!/usr/bin/env sh
    set -e
    echo "Building grammar WASM (requires Docker)..."
    cd editors/tree-sitter-oneway && tree-sitter build --wasm
    cp editors/tree-sitter-oneway/tree-sitter-oneway.wasm editors/zed-oneway/grammars/oneway.wasm
    echo "Building extension WASM..."
    cd editors/zed-oneway && cargo build --release --target wasm32-wasip1
    cp editors/zed-oneway/target/wasm32-wasip1/release/oneway_zed.wasm editors/zed-oneway/extension.wasm
    echo "Done. Commit editors/zed-oneway/grammars/oneway.wasm and editors/zed-oneway/extension.wasm"
