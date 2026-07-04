// Canon syntax highlighting for mdBook.
//
// mdBook's `book.js` highlights every `<code>` block via
// `hljs.highlightBlock` immediately on script load, BEFORE additional-js
// files (like this one) execute. By the time we register the `canon`
// language, mdBook has already run hljs over the blocks with the
// language missing - they end up un-highlighted.
//
// The fix: register `canon`, then re-highlight any block tagged
// `language-canon` (or `language-ow`) using `hljs.highlight(name, code)`
// directly. We replace `innerHTML` ourselves rather than going through
// `highlightBlock`, which avoids the auto-detection / no-highlight
// short-circuit hljs 10.x applies to already-processed blocks.
//
// Color philosophy: almost every token in Canon is PascalCase (types,
// constructors, variants, parameters referenced by type name), so
// painting all PascalCase one color turns snippets into a monochrome
// wall. Instead, bare PascalCase identifiers stay PLAIN (default text
// color) and color goes to the tokens that carry the program's shape:
//
//   - definitions      `greet = (…) -> …`   -> hljs-title
//   - calls            `.print()`, `.map()` -> hljs-title
//   - constructors     `True()`, `Body("x")` -> hljs-type
//                      (a PascalCase name followed by `(` CONSTRUCTS;
//                       without `(` it observes - mirroring the
//                       language's own rule)
//   - core vocabulary  `Int`, `String`, …   -> hljs-built_in
//   - literals         `True`, `None`, `Ok` -> hljs-literal
//   - structure        `->`, `?`, `*`, `+`  -> hljs-operator
//   - strings/numbers  as usual

(function () {
    function defineCanon(hljs) {
        return {
            name: 'Canon',
            aliases: ['ow'],
            keywords: {
                keyword: 'bindings extern impl use',
                literal: 'Err Fail None Ok Pass Some True False',
                built_in:
                    'Bool Byte Bytes Float Future Handle Hex Int Json ' +
                    'List Map Never Option Ord Result Set Stream String ' +
                    'TestResult Unit'
            },
            contains: [
                {
                    className: 'string',
                    begin: '"',
                    end: '"',
                    contains: [{ begin: '\\\\.' }]
                },
                {
                    className: 'number',
                    variants: [
                        { begin: '\\b0x[a-fA-F0-9_]+\\b' },
                        { begin: '\\b\\d+\\.\\d+\\b' },
                        { begin: '\\b\\d+\\b' }
                    ]
                },
                {
                    // Definition site: `name = …` at the start of a line
                    // (functions, trait impls are PascalCase and excluded
                    // on purpose - they read as types).
                    className: 'title',
                    begin: '^[ \\t]*[a-z][A-Za-z0-9_]*(?=[ \\t]*=)'
                },
                {
                    // Call site: `.method(` - the dot rides along to
                    // avoid a lookbehind (Safari compatibility).
                    className: 'title',
                    begin: '\\.[a-z][A-Za-z0-9_]*(?=\\()'
                },
                {
                    // Constructor call: PascalCase followed by `(`.
                    // Mirrors the language rule: `()` constructs, its
                    // absence observes. Bare PascalCase stays plain.
                    className: 'type',
                    begin: '\\b[A-Z][A-Za-z0-9_]*(?=\\()'
                },
                {
                    // Structural operators: arrows, propagation,
                    // turbofish, sum/product/repetition, dispatch arms.
                    className: 'operator',
                    begin: '(->|::<|\\?|\\^|\\*|\\+)'
                }
            ]
        };
    }

    function highlightCanonBlocks() {
        // Match `language-canon` with optional fence flags - a block
        // tagged ```canon,run=hello gets the single class
        // "language-canon,run=hello" (the info string up to whitespace).
        var blocks = Array.prototype.filter.call(
            document.querySelectorAll('pre code'),
            function (block) {
                return /(?:^|\s)language-(?:canon|ow)(?:,|\s|$)/.test(
                    block.className
                );
            }
        );
        blocks.forEach(function (block) {
            var raw = block.textContent;
            try {
                var result = hljs.highlight('canon', raw);
                block.innerHTML = result.value;
                block.classList.add('hljs');
            } catch (err) {
                // If something goes wrong, leave the block as-is rather
                // than nuking its content.
                console.warn('canon-highlight: failed to highlight block', err);
            }
        });
    }

    function init() {
        if (typeof hljs === 'undefined') {
            setTimeout(init, 50);
            return;
        }
        if (!hljs.getLanguage('canon')) {
            hljs.registerLanguage('canon', defineCanon);
        }
        highlightCanonBlocks();
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
