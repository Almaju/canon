// Compile and run Canon in the browser.
//
// The compiler itself is `canon-compiler.wasm` — the crate built for
// `wasm32-unknown-unknown` (`src/playground.rs`), imports: none. Give it
// source, get a WASI P3 component back. That component is then run right
// here: a Canon CLI component is a thin canonical-ABI wrapper around two
// core modules, and a browser instantiates core wasm natively, so the
// whole round trip is `WebAssembly.instantiate` twice and a dozen import
// stubs. No transpiler, no JSPI, no server.
//
// The component's shape is pinned by tests/playground_host_test.rs: one
// module providing memory, one importing it and exporting `run`. If that
// test fails, this file is what it is protecting.
//
// Canonical format is a compiler phase, not a lint, so the page formats
// what you typed before compiling it and hands the canonical text back —
// otherwise every stray space would come back as a checker error.
//
// Exposes `globalThis.canonPlay`:
//
//   compileAndRun(source, sink) -> Promise<{status, exitCode, canonical}>
//   mount(el)                   -> hydrate the playground page
//
// Loaded as a plain classic script, so it touches only globals.

(function () {
  "use strict";

  var COMPILER_URL = "canon-compiler.wasm";
  var TAG_DIAGNOSTICS = 0;

  // ── the compiler ──────────────────────────────────────────────────
  var moduleReady = null;
  function compilerModule() {
    if (!moduleReady) {
      moduleReady = WebAssembly.compileStreaming
        ? WebAssembly.compileStreaming(fetch(COMPILER_URL))
        : fetch(COMPILER_URL)
            .then(function (r) { return r.arrayBuffer(); })
            .then(function (b) { return WebAssembly.compile(b); });
    }
    return moduleReady;
  }

  // A fresh instance per call round: the compiler leaks its input buffer
  // by design (the host owns it) and aborts on panic, so throwing the
  // instance away is both the simplest reset and the only reliable one.
  // Compiling the module is the expensive half and that is cached above.
  function call(inst, fn, source) {
    var ex = inst.exports;
    var bytes = new TextEncoder().encode(source);
    var ptr = ex.canon_alloc(bytes.length);
    new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
    // A panic aborts rather than returning, but the hook has already
    // left its message in the result buffer and memory survives the
    // trap — so the result is read the same way either way.
    try { ex[fn](ptr, bytes.length); } catch (e) {}
    var out = new Uint8Array(ex.memory.buffer, ex.canon_result_ptr(), ex.canon_result_len());
    if (out.length === 0)
      return { ok: false, diagnostics: "the compiler crashed on this program" };
    if (out[0] === TAG_DIAGNOSTICS)
      return { ok: false, diagnostics: new TextDecoder().decode(out.subarray(1)) };
    return { ok: true, payload: out.slice(1) };
  }

  function formatAndCompile(source) {
    return compilerModule()
      .then(function (mod) { return WebAssembly.instantiate(mod, {}); })
      .then(function (inst) {
        var formatted = call(inst, "canon_format", source);
        if (!formatted.ok) return formatted;
        var canonical = new TextDecoder().decode(formatted.payload);
        var compiled = call(inst, "canon_compile", canonical);
        compiled.canonical = canonical;
        return compiled;
      });
  }

  // ── unwrapping the component ──────────────────────────────────────
  // Walk the component's sections and collect every nested core module
  // (component section id 1 carries a complete core wasm binary).
  function coreModules(bytes) {
    var mods = [];
    var i = 8; // magic + version
    while (i < bytes.length) {
      var id = bytes[i++];
      var size = 0;
      var shift = 0;
      for (;;) {
        var b = bytes[i++];
        size |= (b & 0x7f) << shift;
        if ((b & 0x80) === 0) break;
        shift += 7;
      }
      if (id === 1) mods.push(bytes.subarray(i, i + size));
      i += size;
    }
    return mods;
  }

  // ── the host ──────────────────────────────────────────────────────
  // Everything a Canon CLI component imports at the core level. The
  // waitable set is the async canonical ABI; nothing here ever suspends,
  // because a program that genuinely blocks is reaching for a WASI
  // interface the browser has none of — and that lands on the stub below
  // with its name in the message.
  function hostImports(provider, sink, state) {
    var memory = provider.exports.memory;
    var dec = new TextDecoder();

    // Hand a string back through a canonical-ABI return area: the bytes
    // go in guest memory via its own allocator, and the (ptr, len) pair
    // goes where the caller asked for it.
    function returnString(text, retptr) {
      var bytes = new TextEncoder().encode(text);
      var ptr = provider.exports.cabi_realloc(0, 0, 1, bytes.length);
      new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
      var view = new DataView(memory.buffer);
      view.setInt32(retptr, ptr, true);
      view.setInt32(retptr + 4, bytes.length, true);
    }

    function stream(isErr) {
      return {
        "write-via-stream": function () { return 1; },
        "stream-new": function () { return (2n << 32n) | 1n; },
        "stream-write": function (writer, ptr, len) {
          sink(dec.decode(new Uint8Array(memory.buffer, Number(ptr), Number(len))), isErr);
          return 0;
        },
        "stream-drop-writable": function () { return 0; },
        "future-drop-readable": function () { return 0; },
      };
    }
    return {
      "wasi:cli/stdout": stream(false),
      "wasi:cli/stderr": stream(true),
      // A playground program has no command line and no working
      // directory; both lower to an empty list.
      "wasi:cli/environment": {
        "get-arguments": function () { return 0; },
        "get-initial-cwd": function () { return 0; },
      },
      "wasi:cli/exit": {
        "exit-with-code": function (code) { state.exitCode = Number(code); },
      },
      "canon:async/waitable": {
        "set-new": function () { return 1; },
        "join": function () { return 0; },
        "set-wait": function () { return 0; },
        "set-drop": function () { return 0; },
        "subtask-drop": function () { return 0; },
        "task-return": function () { return 0; },
        "subtask-cancel": function () { return 0; },
      },
      // The one thing Canon can't express in Canon: shortest-round-trip
      // decimal for an f64 (`host_builtin_json` in src/runtime.rs).
      "canon:builtins/json": {
        "from-float": function (value, retptr) {
          returnString(jsonFloat(value), retptr);
        },
      },
    };
  }

  // Matches the native host byte for byte, which JS does not do on its
  // own: `String(v)` switches to exponent notation outside 1e-6..1e21,
  // where Rust's f64 Display never does, and it renders -0 as "0". So
  // take JS's shortest-round-trip digits and place the point by hand.
  function jsonFloat(value) {
    // JSON has no spelling for these; the native host emits null too.
    if (!isFinite(value)) return "null";
    if (Object.is(value, -0)) return "-0";
    var sign = value < 0 ? "-" : "";
    var parts = Math.abs(value).toExponential().split("e");
    var digits = parts[0].replace(".", "");
    var point = Number(parts[1]) + 1; // digits before the decimal point
    if (point <= 0) return sign + "0." + "0".repeat(-point) + digits;
    if (point >= digits.length) return sign + digits + "0".repeat(point - digits.length);
    return sign + digits.slice(0, point) + "." + digits.slice(point);
  }

  function run(component, sink) {
    var mods = coreModules(component);
    return Promise.all(mods.map(function (m) { return WebAssembly.compile(m); }))
      .then(function (compiled) {
        // Identify the two modules by what they say about themselves
        // rather than by position: the provider owns memory, the program
        // imports it.
        var provider = null;
        var program = null;
        compiled.forEach(function (m) {
          var importsMemory = WebAssembly.Module.imports(m).some(function (im) {
            return im.kind === "memory";
          });
          if (importsMemory) program = m;
          else if (WebAssembly.Module.exports(m).some(function (ex) { return ex.kind === "memory"; }))
            provider = m;
        });
        if (!provider || !program)
          throw new Error("unexpected component shape: " + compiled.length + " core modules");

        var providerInst = new WebAssembly.Instance(provider, {});
        var state = { exitCode: null };
        var known = hostImports(providerInst, sink, state);
        var imports = {};
        WebAssembly.Module.imports(program).forEach(function (im) {
          imports[im.module] = imports[im.module] || {};
          if (im.module === "env") {
            imports.env[im.name] = providerInst.exports[im.name];
            return;
          }
          var have = known[im.module] && known[im.module][im.name];
          imports[im.module][im.name] = have || function () {
            throw new Error(im.module + "." + im.name + " is not available in the browser");
          };
        });

        new WebAssembly.Instance(program, imports).exports.run();
        return state;
      });
  }

  function compileAndRun(source, sink) {
    // Anything before the program even starts — the compiler failing to
    // download, a browser refusing the module — is reported in the same
    // panel rather than left as a rejected promise and a stuck button.
    return formatAndCompile(source).catch(function (err) {
      return { ok: false, diagnostics: "could not load the compiler: " + (err.message || err) };
    }).then(function (result) {
      if (!result.ok) {
        sink(result.diagnostics + "\n", true);
        return { status: "error", exitCode: null, canonical: result.canonical };
      }
      return run(result.payload, sink).then(
        function (state) {
          return { status: "ok", exitCode: state.exitCode, canonical: result.canonical };
        },
        function (err) {
          sink(String(err.message || err) + "\n", true);
          return { status: "trap", exitCode: null, canonical: result.canonical };
        }
      );
    });
  }

  // ── the playground page ───────────────────────────────────────────
  var STORAGE_KEY = "canon:playground";
  var HELLO =
    'Unit => Program {\n' +
    '    "hello, world" -> Print\n' +
    '}\n';

  // A shared link carries the source in a query parameter, not the
  // fragment — the fragment is the docs app's router (`#play`).
  function sourceFromUrl() {
    var code = new URLSearchParams(location.search).get("code");
    if (!code) return null;
    try {
      return new TextDecoder().decode(
        Uint8Array.from(atob(code.replace(/-/g, "+").replace(/_/g, "/")), function (c) {
          return c.charCodeAt(0);
        })
      );
    } catch (e) {
      return null;
    }
  }

  function shareUrl(source) {
    var b64 = btoa(String.fromCharCode.apply(null, new TextEncoder().encode(source)))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
    return location.origin + location.pathname + "?code=" + b64 + "#play";
  }

  function stored() {
    try { return localStorage.getItem(STORAGE_KEY); } catch (e) { return null; }
  }

  function mount(el) {
    if (el.dataset.mounted) return;
    el.dataset.mounted = "1";

    var editor = el.querySelector(".pg-editor");
    var out = el.querySelector(".pg-out code");
    var status = el.querySelector(".pg-status");
    var runBtn = el.querySelector(".pg-run");
    var shareBtn = el.querySelector(".pg-share");
    var resetBtn = el.querySelector(".pg-reset");

    editor.value = sourceFromUrl() || stored() || HELLO;

    editor.addEventListener("input", function () {
      try { localStorage.setItem(STORAGE_KEY, editor.value); } catch (e) {}
    });

    function write(text, isErr) {
      var span = document.createElement("span");
      if (isErr) span.className = "pg-err";
      span.textContent = text;
      out.appendChild(span);
    }

    function go() {
      out.textContent = "";
      status.textContent = "compiling…";
      runBtn.disabled = true;
      var started = performance.now();
      compileAndRun(editor.value, write).then(function (result) {
        runBtn.disabled = false;
        // Canonical format is the language's, not a preference: show the
        // reader what their program actually is.
        if (result.canonical && result.canonical !== editor.value) {
          editor.value = result.canonical;
          try { localStorage.setItem(STORAGE_KEY, result.canonical); } catch (e) {}
        }
        var ms = Math.round(performance.now() - started);
        if (result.status === "ok") {
          if (!out.textContent) out.textContent = "(no output)";
          status.textContent =
            result.exitCode ? "exit " + result.exitCode + " · " + ms + "ms" : ms + "ms";
        } else {
          status.textContent = result.status === "trap" ? "trap" : "error";
        }
      });
    }

    runBtn.addEventListener("click", go);
    // Ctrl/Cmd+Enter runs, the way every other editor on the web does.
    editor.addEventListener("keydown", function (e) {
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); go(); }
    });

    shareBtn.addEventListener("click", function () {
      var url = shareUrl(editor.value);
      history.replaceState(null, "", url);
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(url).then(function () {
          shareBtn.textContent = "copied";
          setTimeout(function () { shareBtn.textContent = "share"; }, 1400);
        });
      }
    });

    resetBtn.addEventListener("click", function () {
      editor.value = HELLO;
      try { localStorage.setItem(STORAGE_KEY, HELLO); } catch (e) {}
      out.textContent = "";
      status.textContent = "";
    });
  }

  globalThis.canonPlay = { compileAndRun: compileAndRun, mount: mount };
})();
