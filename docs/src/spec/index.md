# Language Specification

This section defines Canon's rules: what the lexer accepts, how the
type algebra works, how expressions evaluate, what the compiler
enforces, and what a compiled program *is* at the WebAssembly Component
level. The [Tour](../tour/philosophy.md) motivates; this section
defines.

## How to Read It

Each chapter is self-contained and cross-referenced:

- [Lexical Structure](./lexical-structure.md): tokens, identifiers,
  literals, escapes, and what the grammar omits (comments, semicolons).
- [Types](./types.md): the `+` / `*` / `^` type algebra, newtypes,
  generics, recursion, and the no-inference rule.
- [Expressions and Dispatch](./expressions.md): precedence, the
  construction-vs-observation rule, dispatch semantics, `?`, and JSON
  literals.
- [Functions and Traits](./functions.md): declarations, commutative
  calling, lambdas, traits, and the entry-point rule.
- [Ordering Rules](./ordering.md): every place alphabetical order is
  enforced, and the exact comparison used.
- [Modules and Packages](./modules.md): file/module conventions,
  automatic name resolution, manifests, and workspaces.
- [Effects and the Async Model](./effects-and-async.md): values as
  capabilities, suspension inference, and auto-await.
- [Compilation and the ABI](./compilation.md): the pipeline, the memory
  model, worlds, the WIT ↔ Canon mapping, and binding files.

## Status and Authority

Canon is an experimental language. This specification describes the
**reference implementation as it exists**, not a finished standard.
When prose here and compiler behaviour disagree, the compiler is the
fact and this text has a bug:
[file an issue](https://github.com/Almaju/canon/issues).
The repository's [`DESIGN.md`](https://github.com/Almaju/canon/blob/main/DESIGN.md)
is the working design document from which this section is distilled.
It also records intentions that are not implemented yet; the chapters
here flag those where relevant.
