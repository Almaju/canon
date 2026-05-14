# Oneway Language — Development Commands

set quiet

default:
    @just --list

# Build the compiler
build:
    cargo build

# Build in release mode
release:
    cargo build --release

# Run all tests
test:
    cargo test

# Run tests with output
test-verbose:
    cargo test -- --nocapture

# Run an .ow file (compile + execute)
run file:
    #!/usr/bin/env sh
    set -e
    cargo build --quiet 2>/dev/null
    base="$(echo '{{file}}' | sed 's/\.ow$//')"
    rs="${base}.rs"
    cargo run --quiet -- --compile '{{file}}' 2>/dev/null
    "./${base}"
    rm -rf "${rs}" "${base}" "${base}.cargo"

# Run an example by name (e.g. `just example hello`, `just example multifile`)
example name:
    #!/usr/bin/env sh
    set -e
    for path in "examples/{{name}}.ow" "examples/{{name}}/main.ow"; do
        if [ -f "$path" ]; then
            exec just run "$path"
        fi
    done
    echo "No example found at examples/{{name}}.ow or examples/{{name}}/main.ow" >&2
    exit 1

# Emit generated Rust code for an .ow file
emit file:
    cargo run --quiet -- --emit-rust {{file}} 2>/dev/null

# Check sort order of an .ow file
check file:
    cargo run --quiet -- --check {{file}} 2>/dev/null

# Show tokens for an .ow file
tokens file:
    cargo run --quiet -- --tokens {{file}} 2>/dev/null

# Show AST for an .ow file
ast file:
    cargo run --quiet -- --ast {{file}} 2>/dev/null

# Compile an .ow file to binary (no run)
compile file:
    cargo run --quiet -- --compile {{file}} 2>/dev/null

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
        if cargo run --quiet -- --compile "$f" 2>/dev/null; then
            output=$("./${base}" 2>&1) && {
                echo "✓  $output"
                pass=$((pass + 1))
            } || {
                echo "✗  (runtime error)"
                fail=$((fail + 1))
            }
            rm -rf "${base}" "${base}.rs" "${base}.cargo"
        else
            echo "·  (skip — does not compile yet)"
            skip=$((skip + 1))
            rm -rf "${base}.rs" "${base}.cargo"
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
        cargo run --quiet -- --emit-rust "$f" 2>/dev/null || echo "(failed to emit)"
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
        if cargo run --quiet -- --check "$f" 2>/dev/null; then
            echo "✓"
        else
            echo "✗"
        fi
    done

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
