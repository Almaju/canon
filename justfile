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

# Run the test suite
test:
    cargo test

# Run all examples and report pass / fail / skip
examples: build
    #!/usr/bin/env sh
    pass=0; fail=0; skip=0
    for f in examples/*.ow examples/*/main.ow; do
        [ -f "$f" ] || continue
        dir=$(dirname "$f")
        if [ "$(basename "$f")" = "main.ow" ]; then
            label=$(basename "$(dirname "$f")")
            stem="main"
        else
            stem=$(basename "$f" .ow)
            label="$stem"
        fi
        binpath="$dir/.oneway/$stem/$stem"
        printf "%-20s" "$label"
        if cargo run --quiet -- build "$f" >/dev/null 2>&1; then
            tmpout=$(mktemp)
            "$binpath" > "$tmpout" 2>&1 &
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
        else
            echo "·  (skip — does not compile yet)"
            skip=$((skip + 1))
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
