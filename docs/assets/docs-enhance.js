// Progressive enhancement for the Canon docs app.
//
// The docs are a Canon web app: `view` returns an HTML string that the
// host (canon-web.js) swaps into the page on every render, then calls
// `canonAfterRender(root)` if it exists. This script defines that hook to
// do two things the pure-Canon renderer leaves to the browser:
//
//   1. Syntax-highlight `<pre data-info="canon...">` code blocks. The
//      Markdown renderer emits the fence info string as `data-info`, so
//      "```canon" and "```canon,run" both arrive tagged.
//   2. Put a Run button on "```canon,run" blocks, and mount the
//      playground page. Both compile and run the source in the page
//      through canon-play.js - the block's own text is the program, so
//      what the reader runs is exactly what the reader sees.
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
    return /(?:^|,)run(?:,|$)/.test(info || "");
  }

  // ── click-to-run ──────────────────────────────────────────────────
  function execute(source, outEl, statusEl) {
    outEl.textContent = "";
    statusEl.textContent = "compiling...";
    return canonPlay
      .compileAndRun(source, function (text, isErr) {
        var span = document.createElement("span");
        if (isErr) span.className = "canon-runner-err";
        span.textContent = text;
        outEl.appendChild(span);
      })
      .then(function (result) {
        if (result.status === "ok" && !outEl.textContent) outEl.textContent = "(no output)";
        statusEl.textContent = result.status;
      });
  }

  function addRunButton(pre) {
    var bar = ensureBar(pre);
    var btn = document.createElement("button");
    btn.className = "canon-run-button";
    btn.title = "Run this program in your browser";
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
      execute(pre.querySelector("code").textContent, out, status);
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
    var scope = root || document;
    var pres = scope.querySelectorAll("pre[data-info]");
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
      if (code && infoRun(info) && !pre.dataset.run) {
        pre.dataset.run = "1";
        addRunButton(pre);
      }
    });
    var pg = scope.querySelector(".pg");
    if (pg) canonPlay.mount(pg);
  }

  globalThis.canonAfterRender = enhance;
  if (document.readyState !== "loading") enhance(document);
  else document.addEventListener("DOMContentLoaded", function () { enhance(document); });
})();
