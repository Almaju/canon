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
| `canon.toml` tables (`[deps]`, `[imports]`) | alphabetical keys (normalized, not checker-enforced — see Exemptions) |

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

When in doubt, do not compute it: `canon fmt` and the checker's error
message (``` `x` should come before `y` ```) will tell you.

## Exemptions

- **The entry point** (`main`, the HTTP handler, or a synthesised test
  entry) is exempt from the function-declaration rule and keeps its
  position: it is a distinguished role, not a regular free function.
- **Dispatch arms** are not free to be alphabetical on their own: they
  must follow the scrutinee union's variant order. Since variant order
  is alphabetical, these coincide; the arm rule is still "match the
  union", not "sort your arms".
- **`canon.toml` tables** (`[deps]`, `[imports]`) are *normalized*, not
  *enforced*: the parser (`src/manifest.rs`) stores each table in a map
  keyed by name, so it always iterates alphabetically regardless of the
  order keys were written in. Unlike every other row above, writing
  keys out of order is not a checker error — there is no rejecting
  check, and `canon fmt` does not touch `canon.toml` at all. This gap
  stays open deliberately: the manifest is slated for removal from the
  language (the no-`.toml` direction the toolchain's central `uses`
  registry already follows), so no enforcement is being added to a file
  that is on its way out.

## Auto-Fixing

The canonical order is mechanical, so you never sort by hand:

```sh
canon fmt file.can          # sorts everything into canonical order
canon check file.can        # sort order + types, no codegen
```

`canon fmt` sorts type definitions, function declarations, and
dispatch arms into canonical order. The checker's
ordering errors are the backstop for code that bypassed the formatter,
not a hand-sorting chore. Since `canon check` and `canon run` refuse
files that are not canonically formatted, violations surface
immediately rather than in review.

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
