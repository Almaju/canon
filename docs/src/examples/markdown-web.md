# A Docs Site in the Browser

[`examples/markdown-web`](https://github.com/Almaju/canon/tree/main/examples/markdown-web):
a small documentation site whose pages are **Markdown files rendered to
HTML by Canon**, compiled to WebAssembly and running entirely in your
browser. No React, no bundler, no npm, and no Markdown library -- the
renderer, the view, and the stylesheet are all Canon.

You are looking at one right now: this documentation site is exactly this
program, grown up -- every page is a `.md` file, parsed and rendered to
HTML on the fly by Canon, with no JavaScript framework. Run the smaller
original from a checkout:

```sh
canon run examples/markdown-web        # serves on http://127.0.0.1:8080
```

## Markdown files, imported by name

Canon has no `import` keyword -- a reference resolves to a file by name --
and that rule extends from `.can` to `.md` (see
[Markdown](../reference/markdown-renderer.md)). Referencing `Intro` loads
`intro.md` as a `Markdown` value baked in at compile time, so the content
lives in real markdown files, not string literals:

```canon
Page => Content {
    Page -> (
        * "guide" => Content { Guide() -> Html }
        * String => Content { Intro() -> Html }
    )
}

Page => Html {
    <div class="doc">
        {Styles()}
        <nav>
            <button data-msg="Show:intro">Intro</button>
            <button data-msg="Show:guide">Guide</button>
        </nav>
        <hr>
        {Page -> Content}
    </div>
}
```

`Intro() -> Html` runs the standard library's Markdown renderer -- headings,
paragraphs, **bold**, `code`, lists, fenced code blocks, and links -- the
same `Markdown -> Html` pipe you would use from the command line, only
here it runs client-side.

## The Elm triple

Like every Canon web app (see [the web target](../reference/web-target.md)),
the page is three type-selected constructors: the `Page => Html` view
above, a `Unit => Init` initial page, and a `Page * Msg => Update` reducer.
Clicking a nav button sends a message; `update` swaps the page held in the
model; the view re-renders. The message carries its own `Msg = String`
newtype so it stays distinct from the `Page` model at the value level.
