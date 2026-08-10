# Numerics

Canon has two numeric primitives, and both are exactly their wasm
type: `Int` is a two's-complement **i64**, `Float` is an IEEE 754
**binary64** (f64). There are no other widths — no `u8`, no `i32`, no
`f32` — because a second width is a second spelling for a number.
Where a narrower value has meaning, it is a newtype carrying intent,
not storage: `Byte = Int` ranges over `0..=255` by construction, and
the WIT boundary widens (`u8`/`s32`/`bool` sign- or zero-extend to
i64, `f32` promotes to f64) on the way in.

The arithmetic vocabulary is five result-type nouns, spelled the same
for both types:

| Pipe | wasm (`Int`) | wasm (`Float`) |
|---|---|---|
| `Sum` | `i64.add` | `f64.add` |
| `Difference` | `i64.sub` | `f64.sub` |
| `Product` | `i64.mul` | `f64.mul` |
| `Quotient` | `i64.div_s` | `f64.div` |
| `Remainder` | `i64.rem_s` | `a - trunc(a/b) * b` |

`Float` has no remainder instruction in wasm; the computed form's sign
follows the dividend. Operands never mix: `Int -> Sum(Float)` is a
type error, and the conversion is always written out.

## Overflow and Division

`Int` arithmetic **wraps** on overflow, silently, exactly as the wasm
instructions do — `9223372036854775807 -> Sum(1)` is the minimum
`Int`. Wrapping is the honest description of the machine; a checked
variant is a `Result`-returning stdlib function, not a second
arithmetic.

Two `Int` operations **trap** (the program aborts, the component's
post-return never runs): `Quotient`/`Remainder` by zero, and
`Quotient` of the minimum `Int` by `-1` (the one overflow wasm refuses
to wrap). A program that cannot rule out a zero divisor guards with
dispatch — the checker does not track value ranges.

`Float` never traps: division by zero is `±Inf`, and every undefined
form (`0.0 -> Quotient(0.0)`, `Inf -> Difference(Inf)`) is `NaN`,
per IEEE 754.

## Comparison

The only builtin predicates are `Eq` and `Lt` — the two wasm
comparisons (`i64.eq`/`i64.lt_s`, `f64.eq`/`f64.lt`). Everything else
(`Ne`, `Le`, `Gt`, `Ge`, `Maximum`, `Minimum`) is pure Canon in the
stdlib, one dispatch over the base pair, and `le`/`ge` is the one
spelling — there is no `lte`/`gte`.

Each is declared over a **repeated input** (`Int^2 => Gt`, reached as
`Int.1` / `Int.2`), because operand position is meaning: `a -> Gt(b)`
must be `a > b`, and the receiver is `.1`. That is the spelling
[Functions § The Binding Rule](./functions.md#the-binding-rule)
prescribes when order is the honest semantic — a two-component product
of a type and a newtype of it would instead invite binding by type,
which for an operator is exactly wrong.

On `Float` the predicates are IEEE: `NaN` compares false to
everything, itself included, so `x -> Eq(x)` is the idiomatic NaN
test's negation and `Ord` on floats does not exist — a total order
would have to lie about `NaN`. `Ord` (`Equal + Greater + Less`) is
declared for `Int^2` and `String^2` only.

## Conversion

Every conversion is an explicit construction; none is inserted by the
compiler.

- `Int -> Float` (`f64.convert_i64_s`) is exact up to 2^53 and rounds
  to nearest beyond it.
- `Float -> Int` (`i64.trunc_f64_s`) truncates toward zero — and
  **traps** on `NaN`, `±Inf`, or a magnitude outside `Int`'s range.
  A boundary that cannot rule those out converts through a validating
  stdlib constructor instead of the primitive cast.
- `String(n)` renders either type; a `Float` renders rounded to at
  most six fractional digits, trailing zeros dropped (`2.5 -> String` is
  `"2.5"`, `2.0 -> String` is `"2"`), and JSON encoding renders
  `NaN`/`±Inf` as `null` (JSON has no spelling for them).
- `Int(s)` parses: it is the stdlib's validated constructor,
  `String => Result<Int, MalformedInt>`, so every caller handles the
  failure with `?` or dispatch.

## Literals

An `Int` literal is a decimal digit run; a `Float` literal carries a
decimal point (`1.5`). There is no hex, octal, binary, exponent, or
underscore form — a base is presentation, and presentation is the
formatter's job, not the writer's. Negative values are constructed
(`Negated`, or parsing a `-`-prefixed string); the grammar has no
unary minus.
