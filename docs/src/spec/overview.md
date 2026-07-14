# Language Specification

This section defines Canon's rules: what the lexer accepts, how the
type algebra works, how expressions evaluate, what the compiler
enforces, and what a compiled program *is* at the WebAssembly Component
level. The **Learn** chapters (starting at
[Types & Values](../learn/types-and-values.md)) motivate; this section
defines.

The chapters (in the sidebar, in reading order) are self-contained and
cross-referenced: lexical structure, the type algebra, expressions and
dispatch, functions and traits, ordering, modules, effects and async,
compilation and the ABI, and the Types-Only naming model.

## Status and Authority

Canon is an experimental language. This specification describes the
**reference implementation as it exists**, not a finished standard, and
is the canonical design document for the language. When prose here and
compiler behaviour disagree, the compiler is the fact and this text has
a bug: [file an issue](https://github.com/Almaju/canon/issues).
