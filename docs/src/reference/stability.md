# Stability

From 1.0, a Canon program keeps compiling. This page is the promise:
what a 1.x toolchain holds fixed, what it may still change, and how a
change that breaks programs reaches them.

## What 1.x holds fixed

A program that `canon check` accepts under a 1.x toolchain is accepted
by every later 1.x toolchain and behaves the same. That covers:

- **The language.** Its syntax, its checker rules, and the meaning of
  every accepted program.
- **The canonical form.** A canonical file stays canonical: a later
  formatter does not rewrite it.
- **The prelude.** Every name the `canon` package declares, and every
  constructor's inputs and result. New names may be added; none are
  removed or changed.
- **The entry shapes and their worlds.** `Unit => Program`,
  `Request => Response` and the web triple, and the WIT world each
  compiles to.
- **The project layout.** `src/`, `wit/`, `bindgen/` and
  `deps/<ns>/<name>@<version>/`. A vendored dependency is a directory
  of files, so it stays at its version whatever the toolchain.

## What 1.x may still change

- **Accepted = implemented.** A program the checker accepts but the
  compiler cannot build, or builds wrong, is a bug. Its fix may reject
  the program in a patch release: it never worked, and a diagnostic is
  better than a broken build.
- **Closing a [codegen gap](./codegen-gaps.md)** only adds programs.
- **Diagnostics** — wording, order, and the suggestions they carry.
- **Everything outside the language**: the compiler's Rust API,
  `canon inspect` output, the generated API reference's layout, and the
  ecosystem packages' own versions.

## How a breaking change arrives

A change that would reject or change an accepted program waits for 2.0.
When the old spelling maps mechanically onto the new one, 2.0 ships the
rewrite in `canon check --fix`: one spelling per program makes the
migration a pass of the formatter rather than a hand edit.

## Channels

`stable` carries the tagged releases this page describes. `nightly` is
every push to `main` and carries no promise; `canon use nightly` scopes
it to a directory (see [the canon CLI](../getting-started/building-and-running.md)).
