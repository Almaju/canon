# Examples

A small, curated set of **real programs** from the repository's
[`examples/`](https://github.com/Almaju/canon/tree/main/examples)
directory. Each page shows the program's real source, kept in step with
the compiling code by a drift-guard test in the build.

These five are the showcase: a multi-file project, an HTTP JSON API, an
interactive browser frontend, a documentation site rendered in the
browser, and a fullstack app that shares one set of types across both.
The rest of the language's surface is exercised as
**tests**, not examples -- the deterministic feature demos live in
`tests/runtime/` (where CI pins their exact output) and the stdlib is
covered by `tests/canon/`. Examples exist only to show *real-world
usage*: I/O, networking, a browser, things with environment-dependent
behavior.

| Example | What it shows | Page |
|---|---|---|
| `multifile` | modules: one type per file, imports by reference | [A Multi-File Project](./multifile.md) |
| `notes-api` | a JSON API as a `wasi:http/service` component | [notes-api](./notes-api.md) |
| `todolist-web` | an interactive browser frontend (the Elm triple) with `localStorage` persistence, live preview | [A Todo List in the Browser](./todolist.md) |
| `markdown-web` | a docs site compiled to wasm -- Markdown rendered to HTML by Canon, running in the browser | [A Docs Site in the Browser](./markdown-web.md) |
| `todo-fullstack` | one language on both sides -- a frontend and a backend sharing types | [Fullstack](./fullstack.md) |

## Running Them

From a checkout of the repository:

```sh
canon run examples/notes-api        # any single example
just examples                       # compile + run all, report pass/fail
```

Each packaged example is an ordinary `canon.toml` plus `src/`, so it
doubles as a template for starting your own project.
