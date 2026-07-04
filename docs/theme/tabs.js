// Section tabs for the Canon book.
//
// mdBook has no native concept of top-level sections beyond sidebar
// part-titles. This script adds one: a tab strip under the menu bar
// (Start / Tour / Tutorial / Specification / Examples / Reference),
// and it filters the sidebar to show only the chapters of the active
// section. Everything is derived from the page's path, so SUMMARY.md
// remains the single source of truth for structure.

(function () {
    var SECTIONS = [
        {
            label: 'Start',
            href: 'introduction.html',
            prefixes: ['introduction.html', 'index.html', 'getting-started/']
        },
        { label: 'Tour', href: 'tour/philosophy.html', prefixes: ['tour/'] },
        { label: 'Tutorial', href: 'tutorial/index.html', prefixes: ['tutorial/'] },
        { label: 'Specification', href: 'spec/index.html', prefixes: ['spec/'] },
        { label: 'Examples', href: 'examples/index.html', prefixes: ['examples/'] },
        { label: 'Reference', href: 'reference/stdlib.html', prefixes: ['reference/'] }
    ];

    // Path of the current page relative to the book root, e.g.
    // "tour/types.html". Derived from mdBook's `path_to_root` global
    // ("../" per directory level). A directory URL (the site served at
    // "/" or "/canon/") is the book root, where index.html is the
    // first chapter — the trailing segment there is the deploy prefix,
    // not a chapter, so it must not be taken as the path.
    function currentRelPath() {
        var toRoot = typeof path_to_root !== 'undefined' ? path_to_root : '';
        var depth = (toRoot.match(/\.\.\//g) || []).length;
        var parts = document.location.pathname.split('/');
        var last = parts.pop();
        if (last === '') return 'index.html';
        return parts.slice(parts.length - depth).concat([last]).join('/');
    }

    function sectionOf(relPath) {
        for (var i = 0; i < SECTIONS.length; i++) {
            var p = SECTIONS[i].prefixes;
            for (var j = 0; j < p.length; j++) {
                if (p[j].slice(-1) === '/') {
                    if (relPath.indexOf(p[j]) === 0) return SECTIONS[i];
                } else if (relPath === p[j]) {
                    return SECTIONS[i];
                }
            }
        }
        return null;
    }

    function buildTabBar(active, toRoot) {
        var nav = document.createElement('nav');
        nav.className = 'canon-tabs';
        nav.setAttribute('aria-label', 'Book sections');
        SECTIONS.forEach(function (section) {
            var a = document.createElement('a');
            a.textContent = section.label;
            a.href = toRoot + section.href;
            if (section === active) a.className = 'active';
            nav.appendChild(a);
        });
        return nav;
    }

    // Hide sidebar entries that belong to other sections. Part titles
    // are kept only when at least one of their following chapters is
    // visible.
    function filterSidebar(active, toRoot) {
        var toc = document.querySelector('#sidebar ol.chapter, nav#sidebar ol');
        if (!toc) return;
        var items = Array.prototype.slice.call(toc.children);
        var pendingPart = null;
        items.forEach(function (li) {
            if (li.classList.contains('part-title')) {
                li.style.display = 'none';
                pendingPart = li;
                return;
            }
            if (li.classList.contains('spacer')) {
                li.style.display = 'none';
                return;
            }
            var link = li.querySelector('a[href]');
            if (!link) return;
            var rel = link.getAttribute('href') || '';
            if (toRoot && rel.indexOf(toRoot) === 0) rel = rel.slice(toRoot.length);
            var visible = sectionOf(rel) === active;
            li.style.display = visible ? '' : 'none';
            if (visible && pendingPart) {
                pendingPart.style.display = '';
                pendingPart = null;
            }
        });
    }

    // Replace the book title in the menu bar with the landing page's
    // wordmark (`*canon`). The <title> tag keeps the full book title.
    function brandMenuTitle() {
        var title = document.querySelector('#menu-bar .menu-title');
        if (!title) return;
        title.textContent = '';
        var glyph = document.createElement('span');
        glyph.className = 'canon-glyph';
        glyph.textContent = '*';
        title.appendChild(glyph);
        title.appendChild(document.createTextNode('canon'));
    }

    function init() {
        brandMenuTitle();

        // The print page concatenates every chapter; tabs and sidebar
        // filtering make no sense there.
        if (/(^|\/)print\.html$/.test(document.location.pathname)) return;

        var toRoot = typeof path_to_root !== 'undefined' ? path_to_root : '';
        var active = sectionOf(currentRelPath());
        if (!active) return;

        var menuBar = document.getElementById('menu-bar');
        if (menuBar) {
            menuBar.parentNode.insertBefore(
                buildTabBar(active, toRoot),
                menuBar.nextSibling
            );
        }
        filterSidebar(active, toRoot);
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
