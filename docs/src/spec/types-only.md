# Types-Only Canon

Canon removed local variables with the argument "names lie; types
don't." The same argument applies to the last remaining name-space:
**camelCase function names are removed from the language**. Every named
callable is a PascalCase declaration -- a type. What other languages
spell

```
insert = (Map * String * Value) => Map { ... }
map.insert("k", "v")
```

Canon spells

```
Inserted = Map

(Map * String * Value) => Inserted { ... }

map -> Inserted("k" * "v")
```

The result is a language in which **the only names are type names**.
Values are already unnamed (no `let`); operations are unnamed too -- an
operation is identified by *what it produces*, never by a verb. A
function name can drift from what the function does; a constructor
named after its return type is checked by the compiler. The naming
treadmill (`fromX`, `intoX`, `parseX`, `tryX`) disappears because there
is nothing left to name.

What survives: commutative calling, dispatch, `?`, lambdas, and `.` as
pure field access. Removing names removed nothing from the calling
convention -- `x -> Foo(y)` still reads left to right, lambdas stay
anonymous, and `x.Foo` vs `x -> Foo` stays field-read vs construction.

## The Unified Declaration

Every declaration is `PascalName = rhs`, and the RHS shape decides the
meaning:

| RHS shape | Meaning |
|---|---|
| type expression (`A * B`, `A + B`, `T^N`, alias) | type definition |
| signature, no body | **shape** -- a named function type others implement |
| signature + body, name = a declared shape | **shape implementation** |

