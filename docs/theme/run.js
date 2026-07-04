// Runnable examples for the Canon book.
//
// docs/runner/build.py compiles every ```canon,run=<name> fence to a
// WebAssembly component at docs-build time and transpiles it to JS with
// jco. This script is the browser side: it fetches runner/manifest.json,
// puts a "run" button on each runnable block, streams the program's
// stdout into an output panel below the block, and drives the
// Playground page (#canon-playground) from the same manifest.
//
// Canon components are WASI P3 (async-lifted), so executing them needs
// JSPI (WebAssembly.Suspending) - stable in Chromium; Firefox/Safari
// pending. Without it the button explains instead of running.

(function () {
    var root = typeof path_to_root !== 'undefined' ? path_to_root : '';
    var manifest = null; // { examples: [{name, source, page, title}] }
    var byName = {};
    var runCounter = 0;
    var queue = Promise.resolve();

    var hasJspi = typeof WebAssembly !== 'undefined' &&
        typeof WebAssembly.Suspending === 'function';

    var JSPI_MSG =
        'Live examples need JSPI (WebAssembly.Suspending), which this ' +
        'browser does not ship yet. Try Chrome or Edge.';

    // mdBook splits the fence info string on commas, so a block tagged
    // ```canon,run=hello has the classes "language-canon" and "run=hello".
    function runnableName(code) {
        if (!/(?:^|\s)language-canon(?:\s|$)/.test(code.className)) return null;
        var m = /(?:^|\s)run=([a-z0-9-]+)(?:\s|$)/.exec(code.className);
        return m ? m[1] : null;
    }

    // ── output panel ────────────────────────────────────────────────

    function buildPanel(afterEl, joined) {
        var panel = document.createElement('div');
        panel.className = 'canon-runner';
        panel.innerHTML =
            '<div class="canon-runner-bar"><span class="dot"></span>' +
            '<span class="canon-runner-label">output</span>' +
            '<span class="canon-runner-status"></span></div>' +
            '<pre class="canon-runner-out"><code></code></pre>';
        afterEl.parentNode.insertBefore(panel, afterEl.nextSibling);
        if (joined) afterEl.classList.add('canon-has-runner');
        return panel;
    }

    function panelParts(panel) {
        return {
            status: panel.querySelector('.canon-runner-status'),
            out: panel.querySelector('.canon-runner-out code')
        };
    }

    function appendLine(out, text, isErr) {
        var span = document.createElement('span');
        if (isErr) span.className = 'canon-runner-err';
        span.textContent = text + '\n';
        out.appendChild(span);
    }

    // ── execution ───────────────────────────────────────────────────

    function execute(name, panel) {
        var p = panelParts(panel);
        p.out.textContent = '';
        if (!hasJspi) {
            appendLine(p.out, JSPI_MSG, true);
            return Promise.resolve();
        }
        p.status.textContent = 'running…';
        // import() in a classic script resolves relative specifiers
        // against this script's URL (theme/run.js), not the page -
        // build an absolute URL against the page instead.
        var url = new URL(
            root + 'runner/' + name + '/' + name + '.js?i=' + runCounter++,
            document.baseURI
        ).href;
        var produced = false;
        // The sink stays installed after the run: the guest drops the
        // stdout write's completion future, so the host-side drain can
        // still be flushing lines when run() resolves. The next run
        // simply replaces the sink (runs are serialized below).
        globalThis.__canonSink = function (line, isErr) {
            produced = true;
            appendLine(p.out, line, isErr);
        };
        function settle() {
            return new Promise(function (res) { setTimeout(res, 200); });
        }
        return import(url)
            .then(function (mod) { return mod.run.run(); })
            .then(settle)
            .then(function () {
                if (!produced) appendLine(p.out, '(program produced no output)');
                p.status.textContent = 'ok';
            })
            .catch(function (e) {
                appendLine(p.out, String(e), true);
                p.status.textContent = 'trap';
            });
    }

    // Runs are serialized: the stdout sink is a global, so two programs
    // running at once would interleave into the wrong panels.
    function enqueue(name, panel) {
        queue = queue.then(function () { return execute(name, panel); });
        return queue;
    }

    function makeButton(label) {
        var btn = document.createElement('button');
        btn.className = 'canon-run-button';
        btn.title = hasJspi ? 'Run this program in your browser' : JSPI_MSG;
        btn.innerHTML = '<span class="canon-run-glyph">&#9654;</span>' +
            (label ? ' ' + label : '');
        return btn;
    }

    // ── inline blocks ───────────────────────────────────────────────

    function wireBlocks() {
        var codes = document.querySelectorAll('pre > code');
        Array.prototype.forEach.call(codes, function (code) {
            var name = runnableName(code);
            if (!name || !byName[name]) return;
            var pre = code.parentNode;
            pre.classList.add('canon-runnable');
            var panel = null;
            var btn = makeButton('run');
            btn.addEventListener('click', function () {
                if (!panel) panel = buildPanel(pre, true);
                enqueue(name, panel);
            });
            var buttons = pre.querySelector('.buttons');
            if (!buttons) {
                buttons = document.createElement('div');
                buttons.className = 'buttons';
                pre.insertBefore(buttons, pre.firstChild);
            }
            buttons.insertBefore(btn, buttons.firstChild);
        });
    }

    // ── playground page ─────────────────────────────────────────────

    function highlight(codeEl, source) {
        codeEl.textContent = source;
        if (typeof hljs !== 'undefined' && hljs.getLanguage &&
            hljs.getLanguage('canon')) {
            try {
                codeEl.innerHTML = hljs.highlight('canon', source).value;
            } catch (e) { /* plain text is fine */ }
        }
    }

    function wirePlayground() {
        var host = document.getElementById('canon-playground');
        if (!host) return;

        var list = document.createElement('div');
        list.className = 'canon-pg-list';
        var stage = document.createElement('div');
        stage.className = 'canon-pg-stage';
        host.appendChild(list);
        host.appendChild(stage);

        var pre = document.createElement('pre');
        var code = document.createElement('code');
        code.className = 'language-canon hljs';
        pre.appendChild(code);

        var runBtn = makeButton('run');
        var buttons = document.createElement('div');
        buttons.className = 'buttons';
        buttons.appendChild(runBtn);
        pre.insertBefore(buttons, pre.firstChild);

        stage.appendChild(pre);
        var panel = buildPanel(pre, true);

        var current = null;
        var items = [];

        function select(ex, item) {
            current = ex;
            items.forEach(function (el) { el.classList.remove('active'); });
            item.classList.add('active');
            highlight(code, ex.source);
            var p = panelParts(panel);
            p.out.textContent = '';
            p.status.textContent = '';
        }

        manifest.examples.forEach(function (ex, i) {
            var item = document.createElement('button');
            item.className = 'canon-pg-item';
            item.innerHTML =
                '<span class="canon-pg-name">' + ex.name + '</span>' +
                '<span class="canon-pg-from">' + ex.title + '</span>';
            item.addEventListener('click', function () { select(ex, item); });
            list.appendChild(item);
            items.push(item);
            if (i === 0) select(ex, item);
        });

        runBtn.addEventListener('click', function () {
            if (current) enqueue(current.name, panel);
        });

        if (!hasJspi) {
            var note = document.createElement('p');
            note.className = 'canon-pg-jspi';
            note.textContent = JSPI_MSG;
            host.parentNode.insertBefore(note, host);
        }
    }

    // ── boot ────────────────────────────────────────────────────────

    function boot() {
        fetch(root + 'runner/manifest.json')
            .then(function (r) { return r.ok ? r.json() : null; })
            .then(function (m) {
                if (!m || !m.examples) return; // runner not built (local mdbook)
                manifest = m;
                m.examples.forEach(function (ex) { byName[ex.name] = ex; });
                wireBlocks();
                wirePlayground();
            })
            .catch(function () { /* no runner assets - book works without */ });
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot);
    } else {
        boot();
    }
})();
