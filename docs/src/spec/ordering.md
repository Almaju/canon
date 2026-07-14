# Ordering Rules

Canon's guiding rule: **wherever ordering is discretionary, the compiler
enforces alphabetical order**. Reordering is never a meaningful change,
so diffs that only reshuffle a list cannot exist, and two programmers
writing the same program produce the same bytes.

## Where It Applies

| Construct | Required order |
|---|---|
| Components of a product type | alphabetical |
| Variants of a union type | alphabetical |
| Function components `(A * B)` | alphabetical |
| Type definitions within a file | alphabetical |
| Function declarations within a file | alphabetical by surface name — one sequence covering constructors (their type name), shape implementations (the shape's name), and free functions |
| Trait composition (`Show = Debug * PrintString`) | alphabetical |
| Error unions in `Result<T, A + B>` | alphabetical |
| Dispatch arms | the union's variant order (itself alphabetical) |
| Literal dispatch arms | alphabetical for strings, ascending for ints; catch-all last |

One deliberate non-rule: **call operands are never reordered**. The
piped value is the first operand and literal arguments keep their
written order — operand position is meaning (untagged same-typed
components bind by declaration order, `0 -> Difference(5)` is not
`5 -> Difference(0)`), and where position carries meaning, order is not
discretionary, so the alphabetical rule does not apply.

## The Exact Comparison

"Alphabetical" means **byte-wise lexicographic comparison of the UTF-8
names**: the compiler compares the raw strings, case-sensitively. In
ASCII terms, digits sort before uppercase letters, and all uppercase
letters sort before all lowercase letters.

One consequence:

- `notFound` sorts before `noteOneBody` (`F` < `e` byte-wise), even
  though a dictionary would order them the other way.

When in doubt, do not compute it: `canon check --fix` and the checker's error
message (``` `x` should come before `y` ```) will tell you.

## Exemptions

- **The entry point** (`main`, the HTTP handler, or a synthesised test
  entry) is exempt from the function-declaration rule and keeps its
  position: it is a distinguished role, not a regular free function.
- **Dispatch arms** are not free to be alphabetical on their own: they
  must follow the scrutinee union's variant order. Since variant order
  is alphabetical, these coincide; the arm rule is still "match the
  union", not "sort your arms".
- **Project configuration** has no ordering rules because there is
  none to order: the manifest left the language ([Modules and
  Packages](./modules.md#no-manifest)), and the trees that replaced it
  (`wit/`, `deps/`, `bindgen/`) are directories — the filesystem keeps
  them sorted.

## Auto-Fixing

The canonical order is mechanical, so you never sort by hand:

```sh
canon check file.can        # sort order + types, no codegen
canon check --fix file.can  # same, sorting everything into canonical order first
```

`canon check --fix` sorts type definitions, function declarations, and
dispatch arms into canonical order before checking. Ordering errors
are reported for code that skipped `--fix`, never as a hand-sorting
chore. Since `canon check` and `canon run` refuse files that are not
canonically formatted, violations surface immediately rather than in
review.

## Source-Level Only, Never a Wire Format

Alphabetical order is a *source-level* canon. Union variants are
numbered by their alphabetical position internally, so adding a variant
renumbers everything after it. Serialized values must therefore always
carry variant **names**, not indices. At the Component Model boundary
the WIT file's declared order governs the ABI, and the compiler maps
between the two.

## Rationale

Ordering is a constant source of bikeshedding and diff noise. Forcing
one canonical order makes code read the same regardless of author and
makes "moved a declaration" disappear as a category of change. The rule
is also why Canon needs newtypes for disambiguation instead of parameter
names: if `(Name * Greeting)` were legal, argument order would be a
choice again.
