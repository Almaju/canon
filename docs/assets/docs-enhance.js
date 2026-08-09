// Progressive enhancement for the Canon docs app.
//
// The docs are a Canon web app: `view` returns an HTML string that the
// host (canon-web.js) swaps into the page on every render, then calls
// `canonAfterRender(root)` if it exists. This script defines that hook to
// do two things the pure-Canon renderer leaves to the browser:
//
//   1. Syntax-highlight `<pre data-info="canon...">` code blocks with
//      canon-play.js's tokenizer — the same one painting the editor. The
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

  // ── the tour ──────────────────────────────────────────────────────
  // A tour step is prose plus exactly one Canon block, and that block is
  // the step's program — it belongs in the pane on the right, not in the
  // middle of the prose. Hoisting it here rather than in the Canon view
  // keeps a step a plain Markdown file with a plain fenced block, so
  // `tests/docs_snippets.rs` still compile-checks all sixteen of them.
  function hoistTourProgram(scope) {
    var doc = scope.querySelector(".tour-doc");
    var pane = scope.querySelector(".tour-pane .pg");
    if (!doc || !pane) return;
    // The step's program is its Canon block. A ` ```text ` block is
    // illustration — annotated, or deliberately not a whole program —
    // and stays where the prose put it.
    var pre = null;
    Array.prototype.some.call(doc.querySelectorAll("pre[data-info]"), function (el) {
      if (!infoLang(el.getAttribute("data-info") || "")) return false;
      pre = el;
      return true;
    });
    var code = pre && pre.querySelector("code");
    if (!code) return;
    pre.parentNode.removeChild(pre);
    canonPlay.mount(pane, code.textContent);
    // Two steps reach for a file and for the network, and a browser tab
    // hosts neither. Say so, rather than shipping a button whose only
    // possible answer is a missing import.
    if (!infoRun(pre.getAttribute("data-info") || "")) markHostOnly(pane);
  }

  function markHostOnly(pane) {
    var bar = pane.querySelector(".pg-bar");
    var run = bar.querySelector(".pg-run");
    var hint = bar.querySelector(".pg-hint");
    if (run) run.remove();
    if (hint) hint.remove();
    var note = document.createElement("div");
    note.className = "pg-host";
    note.textContent = "needs a real host — run this one with `canon run`";
    pane.insertBefore(note, pane.querySelector(".pg-edit"));
  }

  // ── the hook ──────────────────────────────────────────────────────
  function enhance(root) {
    var scope = root || document;
    hoistTourProgram(scope);
    var pres = scope.querySelectorAll("pre[data-info]");
    Array.prototype.forEach.call(pres, function (pre) {
      var info = pre.getAttribute("data-info") || "";
      var code = pre.querySelector("code");
      if (code && infoLang(info) && !code.dataset.hl) {
        code.innerHTML = canonPlay.highlight(code.textContent);
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
