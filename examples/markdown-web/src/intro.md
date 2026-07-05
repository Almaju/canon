# Canon in the Browser

This page is a **markdown file** imported into a Canon web app.

## How it works
Canon compiles to a WebAssembly component and runs client-side. The
document is loaded from `intro.md` and rendered to HTML by the standard
library `Markdown` renderer, entirely in Canon.

## No framework
There is no React, no bundler, no JavaScript you wrote. The `view`, the
`update` loop, and the **Markdown renderer** are all Canon.
