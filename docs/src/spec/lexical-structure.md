# Lexical Structure

Canon source is UTF-8 text. The lexer produces a token stream of
identifiers, literals, keywords, and punctuation. Everything the
language rejects at this level (comments, unknown escapes) is a lexer
error with a source span.

## Identifiers

Two identifier classes, distinguished by their first character:

| Class | Form | Used for |
|---|---|---|
| **PascalCase** | `[A-Z][A-Za-z0-9_]*` | types, traits, trait implementations, constructors, variants |
| **camelCase** | `[a-z][A-Za-z0-9_]*` | functions |

The case split is load-bearing: `print` is a function, `Print` is a
trait (and `Print = (Foo) -> …` implements it for `Foo`). There is no
third class: no SCREAMING_CASE constants, no leading underscores with
special meaning.

## Keywords

The keyword set is small:

| Keyword | Role |
|---|---|
| `use` | import a type ([Modules and Packages](./modules.md)) |
| `extern` | bind a declaration to a Component Model import ([Compilation](./compilation.md)) |
| `bindings` | file-level directive naming the WIT interface a generated binding file covers |
| `impl` | placeholder body marking a trait declaration's default implementation |

There is no `let`, `if`, `else`, `match`, `while`, `for`, `return`,
`async`, `await`, `pub`, or `mod`. The absences are deliberate; the
[Tour](../tour/philosophy.md) lists what replaces each.

## Literals

| Literal | Example | Desugars to |
|---|---|---|
| Integer | `123` | `Int(123)` |
| Float | `1.5` | `Float(1.5)` |
| Hex | `0xFF0000` | `Hex(0xFF0000)` |
| String | `"abc"` | `String("abc")` |
| JSON object/array | `{"a":1}`, `[1,2]` | `Json` value ([Expressions](./expressions.md#json-literals)) |

## String Escape Sequences

| Sequence | Meaning |
|---|---|
| `\\` | backslash |
| `\"` | double quote |
| `\n` | newline (LF) |
| `\r` | carriage return |
| `\t` | horizontal tab |
| `\0` | null byte |
| `\xNN` | byte by hex value (2 digits) |
| `\uNNNN` | Unicode scalar (4 hex digits) |
| `\UNNNNNNNN` | Unicode scalar (8 hex digits) |

An unrecognised escape (e.g. `\q`) is a **compile-time lexer error**.
There are no raw string literals.

A `String` is `Byte^*` interpreted as UTF-8. Indexing (`byteAt`) yields
bytes, not code points. Higher-level text operations are stdlib
functions, not language built-ins.

Strings carry the same comparison surface as `Int` — `eq`, `ne`, `lt`,
`le`, `gt`, `ge` — one spelling for comparison regardless of type.
Order is byte-wise lexicographic, shorter-first on a shared prefix
(`"app".lt("apple")` is `True`): the same order the compiler enforces
on declarations, now available to programs.

## No Comments

There is no comment syntax. `//`, `/* */`, and `#` are all rejected at
compile time. Documentation belongs in types and names; prose belongs
outside the source file.

## Statement Separation and Layout

- A function body is a **newline-separated sequence of expressions**.
  There are no semicolons.
- An expression may span multiple lines when the continuation is
  syntactically unambiguous (e.g. a chain whose next line begins with
  `.`).
- Layout is **canonical**: `canon fmt` defines the one accepted
  formatting, and `canon check` / `canon run` refuse files that deviate
  from it. Formatting is part of the language surface, not a style
  choice. The formatter also sorts everything the
  [ordering rules](./ordering.md) cover, so it is the auto-fixer as
  well as the pretty-printer.
