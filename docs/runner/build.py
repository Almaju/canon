#!/usr/bin/env python3
"""Build the in-browser runner for the Canon book's runnable examples.

Any fenced code block in docs/src tagged ``` ```canon,run=<name>``` is a
complete, runnable Canon program. This script extracts each one, compiles
it with the canon compiler, transpiles the resulting WASI P3 component to
JavaScript with jco, patches around a known jco emission bug, and emits
everything (plus a manifest consumed by docs/theme/run.js) into the built
site. The markdown is the single source of truth: a snippet that stops
compiling fails the docs build.

Usage:
    python3 docs/runner/build.py --canon target/release/canon \
        --src docs/src --out docs/book/runner [--jco "npx jco"]

Constraints on runnable snippets (enforced here):
  - must be a self-contained program (its own `main`),
  - may only import wasi:cli stdout/stderr and canon:builtins/json —
    the two interfaces the browser shims in docs/runner/shims/ cover.
    Anything else (clocks, random, fs, sockets, http) fails the build
    with a pointer to this file.
"""

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# The WASI P3 rc version the canon compiler currently emits. Must match
# src/codegen/wasm/component.rs; jco pins supported versions the same way.
WASI_VERSION = "0.3.0-rc-2026-03-15"

FENCE_RE = re.compile(
    r"^```canon,run=([a-z0-9][a-z0-9-]*)[ \t]*\n(.*?)^```[ \t]*$",
    re.M | re.S,
)
TITLE_RE = re.compile(r"^#\s+(.+)$", re.M)

# jco 1.24.6 emits code that references FutureReadableEnd/FutureWritableEnd
# without ever defining them (it defines the Stream* analogues). The guest
# only ever drops these futures, so a minimal stand-in is enough.
POLYFILL_ANCHOR = "    class InternalFuture{"
POLYFILL = """
    class _FutureEndPolyfill {
      constructor(args) { this._args = args || {}; this._done = false; this._pendingEvent = null; }
      futureTableIdx() { return this._args.tableIdx; }
      setWaitableIdx(i) { this._waitableIdx = i; }
      waitableIdx() { return this._waitableIdx; }
      setHandle(h) { this._handle = h; }
      handle() { return this._handle; }
      setTarget(t) { this._target = t; }
      setGlobalFutureMapRep(r) { this._globalRep = r; }
      setHostInjectFn(fn) { this._hostInjectFn = fn; }
      getElemMeta() { return this._args.elemMeta; }
      isDoneState() { return this._done; }
      hasPendingEvent() { return this._pendingEvent !== null; }
      getPendingEvent() { const e = this._pendingEvent; this._pendingEvent = null; return e; }
      async hostWrite(_o) { this._done = true; }
      drop() { this._done = true; if (this._args.setDroppedFn) this._args.setDroppedFn(); }
    }
    class FutureReadableEnd extends _FutureEndPolyfill {}
    class FutureWritableEnd extends _FutureEndPolyfill {}

"""

IMPORT_RE = re.compile(r"""(?:^|\n)\s*import\b[^'"]*['"]([^'"]+)['"]""")


def fail(msg: str) -> None:
    print(f"runner/build.py: error: {msg}", file=sys.stderr)
    sys.exit(1)


def run(cmd, **kw):
    proc = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if proc.returncode != 0:
        fail(
            f"command failed: {' '.join(map(str, cmd))}\n"
            f"--- stdout ---\n{proc.stdout}\n--- stderr ---\n{proc.stderr}"
        )
    return proc


def extract_examples(src_dir: Path):
    """Return [(name, source, page_html, page_title)] in document order."""
    examples = []
    seen = {}
    for md in sorted(src_dir.rglob("*.md")):
        text = md.read_text()
        title_m = TITLE_RE.search(text)
        title = title_m.group(1).strip() if title_m else md.stem
        rel_html = md.relative_to(src_dir).with_suffix(".html").as_posix()
        for m in FENCE_RE.finditer(text):
            name, source = m.group(1), m.group(2)
            if name in seen:
                fail(f"duplicate run={name} in {md} (first seen in {seen[name]})")
            seen[name] = md
            examples.append((name, source, rel_html, title))
    return examples


def patch_polyfill(js_path: Path) -> None:
    src = js_path.read_text()
    if "class FutureReadableEnd extends _FutureEndPolyfill" in src:
        return
    if POLYFILL_ANCHOR not in src:
        fail(
            f"{js_path}: jco output changed shape — polyfill anchor "
            f"{POLYFILL_ANCHOR!r} not found. If the jco version was bumped, "
            "check whether the FutureReadableEnd/FutureWritableEnd emission "
            "bug still exists (this polyfill may be deletable)."
        )
    js_path.write_text(src.replace(POLYFILL_ANCHOR, POLYFILL + POLYFILL_ANCHOR, 1))


def check_imports(name: str, out_dir: Path) -> None:
    """Every import in the emitted JS must be relative (our shims)."""
    for js in out_dir.glob("*.js"):
        for spec in IMPORT_RE.findall(js.read_text()):
            if not spec.startswith(("./", "../")):
                fail(
                    f"example '{name}' needs import '{spec}', which has no "
                    "browser shim. Runnable book snippets may only use "
                    "stdout printing and JSON — see docs/runner/build.py."
                )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--canon", required=True, help="path to the canon binary")
    ap.add_argument("--src", default="docs/src", help="book markdown root")
    ap.add_argument("--out", required=True, help="output directory (runner/)")
    ap.add_argument("--jco", default="jco", help="jco command (may be multi-word)")
    args = ap.parse_args()

    canon = Path(args.canon).resolve()
    src_dir = Path(args.src).resolve()
    out_root = Path(args.out).resolve()
    shims_src = Path(__file__).parent / "shims"
    jco = args.jco.split()

    examples = extract_examples(src_dir)
    if not examples:
        fail(f"no ```canon,run=<name> blocks found under {src_dir}")

    if out_root.exists():
        shutil.rmtree(out_root)
    (out_root / "shims").mkdir(parents=True)
    for shim in shims_src.glob("*.js"):
        shutil.copy(shim, out_root / "shims" / shim.name)

    manifest = []
    for name, source, page, title in examples:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            (tmp / f"{name}.can").write_text(source)
            run([canon, "build", f"{name}.can"], cwd=tmp)
            wasm = tmp / "build" / name / f"{name}.wasm"
            if not wasm.exists():
                fail(f"example '{name}': expected {wasm} after canon build")

            out_dir = out_root / name
            run(
                jco
                + [
                    "transpile",
                    str(wasm),
                    "-o",
                    str(out_dir),
                    "--no-wasi-shim",
                    "--no-typescript",
                    "-M",
                    f"wasi:cli/*@{WASI_VERSION}=../shims/stdout.js#*",
                    "-M",
                    "canon:builtins/json@0.1.0=../shims/json.js#json",
                ]
            )
            patch_polyfill(out_dir / f"{name}.js")
            check_imports(name, out_dir)

        manifest.append({"name": name, "source": source, "page": page, "title": title})
        print(f"  built {name} ({page})")

    (out_root / "manifest.json").write_text(
        json.dumps({"wasiVersion": WASI_VERSION, "examples": manifest}, indent=1)
    )
    print(f"runner: {len(manifest)} runnable examples → {out_root}")


if __name__ == "__main__":
    main()
