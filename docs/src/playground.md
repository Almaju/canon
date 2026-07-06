# Playground

Every program below is a real Canon program from this book, compiled to a
WebAssembly component by the `canon` compiler when the site was built, and
transpiled to JavaScript with [jco](https://github.com/bytecodealliance/jco).
Pick one and run it. It executes **entirely in your browser**; there is no
server.

<div id="canon-playground"></div>

> **This whole documentation site is also a Canon program.** The pages you
> are reading are rendered by a Canon web app — the Elm-architecture triple
> compiled to WebAssembly, rendering Markdown to HTML with no JS framework.
> [Open the live app →](../app/)

## How this works

- At docs-build time, every code block in the book tagged as runnable is
  extracted and compiled with `canon build`. A snippet that stops
  compiling fails the build, so what you read is what runs.
- The resulting `wasi:cli` component is transpiled to JavaScript by jco;
  `wasi:cli/stdout` is satisfied by a ~30-line browser shim that pipes the
  program's output into the panel above.
- Canon components target WASI Preview 3, whose async ABI needs
  [JSPI](https://github.com/WebAssembly/js-promise-integration): stable
  in Chrome and Edge, in progress in Firefox and Safari.

Editing code here requires the compiler itself in the browser, a planned
follow-up (the compiler is pure Rust with no external toolchain, so a
`wasm32` build of `canon check`/`canon build` is realistic). Until then,
[install Canon](getting-started/installation.md); the whole language is
an afternoon:

```sh
curl -fsSL https://raw.githubusercontent.com/almaju/canon/main/install.sh | sh
```