A bodied declaration must be named after a declared shape or the type
it constructs -- and when the name *is* the constructed type, the name
is redundant and the declaration is written as an anonymous arrow
(below). Anything else is a compile error, checkable from signatures
alone. Declaring a *new* body-less shape is itself a checker error
until shapes can do something a result newtype cannot
([Functions § Shape or Result Newtype](./functions.md#shape-or-result-newtype));
the two standing shapes are the compiler's interpolation hooks,
`ToJson` and `ToHtml`, which programs implement but never re-declare.

## Anonymous Constructors -- `(A) => B { ... }`

A constructor named after its return type repeats information the
signature already carries (`Url = (String) => Result<Url, InvalidUrl>`
spells `Url` twice). So the constructor declaration form is the
**anonymous arrow** -- the typed edge itself, with no name at all:

```
String => Result<Url, InvalidUrl> { ... }        # the Url constructor
Bool => Json { ... }                             # family members are just arrows
Int => Json { ... }
Unit => Map { ... }                              # the empty-map constructor
Request => Response { ... }                      # an entire HTTP service
```

The constructed type is the return type with `Result`/`Option`/`Future`
peeled, so fallible constructors stay anonymous too. Call sites are
unchanged -- a constructor is still invoked by its output type
(`Url("...")?`); the arrow only removes the redundant declaration-site
name. A single named input drops its parentheses (`Request => Response`
is exactly `(Request) => Response`); products and generic inputs keep
theirs.

`Unit` is the name of "no input": a nullary constructor is
`Unit => Map`, not `() => Map` -- `()` is not a declaration form. The
CLI entry pairs an input world type with an output one, mirroring the
HTTP handler: `Args => Exit` (`Args = List<String>`, `Exit = Int`, both
from `canon/std`) is the argument-vector-in, exit-status-out shape,
selected by its signature exactly as the HTTP handler
`Request => Response` is. The arg-less `Unit => Program`
(`Program = Unit`) remains valid for programs that read no arguments.

This is not a second function syntax -- it is the language's **only**
function form, appearing at every level: top level it declares a
constructor, in expression position it is a lambda, and every dispatch
arm is one (`* False => Unit { ... }`). Named declarations remain for
exactly one thing: shape *implementations*
(`ToJson = (Bool) => Json { ... }`), where the name carries the only
information the types cannot. The rule reads: *if the types fully
determine the operation, it has no name; if they don't, the name is a
result newtype.*

The named form (`Url = (String) => ...`) still parses and means the
same thing, but it is not a second spelling: `canon check --fix`
rewrites it to the anonymous arrow whenever the name is exactly the
constructed type, and the format gate (`canon check`/`run` refuse
non-canonical files) makes the arrow the one form that survives.

**Constructors form families.** A type may have any number of
constructor implementations, distinguished by input product: `Json` has
`(Bool)`, `(Float)`, `(Int)`, and
`(String) => Result<Json, MalformedJson>` constructors. Coherence
generalizes from traits: at most one implementation per (name, input
product) pair in the whole program. Traits and constructor overloads
thereby collapse into one concept -- *a PascalCase name is a family of
implementations selected by input product*.

## Three Operators -- `->` executes, `.` reads, `=>` declares

Three symbols, three non-overlapping jobs:

| Symbol | Job | Editor completion after it |
|---|---|---|
| `->` | **execute** -- the only call / pipe / construct / dispatch form | functions whose input product contains the left value's type |
| `.` | **read** -- field access only | the value's fields/components |
| `=>` | **declare** -- every constructor / shape / lambda / dispatch-arm definition | -- |

The point of the split: `.` and `->` do not compete to mean "call." `.`
only *reads* a component; `->` *applies* a function **and** pipes a
scrutinee into a dispatch (`value -> ( * ... )`). So typing `.` offers
fields and typing `->` offers functions -- autocompletion on both, for
different things. And `=>` vs `->` gives declaration a spelling
distinct from execution: **`=>` defines a mapping, `->` flows a value
through one.** A `->` at a declaration site is a parse error with a
targeted message, as is the retired `value.( ... )` dispatch.

The declaration/execution mirror is the payoff -- the same shape, `=>`
when defining, `->` when running:

```
(From * String * To) => String { ... }         # declare
string -> Substring(From(1) * To(4))           # execute
```

Commutativity is preserved: `(A * B) => C` is reachable from either
side (`a -> C(b)` or `b -> C(a)`), because the pipe fills one component
of the input product and the rest follow. A function stays linked to
every type in its input, never bound to a single receiver.

`canon check --fix` canonicalizes every call, and the rule is *values
flow through pipes; literals are born in the parens*: a computed first
input pipes (`a -> Name(b)`), a lone scalar literal stays inside the
construction (`Greeting("hi")`), builtin vocabulary (`Sum`, `Print`,
`Joined`, ...) has no prefix form so literals keep piping into it, and
operand order is never reordered. See
[Expressions § Canonical Call Form](./expressions.md#canonical-call-form)
for the full case list. The retired forms and their canonical
replacements:

| Retired form | Canonical form |
|---|---|
| `"hi".Print()` | `"hi" -> Print` |
| `string.A().B().C()` | `string -> A -> B -> C` |
| `alice.Compare(bob)` / `bob.Compare(alice)` | `alice -> Compare(bob)` / `bob -> Compare(alice)` |
| `"41".Int()?.add(1)` | `Int("41")? -> Sum(1)` |
| dispatch `bool.( ... )` | `bool -> ( ... )` |
| arm `* (False) => Unit { ... }` | `* False => Unit { ... }` |
| named ctor `Url = (String) => Result<Url, _> { ... }` | `String => Result<Url, _> { ... }` |
| shape impl `ToJson = (Bool) => Json { ... }` | unchanged -- implementations keep their name |
| field `user.Birthday` | unchanged -- `.` reads |

Auto-discovery is the headline feature, not a side effect: `->`
completion queries "functions whose input product mentions this type,"
`.` completion queries the value's fields. Both providers exist in the
language server: after `->` it offers every reachable declaration whose
input product contains the piped value's type -- constructors, family
members, piped newtype wraps -- plus the builtin pipe vocabulary
applicable to that type; after `.` it offers the value's product
components (and 1-based positional indexes when a component type
repeats). camelCase FFI bindings are excluded -- they are reached
through their PascalCase result newtypes.

## What Replaces Each Kind of Function

- **Conversions and creations** are constructors
  ([Types § Conversions](./types.md#conversions)): `fromBool` ->
  `Json(Bool)`, `openFile` -> `File(Path)`.
- **Accessors** construct the accessed thing:
  `get = (Map * String) => Option<Value>` becomes a `Value`
  constructor -- `map -> Value("k")?` reads "the Value in this Map at
  this key, which might not exist." `keys` -> `Keys = List<Key>` +
  `(Map) => Keys`.
- **Endomorphisms** -- operations whose output type equals an input
  type, the one place a type genuinely underdetermines the function --
  take **result newtypes**: `Inserted = Map`, `Removed = Map`,
  `Joined = String`. This is a *checked rule*, not a convention: an
  arrow that constructs a type appearing in its own input product is a
  checker error directing to the newtype. Newtype substitutability
  makes chaining free:
  `Map() -> Inserted("a" * "1") -> Removed("a")` composes because
  `Inserted` flows anywhere `Map` is expected. The verb's information
  relocates into a name that is checked, sorted, globally resolvable,
  and usable in downstream signatures.
- **Effects produce evidence**: `write` becomes `Written = Path` +
  `(Contents * Path) => Result<Written, IoError>`. A downstream
  function that accepts `(Written)` instead of `(Path)` *requires proof
  the write happened* -- capability-style sequencing with no new
  machinery. Effect capabilities themselves are ordinary threaded
  values, received as parameters and never conjured
  ([Effects](./effects-and-async.md)); `Print` is the single tokenless
  exception.
- **Shared vocabulary** -- operations whose meaning spans types -- are
  merged result newtypes with a constructor family: `Length = Int`
  declared identically by `map.can`, `set.can`, and friends merges into
  one type, and `(Map) => Length`, `(Set) => Length` are family members
  selected by receiver. Arithmetic and comparison take the noun/pipe
  vocabulary: `Sum`, `Product`, `Difference`, `Quotient`, `Remainder`
  and `Eq`/`Lt`/`Le`/`Gt`/`Ge` (`2 -> Sum(3)`,
  `price -> Product(quantity)`), with `Maximum`/`Minimum` as stdlib
  newtypes over `Int * OtherInt` -- non-commutative cases disambiguated
  by `OtherInt` under the ordinary binding rule.
- **Higher-order operations** take lambdas -- the anonymous arrow in
  expression position:
  `list -> Mapped((Int) => Int { Int -> Product(2) })`.
  `Mapped`/`Filtered` are builtin vocabulary today (linear-memory
  layout, per [Minimal Primitives](#minimal-primitives)).
- **Entry points** are world shapes, selected by signature, never by
  name: the CLI entry `Args => Exit`, the HTTP handler
  `Request => Response`, and the web triple `Model => Html` (view) with
  `Unit => Init` and `Model * Msg => Update` marker newtypes. A literal
  `main` name is a checker error -- entries are anonymous.
- **The FFI boundary.** WIT functions are kebab-case verbs; no
  mechanical mapping can invent result nouns. Generated bindings are
  string-anchored anonymous constructors minting a result newtype per
  function (`Int => ExitWithCode { "exit-with-code" }` -- the string is
  the WIT fragment verbatim), so even the boundary is types-only. The
  camelCase alias form survives only in *hand-written* binding files
  for the two shapes the string-anchored lowering doesn't cover yet --
  resource methods (pending resource lowering) and generic combinators
  (pending generic externs) -- so camelCase in a Canon program has
  exactly one meaning: *this identifier is foreign*.

## Name Resolution Under Types-Only

Moving every operation into the type namespace multiplies name pressure
(`Item`, `Value`, `Key` recur across files), so resolution sharpens:

1. **Structurally identical duplicates are one type.** Type equality is
   syntactic (alphabetical order gives every type one canonical
   spelling), so `Length = Int` declared by both `map.can` and
   `set.can` merges into a single type -- the co-declared
   `(Map) => Length` and `(Set) => Length` arrows are then ordinary
   family members selected by receiver (`map -> Length`,
   `set -> Length`). *Differing* bodies under one name are a hard
   error.
2. **Constructor families cross files.** A name declared only as
   function bodies co-resolves across files: a reference loads *all*
   declaring files, and the error is input-product overlap, not
   co-declaration.
3. **Distinct same-shaped operations stay distinct** the same way
   same-typed parameters do: newtypes. Two different `String -> Int`
   operations are two result newtypes (`Length = Int`,
   `ByteCount = Int`) -- which is a feature: `Length` and `Age` can no
   longer accidentally interchange.

## Worked Example

`map.can` as shipped (excerpt -- the full file is
`packages/canon/std/src/map.can` in the repository):

```
Inserted = Map

Map * String * Value => Inserted {
    Map -> (
        * Empty => Inserted { Empty() -> Node(String * Value) }
        * Node => Inserted {
            String -> Lt(Node.Key) -> (
                * False => Inserted {
                    String -> Eq(Node.Key) -> (
                        * False => Inserted {
                            Node.Key -> Node(Node.Rest -> Inserted(String * Value) * Node.Value)
                        }
                        * True => Inserted { Node.Rest -> Node(String * Value) }
                    )
                }
                * True => Inserted { Map -> Node(String * Value) }
            )
        }
    )
}

Value = String

Map * String => Option<Value> {
    Map -> (
        * Empty => Option<Value> { None() }
        * Node => Option<Value> {
            Node.Key -> Eq(String) -> (
                * False => Option<Value> { Node.Rest -> Value(String) }
                * True => Option<Value> { Node.Value -> Some }
            )
        }
    )
}
```

Call site:
`Map() -> Inserted("a" * "1") -> Inserted("b" * "2") -> Value("a")?`.
Note `Value` carries two constructors -- the total newtype wrap
`Value(String)` and the fallible lookup `Value(Map * String)` -- a
family with disjoint inputs, which is the feature working, not a
collision. The recursive call stores `Inserted` into the `Rest = Map`
field through ordinary newtype substitutability.

One honest limitation: `set.can`'s operations are `Added = Set` and
`Dropped = Set`, not a second `Inserted`/`Removed`. `Inserted = Map`
and `Inserted = Set` are *differing* type bodies under one name, which
is a hard clash whenever both files load (and `Length`'s structural
merge loads both). Reference-site-only ambiguity for type names (see
[Open Work](#open-work)) is what will let the shared vocabulary return;
until it lands, cross-container endomorphism names must be distinct.

## Costs, Named Honestly

The information doesn't disappear; it relocates -- `Inserted` is
`insert` wearing PascalCase, and the claim is only that the relocation
makes the name checked, sorted, resolvable, and usable as evidence.
Naming pressure shifts to English participles, and recursive helper
chains are the stress test: the pure-Canon JSON parser (~30
same-signature anonymous arrows over result newtypes) was ported early
on purpose, and the style held. Overload resolution enters the checker,
mitigated by the absence of inference: selection uses declared types
only, and disjointness is checked at declaration.

## Minimal Primitives

The compiler supplies a builtin only when it touches something the
language *cannot* express. The test is mechanical -- a builtin is
justified by exactly one of:

1. **wasm numerics** -- `Int`/`Float` arithmetic and the base
   comparisons (`lt`, `eq`); machine ops have no decomposition.
2. **linear-memory layout** -- `String`'s
   `byteAt`/`length`/`substring`/`concat`, `List`'s slot access and
   growth; Canon values don't expose their own bytes.
3. **canonical-ABI machinery** -- `Parallel`/`Race` waitable sequences,
   `Handle` resources, async lift/lower.
4. **a host boundary** -- `print` (stdout), the `canon:builtins/*` and
   `wasi:*` imports.

Everything else lives in Canon source, like any user code. Already pure
Canon: `Map`, `Set`, the entire JSON parser and validator,
`Int(String)` parsing, `TestResult`, the HTML element vocabulary,
`Bool`'s algebra (`And`/`Or`/`Not` are three dispatches -- the union
*is* the machine op), and the derived comparisons
(`Ne`/`Le`/`Gt`/`Ge`, each one dispatch over the base `Lt`/`Eq`).

Queued to move out of the compiler as their blockers clear:
`String(Int)` decimal rendering (needs a load trigger),
`List<String>.Json()` (blocked on a
[codegen gap](../reference/codegen-gaps.md)), and `print` (becomes a
binding plus wrapper when capability entry points land).

## Open Work

The model above is enforced end to end; these are the open slices, each
independently shippable:

- **Reference-site ambiguity for type names** -- two files declaring
  *differing* bodies under one name is currently a load-time hard
  error; moving the error to the ambiguous *reference* site (plus
  `Owner.Item` type-position qualification for the genuine collisions)
  is what lets `Inserted` mean both the Map and Set operation.
- **Shapes return as justifications land** -- bare-type-parameter
  returns (`Fold`), generic constraints (`<T: Show>`), and default
  bodies are the three things a result newtype cannot do; the body-less
  shape declaration is re-admitted per case as each is implemented
  ([Functions § Shape or Result Newtype](./functions.md#shape-or-result-newtype)).
- **First-class function references** -- passing a constructor as a
  value (today every higher-order call site writes an inline lambda).
  Colliding with `.`-means-field, the likely form is bare names
  resolved by expected type.
- **Statement-initial `->`** -- zero-input application spelled `-> Now`
  needs a parser rule; today zero-arg calls stay prefix (`Now()`).
- **Capability entry points** -- printing that returns the capability
  (`Stdout -> Print("a") -> Print("b")`), retiring the tokenless
  `print` sink.
- **Extern lowering completeness** -- resource methods and generic
  combinators still bind through the camelCase alias form in
  hand-written binding files; each lands (resource lowering, generic
  externs) and the alias form retires.
