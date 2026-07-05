# The Guide

Switching pages is a message handled by the Canon `update` function.

## State
The current page lives in the **model**. Clicking a nav button sends a
message; `update` returns a new model; the `view` re-renders.

## Content
Each page is its own `.md` file, imported by name — `Intro` loads
`intro.md`, `Guide` loads `guide.md`.
