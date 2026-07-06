#!/usr/bin/env node
// Build the in-browser runner for the docs' click-to-run snippets.
//
// Any fenced code block in docs/src tagged ```canon,run=<name> is a
// complete, runnable Canon program. This script extracts each one,
// compiles it with the canon compiler, transpiles the resulting WASI P3
// component to JavaScript with jco, patches around a known jco emission
// bug, and emits everything (plus a manifest consumed by
// docs/assets/docs-enhance.js) into the built site. The markdown is the
// single source of truth: a snippet that stops compiling fails the docs
// build.
//
// Node with no dependencies - Node is already required for jco itself.
//
// Usage:
//   node docs/runner/build.mjs --canon target/release/canon \
//     --src docs/src --out docs/build/runner [--jco "npx jco"]
//
// Constraints on runnable snippets (enforced here): each must be a
// self-contained program that only imports wasi:cli stdout/stderr and
// canon:builtins/json - the interfaces the browser shims in
// docs/runner/shims/ cover. Anything else fails the build.

import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The WASI P3 rc version the canon compiler currently emits. Must match
// src/codegen/wasm/component.rs; jco pins supported versions the same way.
const WASI_VERSION = "0.3.0-rc-2026-03-15";

const FENCE_RE = /^```canon,run=([a-z0-9][a-z0-9-]*)[ \t]*\n([\s\S]*?)^```[ \t]*$/gm;
const TITLE_RE = /^#\s+(.+)$/m;
const IMPORT_RE = /(?:^|\n)\s*import\b[^'"]*['"]([^'"]+)['"]/g;

// jco 1.24.x emits code that references FutureReadableEnd/FutureWritableEnd
// without ever defining them (it defines the Stream* analogues). The guest
// only ever drops these futures, so a minimal stand-in is enough.
const POLYFILL_ANCHOR = "    class InternalFuture{";
const POLYFILL = `
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

`;

function fail(msg) {
  console.error(`runner/build.mjs: error: ${msg}`);
  process.exit(1);
}

function run(cmd, args, opts = {}) {
  try {
    execFileSync(cmd, args, { stdio: ["ignore", "pipe", "pipe"], ...opts });
  } catch (e) {
    fail(
      `command failed: ${cmd} ${args.join(" ")}\n--- stdout ---\n` +
        `${e.stdout || ""}\n--- stderr ---\n${e.stderr || ""}`
    );
  }
}

function parseArgs() {
  const args = { src: "docs/src", jco: "jco" };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i].replace(/^--/, "");
    if (!["canon", "src", "out", "jco"].includes(key) || argv[i + 1] === undefined)
      fail(`bad argument: ${argv[i]}`);
    args[key] = argv[i + 1];
  }
  if (!args.canon || !args.out) fail("--canon and --out are required");
  return args;
}

// Returns [{name, source, page, title}] in document order.
function extractExamples(srcDir) {
  const examples = [];
  const seen = new Map();
  const mds = readdirSync(srcDir, { recursive: true, encoding: "utf8" })
    .filter((p) => p.endsWith(".md"))
    .sort();
  for (const rel of mds) {
    const text = readFileSync(join(srcDir, rel), "utf8");
    const titleM = TITLE_RE.exec(text);
    const title = titleM ? titleM[1].trim() : rel.replace(/\.md$/, "");
    const page = rel.replace(/\\/g, "/").replace(/\.md$/, ".html");
    for (const m of text.matchAll(FENCE_RE)) {
      const [, name, source] = m;
      if (seen.has(name))
        fail(`duplicate run=${name} in ${rel} (first seen in ${seen.get(name)})`);
      seen.set(name, rel);
      examples.push({ name, source, page, title });
    }
  }
  return examples;
}

function patchPolyfill(jsPath) {
  const src = readFileSync(jsPath, "utf8");
  if (src.includes("class FutureReadableEnd extends _FutureEndPolyfill")) return;
  if (!src.includes(POLYFILL_ANCHOR))
    fail(
      `${jsPath}: jco output changed shape - polyfill anchor not found. ` +
        "If the jco version was bumped, check whether the " +
        "FutureReadableEnd/FutureWritableEnd emission bug still exists " +
        "(this polyfill may be deletable)."
    );
  writeFileSync(jsPath, src.replace(POLYFILL_ANCHOR, POLYFILL + POLYFILL_ANCHOR));
}

// Every import in the emitted JS must be relative (our shims).
function checkImports(name, outDir) {
  for (const file of readdirSync(outDir).filter((f) => f.endsWith(".js"))) {
    const text = readFileSync(join(outDir, file), "utf8");
    for (const m of text.matchAll(IMPORT_RE)) {
      const spec = m[1];
      if (!spec.startsWith("./") && !spec.startsWith("../"))
        fail(
          `example '${name}' needs import '${spec}', which has no browser ` +
            "shim. Runnable snippets may only use stdout printing and " +
            "JSON - see docs/runner/build.mjs."
        );
    }
  }
}

function main() {
  const args = parseArgs();
  const canon = resolve(args.canon); // canon build runs from a temp cwd
  const jco = args.jco.split(" ");
  const shimsSrc = join(dirname(fileURLToPath(import.meta.url)), "shims");

  const examples = extractExamples(args.src);
  if (examples.length === 0)
    fail(`no \`\`\`canon,run=<name> blocks found under ${args.src}`);

  rmSync(args.out, { recursive: true, force: true });
  mkdirSync(join(args.out, "shims"), { recursive: true });
  cpSync(shimsSrc, join(args.out, "shims"), { recursive: true });

  for (const ex of examples) {
    const tmp = mkdtempSync(join(tmpdir(), `canon-runner-${ex.name}-`));
    try {
      writeFileSync(join(tmp, `${ex.name}.can`), ex.source);
      run(canon, ["build", `${ex.name}.can`], { cwd: tmp });
      const wasm = join(tmp, "build", ex.name, `${ex.name}.wasm`);
      if (!existsSync(wasm)) fail(`example '${ex.name}': expected ${wasm} after canon build`);

      const outDir = join(args.out, ex.name);
      run(jco[0], [
        ...jco.slice(1),
        "transpile",
        wasm,
        "-o",
        outDir,
        "--no-wasi-shim",
        "--no-typescript",
        "-M",
        `wasi:cli/*@${WASI_VERSION}=../shims/stdout.js#*`,
        "-M",
        "canon:builtins/json@0.1.0=../shims/json.js#json",
      ]);
      patchPolyfill(join(outDir, `${ex.name}.js`));
      checkImports(ex.name, outDir);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
    console.log(`  built ${ex.name} (${ex.page})`);
  }

  writeFileSync(
    join(args.out, "manifest.json"),
    JSON.stringify({ wasiVersion: WASI_VERSION, examples }, null, 1)
  );
  console.log(`runner: ${examples.length} runnable snippets -> ${args.out}`);
}

main();
