// Oneway syntax highlighting for mdBook.
//
// mdBook's `book.js` highlights every `<code>` block via
// `hljs.highlightBlock` immediately on script load, BEFORE additional-js
// files (like this one) execute. By the time we register the `oneway`
// language, mdBook has already run hljs over the blocks with the
// language missing — they end up un-highlighted.
//
// The fix: register `oneway`, then re-highlight any block tagged
// `language-oneway` (or `language-ow`) using `hljs.highlight(name, code)`
// directly. We replace `innerHTML` ourselves rather than going through
// `highlightBlock`, which avoids the auto-detection / no-highlight
// short-circuit hljs 10.x applies to already-processed blocks.

(function () {
    function defineOneway(hljs) {
        return {
            name: 'Oneway',
            aliases: ['ow'],
            keywords: {
                keyword: 'match mut use Self impl extern while for',
                type:
                    'Bit Bool Byte Bytes Clock Datetime Empty Filesystem ' +
                    'Float Hex HttpClient HttpServer HttpError InvalidUrl ' +
                    'IoError Int Json List Map MalformedJson Network Noop ' +
                    'Option Ord Path Random Result Stderr Stdin Stdout ' +
                    'String Url',
                literal:
                    'False True Off On None Some Ok Err ' +
                    'Equal Greater Less Noop',
                built_in: 'Rust'
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
                    // Trait/type identifiers (PascalCase) — anything not
                    // already in the keyword/type/literal table.
                    className: 'type',
                    begin: '\\b[A-Z][A-Za-z0-9_]*\\b'
                },
                {
                    // Private-method sigil: `*helper`
                    className: 'symbol',
                    begin: '\\*[a-z][A-Za-z0-9_]*'
                },
                {
                    // Arrows + propagation + turbofish + spread
                    className: 'operator',
                    begin: '(->|=>|::<|\\?|\\.\\.\\.)'
                }
            ]
        };
    }

    function highlightOnewayBlocks() {
        var blocks = document.querySelectorAll(
            'pre code.language-oneway, pre code.language-ow'
        );
        blocks.forEach(function (block) {
            var raw = block.textContent;
            try {
                var result = hljs.highlight('oneway', raw);
                block.innerHTML = result.value;
                block.classList.add('hljs');
            } catch (err) {
                // If something goes wrong, leave the block as-is rather
                // than nuking its content.
                console.warn('oneway-highlight: failed to highlight block', err);
            }
        });
    }

    function init() {
        if (typeof hljs === 'undefined') {
            setTimeout(init, 50);
            return;
        }
        if (!hljs.getLanguage('oneway')) {
            hljs.registerLanguage('oneway', defineOneway);
        }
        highlightOnewayBlocks();
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
