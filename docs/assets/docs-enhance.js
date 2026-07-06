// Progressive enhancement for the Canon docs app.
//
// The docs are a Canon web app: `view` returns an HTML string that the
// host (canon-web.js) swaps into the page on every render, then calls
// `canonAfterRender(root)` if it exists. This script defines that hook to
// do two things the pure-Canon renderer leaves to the browser:
//
//   1. Syntax-highlight `<pre data-info="canon...">` code blocks. The
//      Markdown renderer emits the fence info string as `data-info`, so
//      "```canon" and "```canon,run=hello" both arrive tagged.
//   2. Put a Run button on "```canon,run=<name>" blocks. Each such block
//      was compiled to a WASI P3 component and transpiled to JS by
//      docs/runner/build.mjs at docs-build time; the button imports it and
//      streams the program's stdout into a panel. Canon components are
//      async-lifted, so this needs JSPI (WebAssembly.Suspending) - stable
//      in Chromium; without it the button explains instead of running.
//
// Loaded as a plain classic script (injected after canon-web.js), so it
// touches only globals - no bundler, no modules on the page itself.

(function () {
  "use strict";

  // ── Canon syntax highlighter ──────────────────────────────────────
  // A tiny standalone tokenizer (no highlight.js). Almost every name in
  // Canon is PascalCase, so painting all of them one colour would make a
  // wall; instead colour falls on what carries a program's shape -
  // constructors (`Name(`), calls (`name(`), definitions (`name =`), the
  // core vocabulary, literals, operators, strings, numbers - and bare
  // PascalCase stays plain, mirroring the language's own rule.
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

  function infoLang(info) {
    return /(?:^|,)(canon|ow)(?:,|$)/.test(info || "");
  }
  function infoRun(info) {
    var m = /(?:^|,)run=([a-z0-9-]+)(?:,|$)/.exec(info || "");
    return m ? m[1] : null;
  }

  // ── click-to-run ──────────────────────────────────────────────────
  var hasJspi =
    typeof WebAssembly !== "undefined" &&
    typeof WebAssembly.Suspending === "function";
  var JSPI_MSG =
    "Live examples need JSPI (WebAssembly.Suspending), which this browser " +
    "does not ship yet. Try Chrome or Edge.";

  var manifestReady = null; // Promise<Set<string>> of runnable names
  function runnable() {
    if (!manifestReady) {
      manifestReady = fetch("runner/manifest.json")
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (m) {
          var s = new Set();
          if (m && m.examples) m.examples.forEach(function (e) { s.add(e.name); });
          return s;
        })
        .catch(function () { return new Set(); });
    }
    return manifestReady;
  }

  var runCounter = 0;
  var queue = Promise.resolve(); // runs are serialised (shared stdout sink)

  function execute(name, outEl, statusEl) {
    outEl.textContent = "";
    if (!hasJspi) {
      outEl.innerHTML = '<span class="canon-runner-err">' + esc(JSPI_MSG) + "</span>";
      return Promise.resolve();
    }
    statusEl.textContent = "running...";
    var url = new URL(
      "runner/" + name + "/" + name + ".js?i=" + runCounter++,
      document.baseURI
    ).href;
    var produced = false;
    globalThis.__canonSink = function (line, isErr) {
      produced = true;
      var span = document.createElement("span");
      if (isErr) span.className = "canon-runner-err";
      span.textContent = line + "\n";
      outEl.appendChild(span);
    };
    return import(url)
      .then(function (mod) { return mod.run.run(); })
      .then(function () { return new Promise(function (r) { setTimeout(r, 200); }); })
      .then(function () {
        if (!produced) outEl.textContent = "(no output)";
        statusEl.textContent = "ok";
      })
      .catch(function (e) {
        var span = document.createElement("span");
        span.className = "canon-runner-err";
        span.textContent = String(e) + "\n";
        outEl.appendChild(span);
        statusEl.textContent = "trap";
      });
  }

  function addRunButton(pre, name) {
    var bar = ensureBar(pre);
    var btn = document.createElement("button");
    btn.className = "canon-run-button";
    btn.title = hasJspi ? "Run this program in your browser" : JSPI_MSG;
    btn.innerHTML = '<span class="canon-run-glyph">&#9654;</span> run';
    bar.appendChild(btn);

    var panel = null;
    btn.addEventListener("click", function () {
      if (!panel) {
        panel = document.createElement("div");
        panel.className = "canon-runner";
        panel.innerHTML =
          '<div class="canon-runner-bar"><span class="dot"></span>' +
          '<span>output</span><span class="canon-runner-status"></span></div>' +
          '<pre class="canon-runner-out"><code></code></pre>';
        pre.classList.add("canon-has-runner");
        pre.parentNode.insertBefore(panel, pre.nextSibling);
      }
      var out = panel.querySelector(".canon-runner-out code");
      var status = panel.querySelector(".canon-runner-status");
      queue = queue.then(function () { return execute(name, out, status); });
    });
  }

  // ── click-to-copy ─────────────────────────────────────────────────
  // Every code block gets a copy button. The run button (if any) already
  // lives in `.canon-run-bar`; we drop the copy button in beside it so
  // the two never overlap, otherwise we make a bar of our own.
  // The button bar is shared: copy is added first (synchronously), the run
  // button (added later, after the manifest fetch) joins the same bar.
  function ensureBar(pre) {
    var bar = pre.querySelector(".canon-run-bar");
    if (!bar) {
      bar = document.createElement("div");
      bar.className = "canon-run-bar";
      pre.appendChild(bar);
    }
    return bar;
  }

  function addCopyButton(pre) {
    var code = pre.querySelector("code");
    if (!code) return;
    var bar = ensureBar(pre);
    var btn = document.createElement("button");
    btn.className = "canon-copy-button";
    btn.type = "button";
    btn.title = "Copy to clipboard";
    btn.textContent = "copy";
    bar.appendChild(btn);

    var reset = null;
    btn.addEventListener("click", function () {
      var text = code.textContent;
      var done = function (ok) {
        btn.textContent = ok ? "copied" : "failed";
        btn.classList.toggle("canon-copy-ok", ok);
        if (reset) clearTimeout(reset);
        reset = setTimeout(function () {
          btn.textContent = "copy";
          btn.classList.remove("canon-copy-ok");
        }, 1400);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(function () { done(true); },
          function () { done(false); });
      } else {
        done(false);
      }
    });
  }

  // ── the hook ──────────────────────────────────────────────────────
  function enhance(root) {
    var pres = (root || document).querySelectorAll("pre[data-info]");
    var wantsRun = false;
    Array.prototype.forEach.call(pres, function (pre) {
      var info = pre.getAttribute("data-info") || "";
      var code = pre.querySelector("code");
      if (code && infoLang(info) && !code.dataset.hl) {
        code.innerHTML = highlight(code.textContent);
        code.dataset.hl = "1";
        pre.classList.add("canon-code");
      }
      if (code && !pre.dataset.copy) {
        pre.dataset.copy = "1";
        addCopyButton(pre);
      }
      if (infoRun(info)) wantsRun = true;
    });
    if (!wantsRun) return;
    runnable().then(function (names) {
      Array.prototype.forEach.call(pres, function (pre) {
        var name = infoRun(pre.getAttribute("data-info") || "");
        if (name && names.has(name) && !pre.dataset.run) {
          pre.dataset.run = name;
          addRunButton(pre, name);
        }
      });
    });
  }

  globalThis.canonAfterRender = enhance;
  if (document.readyState !== "loading") enhance(document);
  else document.addEventListener("DOMContentLoaded", function () { enhance(document); });
})();
