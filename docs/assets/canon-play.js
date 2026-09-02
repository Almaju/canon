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
//   highlight(source)           -> Canon source as coloured HTML
//   mount(el, seed)             -> hydrate an editor pane
//
// Loaded as a plain classic script, so it touches only globals.

(function () {
  "use strict";

  // ── Canon syntax highlighter ──────────────────────────────────────
  // A tiny standalone tokenizer (no highlight.js). Almost every name in
  // Canon is PascalCase, so painting all of them one colour would make a
  // wall; instead colour falls on what carries a program's shape -
  // constructors (`Name(`), calls (`name(`), definitions (`name =`), the
  // core vocabulary, literals, operators, strings, numbers - and bare
  // PascalCase stays plain, mirroring the language's own rule.
  //
  // It lives here rather than in docs-enhance.js because the editor
  // needs it too: one tokenizer paints the read-only code blocks and the
  // live buffer, so a snippet cannot look different from the same text
  // typed into the playground.
  var KW = new Set(["extern", "impl", "bindings", "use"]);
  var LIT = new Set(["True", "False", "None", "Some", "Ok", "Err", "Pass", "Fail"]);
  var TYPE = new Set([
    "Bool", "Byte", "Bytes", "Float", "Future", "Handle", "Hex", "Html", "Int",
    "Json", "List", "Map", "Markdown", "Never", "Option", "Ord", "Result",
    "Set", "Stream", "String", "TestResult", "Unit",
  ]);

  function esc(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  var TOKEN =
    /("(?:\\.|[^"\\])*")|(\b0x[0-9a-fA-F_]+\b|\b\d+\.\d+\b|\b\d+\b)|([A-Za-z_][A-Za-z0-9_]*)|(->|=>|::<|[?^*+=|.])|(\s+)|([\s\S])/g;

  function highlight(code) {
    var out = "";
    var m;
    TOKEN.lastIndex = 0;
    while ((m = TOKEN.exec(code))) {
      if (m[1]) {
        out += '<span class="tk-str">' + esc(m[1]) + "</span>";
      } else if (m[2]) {
        out += '<span class="tk-num">' + esc(m[2]) + "</span>";
      } else if (m[3]) {
        var id = m[3];
        var rest = code.slice(TOKEN.lastIndex);
        var parens = /^\s*\(/.test(rest);
        var assign = /^\s*=(?![=>])/.test(rest);
        var cls = null;
        if (KW.has(id)) cls = "tk-kw";
        else if (LIT.has(id)) cls = "tk-lit";
        else if (TYPE.has(id)) cls = "tk-type";
        else if (/^[A-Z]/.test(id) && parens) cls = "tk-ctor";
        else if (/^[a-z]/.test(id) && parens) cls = "tk-call";
        else if (/^[a-z]/.test(id) && assign) cls = "tk-def";
        out += cls ? '<span class="' + cls + '">' + esc(id) + "</span>" : esc(id);
      } else if (m[4]) {
        out += '<span class="tk-op">' + esc(m[4]) + "</span>";
      } else {
        out += esc(m[0]);
      }
    }
    return out;
  }

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
  // Everything a Canon CLI component imports at the core level, under
  // the names `wit-component` gives them: an interface's functions
  // under `<iface>@<version>`, the stream builtins a function's
  // streams need beside it (`[stream-new-0]write-via-stream`), and the
  // task intrinsics under `$root`. The waitable set is the async
  // canonical ABI; nothing here ever suspends, because a program that
  // genuinely blocks is reaching for a WASI interface the browser has
  // none of — and that lands on the stub below with its name in the
  // message.
  var VERSION = "@0.3.0-rc-2026-03-15";

  function hostImports(program, sink, state) {
    var dec = new TextDecoder();
    var memory = function () { return program.instance.exports.memory; };

    // Hand a string back through a canonical-ABI return area: the bytes
    // go in guest memory via its own allocator, and the (ptr, len) pair
    // goes where the caller asked for it.
    function returnString(text, retptr) {
      var bytes = new TextEncoder().encode(text);
      var ptr = program.instance.exports.cabi_realloc(0, 0, 1, bytes.length);
      new Uint8Array(memory().buffer, ptr, bytes.length).set(bytes);
      var view = new DataView(memory().buffer);
      view.setInt32(retptr, ptr, true);
      view.setInt32(retptr + 4, bytes.length, true);
    }

    var imports = {};
    imports["wasi:cli/stdout" + VERSION] = {
      "write-via-stream": function () { return 1; },
      "[stream-new-0]write-via-stream": function () { return (2n << 32n) | 1n; },
      "[stream-write-0]write-via-stream": function (writer, ptr, len) {
        sink(dec.decode(new Uint8Array(memory().buffer, Number(ptr), Number(len))), false);
        return 0;
      },
      "[stream-drop-writable-0]write-via-stream": function () { return 0; },
      "[future-drop-readable-1]write-via-stream": function () { return 0; },
    };
    // The browser has no stdin: a read ends the stream at once
    // (`DROPPED`, no bytes), so `Stdin()` is the empty string.
    imports["wasi:cli/stdin" + VERSION] = {
      "read-via-stream": function (retptr) {
        var view = new DataView(memory().buffer);
        view.setInt32(retptr, 1, true);
        view.setInt32(retptr + 4, 1, true);
      },
      "[stream-read-0]read-via-stream": function () { return 1; },
      "[stream-drop-readable-0]read-via-stream": function () { return 0; },
      "[future-drop-readable-1]read-via-stream": function () { return 0; },
    };
    // A playground program has no command line and no working
    // directory; both lower to an empty list.
    imports["wasi:cli/environment" + VERSION] = {
      "get-arguments": function () { return 0; },
      "get-initial-cwd": function () { return 0; },
    };
    imports["wasi:cli/exit" + VERSION] = {
      "exit-with-code": function (code) { state.exitCode = Number(code); },
    };
    imports["$root"] = {
      "[waitable-set-new]": function () { return 1; },
      "[waitable-join]": function () { return 0; },
      "[waitable-set-wait]": function () { return 0; },
      "[waitable-set-drop]": function () { return 0; },
      "[subtask-drop]": function () { return 0; },
      "[subtask-cancel]": function () { return 0; },
    };
    imports["[export]wasi:cli/run" + VERSION] = {
      "[task-return]run": function () { return 0; },
    };
    // The one thing Canon can't express in Canon: shortest-round-trip
    // decimal for an f64 (`host_builtin_json` in src/runtime.rs).
    imports["canon:builtins/json@0.1.0"] = {
      "from-float": function (value, retptr) {
        returnString(jsonFloat(value), retptr);
      },
    };
    return imports;
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
    // The program is the one core module that owns memory; the others
    // are the import shims wit-component puts beside it, which a host
    // answering every import directly has no use for.
    var mods = coreModules(component);
    return Promise.all(mods.map(function (m) { return WebAssembly.compile(m); }))
      .then(function (compiled) {
        var program = compiled.filter(function (m) {
          return WebAssembly.Module.exports(m).some(function (ex) { return ex.kind === "memory"; });
        });
        if (program.length !== 1)
          throw new Error("unexpected component shape: " + compiled.length + " core modules");
        return program[0];
      })
      .then(function (module) {
      var state = { exitCode: null };
      var program = { instance: null };
      var known = hostImports(program, sink, state);
      var imports = {};
      WebAssembly.Module.imports(module).forEach(function (im) {
        imports[im.module] = imports[im.module] || {};
        var have = known[im.module] && known[im.module][im.name];
        imports[im.module][im.name] = have || function () {
          throw new Error(im.module + "." + im.name + " is not available in the browser");
        };
      });
      program.instance = new WebAssembly.Instance(module, imports);
      program.instance.exports["[async-lift-stackful]wasi:cli/run" + VERSION + "#run"]();
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

  // `seed` is a tour step's program: it owns the editor instead of the
  // shared draft, so walking the tour never overwrites what the reader
  // left on the playground page (and `reset` goes back to the step, not
  // to hello-world).
  function mount(el, seed) {
    if (el.dataset.mounted) return;
    el.dataset.mounted = "1";

    var editor = el.querySelector(".pg-editor");
    var out = el.querySelector(".pg-out code");
    var status = el.querySelector(".pg-status");
    var runBtn = el.querySelector(".pg-run");
    var shareBtn = el.querySelector(".pg-share");
    var resetBtn = el.querySelector(".pg-reset");
    var start = seed || HELLO;

    editor.value = seed || sourceFromUrl() || stored() || HELLO;

    // A textarea cannot colour its own text, so the colour lives in a
    // <pre> pinned underneath it, painted from the same buffer: the
    // textarea keeps its caret, selection, and undo stack, and renders
    // its text transparent over the top. The two boxes share every
    // metric that decides where a glyph lands (font, padding, tab size,
    // wrapping), so they cannot drift.
    var paint = function () {
      if (!layer) return;
      layer.innerHTML = highlight(editor.value);
      layer.parentNode.scrollTop = editor.scrollTop;
      layer.parentNode.scrollLeft = editor.scrollLeft;
    };
    var layer = el.querySelector(".pg-hl code");
    paint();

    editor.addEventListener("input", function () {
      paint();
      if (seed) return;
      try { localStorage.setItem(STORAGE_KEY, editor.value); } catch (e) {}
    });
    editor.addEventListener("scroll", paint);

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
          paint();
          if (!seed) {
            try { localStorage.setItem(STORAGE_KEY, result.canonical); } catch (e) {}
          }
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
      editor.value = start;
      paint();
      if (!seed) {
        try { localStorage.setItem(STORAGE_KEY, start); } catch (e) {}
      }
      out.textContent = "";
      status.textContent = "";
    });
  }

  globalThis.canonPlay = {
    compileAndRun: compileAndRun,
    highlight: highlight,
    mount: mount,
  };
})();
