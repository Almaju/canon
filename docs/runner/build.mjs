#!/usr/bin/env node
// Build the in-browser runner for the Canon book's runnable examples.
//
// Any fenced code block in docs/src tagged ```canon,run=<name> is a
// complete, runnable Canon program. This script extracts each one,
// compiles it with the canon compiler, transpiles the resulting WASI P3
// component to JavaScript with jco, patches around a known jco emission
// bug, and emits everything (plus a manifest consumed by
// docs/theme/run.js) into the built site. The markdown is the single
// source of truth: a snippet that stops compiling fails the docs build.
//
// Node with no dependencies - Node is already required for jco itself.
//
// Usage:
//   node docs/runner/build.mjs --canon target/release/canon \
//     --src docs/src --out docs/book/runner [--jco "npx jco"]
//
// Constraints on runnable snippets (enforced here):
//   - must be a self-contained program (its own `main`),
//   - may only import wasi:cli stdout/stderr and canon:builtins/json -
//     the two interfaces the browser shims in docs/runner/shims/ cover.
//     Anything else (clocks, random, fs, sockets, http) fails the build
//     with a pointer to this file.

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

// Web-target example apps (the Elm `init`/`update`/`view` triple) get a
// live, interactive preview instead of a stdout panel. Each entry names
// a package under the repo's `examples/` tree; it is built with `canon
// build`, and its `<stem>.wasm` + compiler-emitted `canon-web.js` are
// copied under `<out>/web/<name>/` with a themed `index.html`. A book
// page embeds that directory in an <iframe>. Unlike the CLI runner, a
// web app needs no jco transpile: browsers instantiate the core module
// directly, and `canon-web.js` is the host. See docs/src/examples/todolist.md.
const WEB_APPS = [{ name: "todolist", src: "examples/todolist-web" }];

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

// jco 1.24.6 emits code that references FutureReadableEnd/FutureWritableEnd
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
            "shim. Runnable book snippets may only use stdout printing " +
            "and JSON - see docs/runner/build.mjs."
        );
    }
  }
}

// Depth-first search for a file by exact name under `dir`.
function findFile(dir, filename) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const hit = findFile(p, filename);
      if (hit) return hit;
    } else if (entry.name === filename) {
      return p;
    }
  }
  return null;
}

// The iframe shell for a web-app preview. Self-contained (its own
// theme, not the book's) and keyed to its own localStorage namespace so
// the demo persists across reloads without touching the book's storage.
function webIndexHtml(name) {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${name}</title>
<style>
  :root { color-scheme: light dark; }
  body {
    margin: 0; padding: 1rem 1.25rem;
    font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
    color: #1a1a2e; background: #fff;
  }
  h1 { font-size: 1.3rem; margin: 0 0 .75rem; }
  form { display: flex; gap: .5rem; margin-bottom: .75rem; }
  input {
    flex: 1; padding: .5rem .6rem; font: inherit;
    border: 1px solid #ccc; border-radius: 6px; background: #fff; color: inherit;
  }
  ul { list-style: none; margin: .5rem 0; padding: 0; }
  li {
    display: flex; align-items: center; gap: .4rem;
    padding: .4rem 0; border-bottom: 1px solid #eee;
  }
  s { color: #999; }
  button {
    font: inherit; padding: .3rem .6rem; cursor: pointer;
    border: 1px solid #cfcfe0; border-radius: 6px;
    background: #f3f3fb; color: inherit;
  }
  button:hover { background: #e8e8f5; }
  li button { padding: .15rem .5rem; font-size: .85em; }
  @media (prefers-color-scheme: dark) {
    body { color: #e6e6f0; background: #1b1b26; }
    input { background: #12121a; border-color: #3a3a4a; }
    li { border-color: #2c2c3a; }
    button { background: #2a2a3a; border-color: #3a3a4a; }
    button:hover { background: #33334a; }
  }
</style>
</head>
<body>
<div id="app"></div>
<script src="canon-web.js"></script>
<script>canonWebStart("${name}.wasm", document.getElementById("app"), "canon-docs:${name}");</script>
</body>
</html>
`;
}

// Build each web-app example and stage its bundle under <out>/web/<name>/.
function buildWebApps(canon, outRoot) {
  const built = [];
  for (const app of WEB_APPS) {
    const srcDir = resolve(REPO_ROOT, app.src);
    if (!existsSync(srcDir)) fail(`web app '${app.name}': ${srcDir} not found`);
    const tmp = mkdtempSync(join(tmpdir(), `canon-web-${app.name}-`));
    try {
      const pkg = join(tmp, "pkg");
      cpSync(srcDir, pkg, { recursive: true });
      run(canon, ["build", pkg], { cwd: tmp });
      // The stem comes from canon.toml's `name`, which may differ from
      // the preview name; take whichever .wasm the build produced.
      const wasm = findAnyWasm(tmp);
      const hostJs = findFile(tmp, "canon-web.js");
      if (!wasm || !hostJs)
        fail(`web app '${app.name}': build produced no .wasm/canon-web.js under ${tmp}`);

      const outDir = join(outRoot, "web", app.name);
      mkdirSync(outDir, { recursive: true });
      cpSync(wasm, join(outDir, `${app.name}.wasm`));
      cpSync(hostJs, join(outDir, "canon-web.js"));
      writeFileSync(join(outDir, "index.html"), webIndexHtml(app.name));
      built.push(app.name);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
    console.log(`  built web app ${app.name}`);
  }
  return built;
}

function findAnyWasm(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const hit = findAnyWasm(p);
      if (hit) return hit;
    } else if (entry.name.endsWith(".wasm")) {
      return p;
    }
  }
  return null;
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

  const webApps = buildWebApps(canon, args.out);

  writeFileSync(
    join(args.out, "manifest.json"),
    JSON.stringify({ wasiVersion: WASI_VERSION, examples, webApps }, null, 1)
  );
  console.log(
    `runner: ${examples.length} runnable examples, ` +
      `${webApps.length} web app(s) → ${args.out}`
  );
}

main();
