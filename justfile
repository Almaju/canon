# Oneway Language — Development Commands
#
# These targets wrap `cargo run -- <subcommand>` for contributors working in
# the repo. End users install the `oneway` binary via the install script and
# invoke it directly (e.g. `oneway run hello.ow`).

set quiet

default:
    @just --list

# Build the compiler
build:
    cargo build

# Build in release mode
release:
    cargo build --release

# Run cargo tests
test:
    cargo test

# Run cargo tests with output
test-verbose:
    cargo test -- --nocapture

# Run an .ow file (compile + execute)
run file:
    cargo run --quiet -- run {{file}}

# Run an example by name (e.g. `just example hello`, `just example multifile`)
example name:
    #!/usr/bin/env sh
    set -e
    for path in "examples/{{name}}.ow" "examples/{{name}}/main.ow"; do
        if [ -f "$path" ]; then
            exec cargo run --quiet -- run "$path"
        fi
    done
    echo "No example found at examples/{{name}}.ow or examples/{{name}}/main.ow" >&2
    exit 1

# Emit generated Rust code for an .ow file
emit file:
    cargo run --quiet -- emit {{file}}

# Check sort order of an .ow file
check file:
    cargo run --quiet -- check {{file}}

# Show tokens for an .ow file
tokens file:
    cargo run --quiet -- tokens {{file}}

# Show AST for an .ow file
ast file:
    cargo run --quiet -- ast {{file}}

# Compile an .ow file to binary (no run)
compile file:
    cargo run --quiet -- build {{file}}

# Run all examples (continues on failure)
examples: build
    #!/usr/bin/env sh
    pass=0; fail=0; skip=0
    for f in examples/*.ow examples/*/main.ow; do
        [ -f "$f" ] || continue
        base="${f%.ow}"
        if [ "$(basename "$f")" = "main.ow" ]; then
            label=$(basename "$(dirname "$f")")
        else
            label=$(basename "$f" .ow)
        fi
        printf "%-20s" "$label"
        if cargo run --quiet -- build "$f" >/dev/null 2>&1; then
            tmpout=$(mktemp)
            "./${base}" > "$tmpout" 2>&1 &
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
            rm -f "${base}"
        else
            echo "·  (skip — does not compile yet)"
            skip=$((skip + 1))
        fi
    done
    echo ""
    echo "${pass} passed, ${fail} failed, ${skip} skipped"

# Emit Rust for all examples
emit-all: build
    #!/usr/bin/env sh
    for f in examples/*.ow examples/*/main.ow; do
        [ -f "$f" ] || continue
        if [ "$(basename "$f")" = "main.ow" ]; then
            label=$(basename "$(dirname "$f")")
        else
            label=$(basename "$f" .ow)
        fi
        echo "=== $label ==="
        cargo run --quiet -- emit "$f" 2>/dev/null || echo "(failed to emit)"
        echo ""
    done

# Check all examples for sort order
check-all: build
    #!/usr/bin/env sh
    for f in examples/*.ow examples/*/main.ow; do
        [ -f "$f" ] || continue
        if [ "$(basename "$f")" = "main.ow" ]; then
            label=$(basename "$(dirname "$f")")
        else
            label=$(basename "$f" .ow)
        fi
        printf "%-20s" "$label"
        if cargo run --quiet -- check "$f" >/dev/null 2>&1; then
            echo "✓"
        else
            echo "✗"
        fi
    done

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

# Format an .ow file
fmt-ow file:
    cargo run --quiet -- fmt {{file}}

# Check formatting of an .ow file (no changes)
fmt-check file:
    cargo run --quiet -- fmt --check {{file}}

# Format all examples
fmt-all: build
    #!/usr/bin/env sh
    for f in examples/*.ow examples/*/*.ow; do
        [ -f "$f" ] || continue
        cargo run --quiet -- fmt "$f"
    done

# Check formatting of all examples (no changes)
fmt-check-all: build
    #!/usr/bin/env sh
    all_ok=true
    for f in examples/*.ow examples/*/*.ow; do
        [ -f "$f" ] || continue
        if ! cargo run --quiet -- fmt --check "$f" 2>/dev/null; then
            all_ok=false
        fi
    done
    if [ "$all_ok" = false ]; then
        echo "Some files are not formatted. Run 'just fmt-all' to fix."
        exit 1
    fi
    echo "All files formatted."

# Install the LSP server binary
install-lsp:
    cargo install --path . --bin oneway-lsp --force

# Run all CI checks locally (mirrors ci.yml)
ci:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test

# Format compiler source
fmt:
    cargo fmt

# Lint compiler source
clippy:
    cargo clippy -- -W warnings

# Clean build artifacts + compiled examples
clean:
    #!/usr/bin/env sh
    cargo clean
    find examples -type f \( -name '*.rs' -o -perm -u+x ! -name '*.ow' \) -delete
    find examples -type d -name '.oneway' -exec rm -rf {} +
