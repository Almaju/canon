# The Guide

Switching pages is a message handled by the Canon `update` function.

## The loop
The current page lives in the *model*:

1. clicking a nav button sends a message
2. `update` returns a new model
3. the `view` re-renders

## What the renderer handles
- headings and paragraphs
- inline: **bold**, *italic*, `code`, [links](https://github.com/almaju/canon)
- lists, including nested ones:
  - like this sub-item
  - and this one
- blockquotes and fenced code

> Every page here is its own `.md` file, imported by name and rendered to
> HTML by the standard library - no JavaScript, no bundler.

All of it - **renderer included** - is Canon compiled to WebAssembly.
