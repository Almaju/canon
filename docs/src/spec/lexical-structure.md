# Lexical Structure

Canon source is UTF-8 text. The lexer produces a token stream of
identifiers, literals, keywords, and punctuation. Everything the
language rejects at this level (comments, unknown escapes) is a lexer
error with a source span.

## Identifiers

Two identifier classes, distinguished by their first character:

| Class | Form | Used for |
|---|---|---|
| **PascalCase** | `[A-Z][A-Za-z0-9_]*` | types, shapes, constructors, variants -- every name in a Canon program |
| **camelCase** | `[a-z][A-Za-z0-9_]*` | foreign imports in [binding files](./compilation.md); `canon test` functions |

The case split is load-bearing: in Canon, **the only names are type
names**, and type names are PascalCase. A camelCase declaration is a
**checker error** everywhere except two places: binding files (the FFI
boundary -- camelCase means exactly "this identifier is foreign") and
test functions (`testAddPositive = () => TestResult`, run by
`canon test`). There is no third class: no SCREAMING_CASE constants, no
leading underscores with special meaning.

## Keywords

The keyword set is exactly three words:

| Keyword | Role |
|---|---|
| `impl` | placeholder body marking a shape declaration's default implementation |
| `mut` | marks a mutable parameter |
| `Self` | the implementing type, inside a shape declaration |

There is no `let`, `if`, `else`, `match`, `while`, `for`, `return`,
`async`, `await`, `pub`, `mod`, or `use` (imports are automatic --
[Modules and Packages](./modules.md)). There is also no `extern` and no
`bindings` keyword: the grammar has zero packaging or FFI vocabulary,
and a [binding file](./compilation.md) is recognized by its shape and
path alone. The absences are deliberate; the [Tour](../guide.md) lists
what replaces each.

## Literals

| Literal | Example | Desugars to |
|---|---|---|
| Integer | `123` | `Int(123)` |
| Float | `1.5` | `Float(1.5)` |
| Hex | `0xFF0000` | `Hex(0xFF0000)` |
| String | `"abc"` | `String("abc")` |
| JSON object/array | `{"a":1}`, `[1,2]` | `Json` value ([Expressions](./expressions.md#json-literals)) |
| HTML element | `<div>{x}</div>` | `Html` value ([Expressions](./expressions.md#html-literals)) |

## String Escape Sequences

| Sequence | Meaning |
|---|---|
| `\\` | backslash |
| `\"` | double quote |
| `\n` | newline (LF) |
| `\r` | carriage return |
| `\t` | horizontal tab |
| `\0` | null byte |
| `\xNN` | ASCII byte by hex value (2 digits, `00`-`7F`; use `\u` for non-ASCII) |
| `\uNNNN` | Unicode scalar (4 hex digits) |
| `\UNNNNNNNN` | Unicode scalar (8 hex digits) |

An unrecognised escape (e.g. `\q`) is a **compile-time lexer error**, as
is a `\xNN` escape outside `00`-`7F` (a `String` is always valid UTF-8,
so a lone non-ASCII byte can't be spelled as a single escape).
There are no raw string literals.

A `String` is `Byte^*` interpreted as UTF-8. Indexing (`ByteAt`) yields
bytes, not code points. Higher-level text operations are stdlib
constructors, not language built-ins.

Strings carry the same comparison surface as `Int` -- `Eq`, `Ne`, `Lt`,
`Le`, `Gt`, `Ge` -- one spelling for comparison regardless of type.
Order is byte-wise lexicographic, shorter-first on a shared prefix
(`"app" -> Lt("apple")` is `True`): the same order the compiler
enforces on declarations, now available to programs.

## No Comments

There is no comment syntax. `//`, `/* */`, and `#` are all **lexer
errors** with a source span. Documentation belongs in types and names;
prose belongs outside the source file.

## Statement Separation and Layout

- A function body is a **newline-separated sequence of expressions**.
  There are no semicolons.
- An expression may span multiple lines when the continuation is
  syntactically unambiguous (e.g. a pipeline whose next line begins
  with `->`).
- Layout is **canonical**: `canon fmt` defines the one accepted
  formatting, and `canon check` / `canon run` refuse files that deviate
  from it. Formatting is part of the language surface, not a style
  choice. The formatter also sorts everything the
  [ordering rules](./ordering.md) cover, so it is the auto-fixer as
  well as the pretty-printer.
