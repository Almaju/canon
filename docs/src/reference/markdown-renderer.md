# Markdown

Canon's standard library can render Markdown to HTML entirely in Canon --
no external parser, no build plugin. The renderer is an ordinary Canon
program that walks a `String` byte-by-byte and emits `Html`, compiled
through the same pipeline as everything else. It lives in
`canon/std` as `markdown.can` and is modelled on the JSON parser
(`json.can`): a cursor threaded as an `Int`, dispatch on the byte at the
cursor, recursion in place of loops.

## Rendering

`Markdown` is a `String` newtype. Piping it to `Html` runs the renderer:

```canon
Args => Exit {
    Markdown("# Canon Docs\nRendered by Canon itself.\n\n## Why\nThe docs compile through the same pipeline as programs.")
        -> Html
        -> Print
    Exit(0)
}
```

Output:

```html
<h1>Canon Docs</h1><p>Rendered by Canon itself.</p><h2>Why</h2><p>The docs compile through the same pipeline as programs.</p>
```

Because `File` reads a document as a `String` (see [Using WASI
Interfaces](./wasi.md)), a whole file renders at runtime in one pipe:

```canon
Args => Exit {
    Path("notes.md")
        -> File?
        -> Read?
        -> Markdown
        -> Html
        -> Print
    Exit(0)
}
```

## Importing markdown files

Writing markdown inside `.can` string literals is awkward. Canon has no
`import` keyword -- a reference resolves to a file by name -- and that rule
extends from `.can` to `.md`: **referencing the PascalCase name a
markdown file kebab-cases to loads the document as a `Markdown` value**,
baked in at compile time.

Given `intro.md` beside your source, `Intro` names it:

```canon
Args => Exit {
    Intro()
        -> Html
        -> Print
    Exit(0)
}
```

The compiler synthesizes `Intro = Markdown` and a nullary constructor
carrying the (escaped) file contents, then resolves `Markdown` to the
stdlib renderer as usual. `Intro()` is the document; `Intro() -> Html`
renders it. The markdown lives in `intro.md`, never in a string literal,
and `canon check --fix` leaves `.md` files untouched.

## In the browser

The renderer is pure string work -- no host imports -- so it runs in the
[web target](./web-target.md) too. A web app's `view` can render an
imported document client-side, so the page *is* a Canon program compiled
to WebAssembly:

```canon
Page => Html {
    <div class="doc">
        <nav>...</nav>
        <hr>
        {Page -> Content}
    </div>
}
```

The view is written as an HTML literal; `{Page -> Content}` interpolates
the rendered document (Html passes through the hole unescaped).

See `examples/markdown-web` for the full triple: nav messages switch the
page held in the model, and each page is its own imported `.md` file,
rendered to HTML entirely in Canon with no JavaScript and no bundler.

## What it renders

The renderer is a practical subset, not a full CommonMark implementation:

| Markdown | HTML |
|---|---|
| `# H`, `## H`, `### H` (space required) | `<h1>`/`<h2>`/`<h3>`; deeper levels clamp to `<h3>` |
| consecutive text lines | one `<p>...</p>` (soft-wrapped lines join with a space) |
| `- item` lines | `<ul><li>...</li></ul>` (one level of `  - ` nesting) |
| `1. item` lines | `<ol><li>...</li></ol>` |
| `> quote` lines | `<blockquote>...</blockquote>` (consecutive lines joined) |
| ` ``` ` fenced block | `<pre><code>...</code></pre>` (raw, escaped, no inline pass) |
| `**bold**` | `<strong>...</strong>` (inner text formatted) |
| `*italic*` | `<em>...</em>` (inner text formatted) |
| `` `code` `` | `<code>...</code>` (contents escaped) |
| `[text](url)` | `<a href="url">...</a>` (url escaped, text formatted) |
| `\| a \| b \|` + `\|---\|---\|` | `<table>` with a `<thead>` header row and `<tbody>` body rows (cells formatted inline) |
| blank lines | block separators |

Text is HTML-escaped as it is walked (`"` `&` `<` `>`), so `a < b & c`
renders as `a &lt; b &amp; c`. A `#` with no following space, and a lone
unmatched `*`, are treated as literal text.

Pipe tables (a header row, a `|---|---|` delimiter, then body rows) render
to `<table>`; cell text runs through the inline pass, so `**bold**`,
`` `code` ``, and links work inside cells.

Not yet handled: lists nested more than one level, `_underscore_`
emphasis, setext headings, and images -- each an additive
extension in the same byte-walking style. The renderer is byte-oriented, so non-ASCII (UTF-8) text in string
literals is subject to the compiler's existing lexer handling of
multi-byte characters; ASCII markdown is unaffected.

## Why this exists

A language that compiles to WebAssembly and runs in the browser should be
able to present its own documentation as a Canon app, not a separate
toolchain. The Markdown renderer plus `.md` import make that direct:
content is authored as ordinary markdown files, imported by name, and
rendered to HTML by the standard library -- on the server for a CLI
generator, or client-side inside the [web target](./web-target.md), where
Canon acts as the frontend framework. The same renderer serves both,
exercising strings, dispatch, escaping, and file resolution end to end.

## Current limits

Two compiler rough edges are worth knowing when writing renderer-style
code (both are bugs, tracked, not language rules):

- When a piped value and a paren argument could fill each other's
  fields in a two-field constructor, prefer piping the *earlier* field
  (`level -> HeadingHtml(content)`, not the reverse) — the other order
  can miscompile.
- When a web app's model is a `String` newtype, give the update's
  message its own newtype too (`Msg = String`, `Page * Msg => Update`),
  so a bare `String` reference in the body is unambiguous.
