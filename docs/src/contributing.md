# Contributing

Canon's reference compiler is written in Rust and emits WebAssembly
components directly. This page covers the two things a new contributor
most often trips over: **how the docs site is built** (this very page is
a Canon program) and **how to run the test suite**.

## This Site Is a Canon Program

The documentation you are reading is not an mdBook or a static-site
generator — it is a Canon [web app](reference/web-target.md), compiled
to WebAssembly and run in your browser. It dogfoods the web target: the
same Model-View-Update triple you would write for any Canon front end.

Three kinds of files live under `docs/src/`:

- **`*.md` pages** — the content. Every page you see in the sidebar is a
  plain Markdown file (this one is `docs/src/contributing.md`).
  Referencing the PascalCase name a file kebab-cases to (`Contributing`
  → `contributing.md`) loads its contents as a `Markdown` value, which
  the standard library's [Markdown renderer](reference/markdown-renderer.md)
  turns into `Html`. Markdown stays in `.md` files, never in Canon string
  literals.
- **`main.can`** — the app shell. It holds the router (a dispatch on the
  page name), the top bar, the sidebar, and the `init` / `view` /
  `update` triple. Adding a page means dropping a `.md` file next to the
  others, then wiring a dispatch arm and a sidebar `<button>` here.
- **`styles.can`** — the stylesheet, emitted as a `<style>` element from
  the `view`.

### Adding a Page

1. Write `docs/src/<your-page>.md` (pages live in a subdirectory per
   area — `tour/`, `reference/`, `spec/`, … — but the slug is just the
   file's basename).
2. In `main.can`, add a dispatch arm to `Page => Content`:
   `* "<your-page>" { YourPage() -> Html }` (arms are sorted
   alphabetically — `canon check --fix` enforces it).
3. Add a `{Model.Page -> NavItem(Slug("<your-page>") * Label("…"))}` line
   to `Model => Side` — or, for a spec chapter, to `Page => SpecLinks`
   plus an arm in `Page => SpecGroup` so the group opens on it.
4. A tour step needs three more: an arm in `Page => Section` marking it
   `"tour"`, a `RailDot` in `Page => Rail`, and a `Page => Pager` arm
   with the `Next`/`Prev` of its new neighbours updated. Everything else
   is reference material and has no reading order.
5. Add it to `results.can` so search can find it.

To make a snippet runnable in the browser, fence it as ` ```canon,run `
— it must be a complete program in canonical format that only prints,
since printing is the whole of the browser's host surface. The reader's
own browser compiles it on click (`docs/assets/canon-play.js`), so a
snippet that stops compiling reports the error in the page.

### Adding a Tour Step

A step under `docs/src/tour/` is the same `.md` page with one extra
rule: it holds **exactly one** Canon code block, and that block is the
step's program. `docs-enhance.js` lifts it out of the prose into the
editor on the right, so the reader runs the program the page is about.
Fence it ` ```canon,run ` for a step the browser can run, or ` ```canon `
for one that needs a real host — that variant loses its run button and
says so. Its `Page => Content` arm ends `-> Lesson -> Tour(Page)`, and
the step needs an entry in `Page => Rail` to take its place in the
numbered rail.

## Previewing the Docs Locally

Compile the site and serve it on `localhost`:

```sh
just docs
```

That runs `canon build docs` (writing the wasm bundle to `docs/build/`)
and serves it at <http://127.0.0.1:8080>. Edit a `.md` page or the app
shell, re-run `just docs`, and refresh.

Under the hood `canon build docs` compiles `docs/src/main.can` — the web
target detects the `Model => Html` view and emits a browser bundle
(`docs.wasm` + `canon-web.js` + `index.html`) instead of a CLI or HTTP
component. See [The Web Target](reference/web-target.md) for how that
detection works and what gets emitted.

## Running the Tests

One command runs everything:

```sh
just test        # cargo test — every integration harness plus unit tests
```

`cargo test` is the canonical CI gate. It drives the checker fixtures
(`tests/checker/`), full-pipeline runtime fixtures (`tests/runtime/`),
the Canon-language test suite (`tests/canon/`), and the compiler's Rust
unit tests. When an error message or a program's output changes on
purpose, regenerate the golden files with:

```sh
just update-fixtures
```

Review the resulting `git diff` — it is the surface for "did this output
change in a sensible way?". Before opening a pull request, mirror CI
locally:

```sh
just ci          # fmt --check + clippy + test
```

Examples under `examples/` are documentation, not tests; `just examples`
is an optional smoke check that the whole pipeline still runs end to end.
