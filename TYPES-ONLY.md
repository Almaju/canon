# Types-Only Canon — Removing Named Functions

Status: design proposal. Nothing here is implemented; DESIGN.md still
describes the current language. This document works the idea out end to
end against the real stdlib surface so the decision can be made on
evidence, then gives a migration plan in slices.

## The Idea

Canon already removed local variables with the argument "names lie;
types don't." This proposal applies the same argument to the last
remaining name-space: **camelCase function names are removed from the
language**. Every named callable becomes a PascalCase declaration — a
type. What is today

```
insert = (Map * String * Value) -> Map { … }
map.insert("k", "v")
```

becomes

```
Inserted = Map

Inserted = (Map * String * Value) -> Inserted { … }

map.Inserted("k" * "v")
```

The result is a language in which **the only names are type names**.
Values are already unnamed (no `let`); after this change, operations
are unnamed too — an operation is identified by *what it produces*,
never by a verb. A Canon program becomes a composition of
constructions, each intermediate value carrying, in its type, the name
of the operation that produced it.

Why this is not just aesthetics:

- **Function names lie; return types can't.** `nowRfc3339 = () -> Now`
  is in the stdlib today — named after RFC 3339, returning `Now`. So
  are `fromBool`/`fromInt`/`fromFloat`/`fromString` (the `from` family
  DESIGN.md § Conversions already bans), `parse` (banned by the same
  section), `openFile`, `create`, `toString`. The naming treadmill
  (`fromX`, `intoX`, `parseX`, `tryX`, `makeX`, `buildX`) exists
  because verbs describe intent and intent drifts. A constructor named
  after its return type is checked by the compiler: the name *is* the
  signature.
- **One declaration form.** The grammar today distinguishes type
  definitions, function declarations, trait declarations, trait
  implementations, and validated constructors. After this change there
  is exactly one form — `PascalName = …` — and the right-hand side's
  shape decides everything. The "Naming Conventions" section of
  DESIGN.md is deleted; there is nothing left to convene about.
- **Provenance in the type system.** `Written = Path` returned from a
  write means every downstream consumer can *require evidence that the
  write happened* by accepting `Written` instead of `Path`. Verbs
  can't do this; result types can. (See "Effects produce evidence"
  below.)
- **The pattern already exists in the tree.** `set.can` declares
  `List = (Set) -> List<Item> { … }` — a bodied PascalCase declaration
  named after its return type, called as `set.List()`. Nobody planned
  that as a paradigm; it fell out of conversion-is-construction. This
  proposal is that declaration form, made total.

What survives unchanged: the dot syntax, commutative calling, dispatch,
`?`, lambdas. "Removing methods" removes the *names*, not the calling
convention — `x.Foo(y)` still reads left to right. Lambdas stay
anonymous (they already are: dispatch arms and `map` arguments never
had names). Field access vs construction stays `x.Foo` vs `x.Foo()`.

## The Unified Declaration

Every declaration is `PascalName = rhs`. The RHS shape decides the
meaning:

| RHS shape | Meaning | Exists today as |
|---|---|---|
| type expression (`A * B`, `A + B`, `T^N`, alias) | type definition | type definition |
| signature, no body (`(A) -> B`) | **shape declaration** — a named function type others implement | trait declaration / callback type |
| signature + body, name = return type | **constructor** — an implementation of the type | validated constructor, `set.can`'s `List` |
| signature + body, name = a declared shape | **shape implementation** | trait implementation |

A bodied declaration must be named either after its return type or
after a declared shape. Anything else is a compile error — that error
is the whole enforcement mechanism, and it is fully checkable from
signatures alone.

Two consequences worth spelling out:

**Constructors form families.** A type may have any number of
constructor implementations, distinguished by input product. This is
already half-implemented (the checker types `Int(String)` and
`Int(Int)` differently), but the validated-constructor rule ("if a
file declares a constructor, that *is* the constructor" — singular)
must be relaxed to: a file may declare several, provided their input
products are distinct. `Json` gets `(Bool)`, `(Float)`, `(Int)`,
`(String)` constructors, replacing today's `from*` quartet.

**Traits and constructor families become one concept.** A shape
declaration (`Length = (T) -> Length` … see vocabulary below) with
implementations per type, and a constructor family with
implementations per input, are the same machinery: *a PascalCase name
is a family of implementations selected by input product*. The
coherence rule generalizes: at most one implementation per (name,
input product) pair in the whole program, checked at link time exactly
like trait coherence today.

## What Replaces Each Kind of Function

Tested against the actual census of every camelCase function in
`packages/canon/std/src/`.

### Conversions and creations — already constructors

`fromBool` → `Json(Bool)`, `parse` → the `Url` constructor,
`openFile` → `File(Path)`, `create` → `HttpServer(Port)`,
`nowRfc3339` → `Now()`, `toString` on `Stream<String>` →
`String(stream)`, `assert` → `TestResult(Bool * Message)`. No new
machinery beyond constructor families; DESIGN.md § Conversions
mandated this all along.

### Accessors — constructors of the accessed thing

The output type is the honest name of what an accessor produces:

```
get    = (Map * String) -> Option<Value>    →   Value = (Map * String) -> Option<Value>
keys   = (Map) -> List<Key>                 →   Keys = List<Key>;  Keys = (Map) -> Keys
body   = (Request) -> Result<String, E>     →   Body = (Request) -> Result<Body, E>
method = (Request) -> String                →   Method = String;  Method = (Request) -> Method
path   = (Request) -> Option<String>        →   Path' analog, same shape
```

`map.Value("k")?` reads as "the Value in this Map at this key —
which might not exist." Note `Value` in `map.can` then has *two*
constructors — the total newtype wrap `Value(String)` and the fallible
lookup `Value(Map * String)` — a constructor family with disjoint
inputs. That is the feature working, not a collision.

### Endomorphisms — result newtypes

An operation whose output type equals an input type (`insert`,
`remove`, `concat`, `trim`, …) is where types genuinely
underdetermine the function: `(Map * String) -> Map` has many
inhabitants. The name must carry the "which one" — so the name moves
into a newtype of the result:

```
Inserted = Map
Inserted = (Map * String * Value) -> Inserted { … }

Removed = Map
Removed = (Map * String) -> Removed { … }
```

Newtype substitutability makes chaining free: `Inserted` flows
anywhere a `Map` is expected, so
`Map().Inserted("a" * "1").Inserted("b" * "2").Removed("a")` composes
without unwrapping, and the recursive call inside the body
(`Node.Rest.Inserted(…)`) stores into the `Rest = Map` field
unchanged.

This is honest about what it is: the verb's information content,
relocated from a camelCase name into a PascalCase one. What the
relocation buys: the name is now globally resolvable (loader
discovery), alphabetically sorted, usable as evidence in downstream
signatures, and *checked* — you cannot declare `Inserted` and return
something that isn't one.

### Effects — evidence types

The most interesting consequence. An effectful operation constructs
*evidence that the effect happened*, as a newtype of whatever value
flows onward:

```
write = (Contents * Path) -> Result<Path, IoError>      # today
Written = Path
Written = (Contents * Path) -> Result<Written, IoError>  # proposed

read = (File) -> Result<String, IoError>                 # today
Contents = (File) -> Result<Contents, IoError>           # proposed (Contents = String exists)
```

Today's round-trip `Contents("x").write(Path("f"))?.File()?.read()?`
becomes `Contents("x").Written(Path("f"))?.File()?.Contents()?`.

A function can now demand `(Written)` instead of `(Path)` and the type
system guarantees the file was written first — capability-style
sequencing with zero new machinery, and it composes with the planned
capability entry points (the capability appears in the constructor's
input product: `Written = (Contents * FileSystem * Path) -> …`).

Pure sinks (`-> Unit`) have no result to name. Two options, not
mutually exclusive: (a) they become shape implementations of a core
shape (`Print = () -> Unit` is *already* the trait-example in
DESIGN.md — `"hi".Print()`); (b) under capability entry points they
stop being sinks — printing returns the capability
(`Stdout.Print("a").Print("b")`), and the exemption disappears. Start
with (a), migrate to (b) when capabilities land.

### Shared vocabulary — shape declarations (the old traits)

`length` on `Map`, `Set`, `String`, `List` must not become four
`Length` newtypes in four files — same-named types in multiple files
would trip the global-uniqueness rule. Instead the *shared* operations
are declared once as shapes with per-type implementations — exactly
today's trait system, now carrying the load it was built for:

```
# core (or canon/std/length.can)
Length = Int
Length = (T) -> Length          # shape

# map.can
Length = (Map) -> Length { … }  # implementation
```

The dividing line between "newtype in your file" and "shape in core"
is the same line as today's "function vs trait": is the operation's
meaning shared across types? `Inserted` for `Map` and `Set` have
different input products (`Map * String * Value` vs `Set * String`),
so they can also coexist as one cross-file constructor family under
the generalized coherence rule — implementation selection by input
product disambiguates. The loader's "ambiguity is a hard error" rule
relaxes for constructor families only: a reference to `Inserted`
loads *all* declaring files; overlap of input products is the error.

### Arithmetic and comparison — the noun vocabulary

Arithmetic results have natural nouns, and they read well:

```
Sum        = Int;  Sum        = (Int * OtherInt) -> Sum         # 2.Sum(3)
Product    = Int;  Product    = (Int * OtherInt) -> Product     # price.Product(quantity)
Difference = Int;  Difference = (Int * OtherInt) -> Difference
Quotient   = Int;  Remainder  = Int; Minimum = Int; Maximum = Int
```

Non-commutative operations (`Difference`, `Quotient`) need the
`OtherInt` wrap at ambiguous call sites — the binding rule already
handles this (`compare = (OtherUser * User)` precedent).

Ordering: `compare` is *already* constructor-shaped —
`Ord = (Int * OtherInt) -> Ord`, so `a.Ord(b).( * (Less) … )`.
Equality and the comparison predicates (`eq`, `lt`, …) return `Bool`,
whose name carries nothing; they become core shapes: `Eq`, `Lt`, `Le`,
`Gt`, `Ge` (`2.Lt(3)`), implemented for `Int`, `Float`, `String`.
Alternatively the predicates are dropped in favor of `Ord` + dispatch;
keeping the shapes is the pragmatic call (JSON parser code does a lot
of `eq`).

### Higher-order operations — generic result newtypes

```
Mapped<U> = List<U>
Mapped = <T, U>(((T) -> U) * List<T>) -> Mapped<U>

Numbers.Mapped((Int) -> Int { Int.Product(2) })
Numbers.Mapped(Int.Doubled)                      # first-class constructor reference
```

`Filtered<T> = List<T>` likewise. Note `Mapped` ≠ `Map` — the
participle convention keeps operation names naturally disjoint from
container names. `fold` is the genuinely awkward one: its result type
is a bare type parameter (`Folded<A> = A` is a degenerate alias), so
`Fold` stays a shape declaration, not a constructor. Generic shapes
are today's generic traits; nothing new.

First-class references generalize cleanly: `Type.Constructor`
(`Int.Doubled`) replaces `Type.function` (`Int.double`) — same
resolution, PascalCase.

### Entry points — worlds are shapes

The entry-point registry becomes core shape declarations:

```
Main    = () -> Unit                    # and the Result/ExitCode variants
Handler = (Request) -> Response
Init    = <M>() -> M
Update  = <M>(M * String) -> M
View    = <M>(M) -> Html
```

A program is a module implementing a world shape; `canon build` keys
on which shape is implemented. This fixes an existing inconsistency:
the web triple is today selected by *names* (`init`/`update`/`view`) —
the only place the language keys on function names, against its own
"no magic `main`" principle. Under shapes, selection is by declared
implementation, uniformly.

### The FFI boundary — camelCase means foreign

WIT functions are kebab-case verbs; no mechanical mapping can invent
result nouns for them. Binding files therefore keep the mechanical
camelCase mapping (`getRandomU64`), and **binding files become the
only place camelCase is legal**. The raw/idiom layering already in
place does the rest: every stdlib wrapper is a constructor over the
raw binding. camelCase in a Canon program then has exactly one
meaning — "this identifier is foreign" — visible at a glance, the way
`unsafe` marks a boundary in Rust. Calling a raw binding directly from
user code stays legal (everything is public) and stays camelCase,
advertising that you've left the discipline.

## Worked Example: `map.can` Ported

```
Inserted = Map

Inserted = (Map * String * Value) -> Inserted {
    Map.(
        * (Empty) -> Inserted { Node(String, Empty(), Value) }
        * (Node) -> Inserted {
            String.Lt(Node.Key).(
                * (False) -> Inserted {
                    String.Eq(Node.Key).(
                        * (False) -> Inserted { Node(Node.Key, Node.Rest.Inserted(String * Value), Node.Value) }
                        * (True) -> Inserted { Node(String, Node.Rest, Value) }
                    )
                }
                * (True) -> Inserted { Node(String, Map, Value) }
            )
        }
    )
}

Key = String

Keys = List<Key>

Keys = (Map) -> Keys {
    Map.(
        * (Empty) -> Keys { List() }
        * (Node) -> Keys { List(Node.Key).Joined(Node.Rest.Keys()) }
    )
}

Length = (Map) -> Length {
    Map.(
        * (Empty) -> Length { 0 }
        * (Node) -> Length { Node.Rest.Length().Sum(1) }
    )
}

Map = Empty + Node

Map = () -> Map {
    Empty()
}

Node = Key * Rest * Value

Removed = Map

Removed = (Map * String) -> Removed {
    Map.(
        * (Empty) -> Removed { Empty() }
        * (Node) -> Removed {
            Node.Key.Eq(String).(
                * (False) -> Removed { Node(Node.Key, Node.Rest.Removed(String), Node.Value) }
                * (True) -> Removed { Node.Rest }
            )
        }
    )
}

Rest = Map

Value = String

Value = (Map * String) -> Option<Value> {
    Map.(
        * (Empty) -> Option<Value> { None() }
        * (Node) -> Option<Value> {
            Node.Key.Eq(String).(
                * (False) -> Option<Value> { Node.Rest.Value(String) }
                * (True) -> Option<Value> { Some(Node.Value) }
            )
        }
    )
}

Values = List<Value>

Values = (Map) -> Values { … }
```

Call sites:

```
Map().Inserted("a" * "1").Inserted("b" * "2").Value("a")?
```

Observations from doing the port: nothing needed that isn't in this
proposal (constructor families, shape impls, newtype substitutability
— the last already ships); the file gains four one-line newtype
declarations and loses nothing; alphabetical declaration order now
interleaves types and their constructors, which reads naturally
because a constructor sorts adjacent to its type; the `Value` family
(total wrap + fallible lookup) is the overloading feature carrying
real weight on day one.

## Costs, Named Honestly

- **The information doesn't disappear; it relocates.** `Inserted` is
  `insert` wearing PascalCase. The claim is not that operation names
  vanish, but that relocating them into the type system makes them
  checked, sorted, globally resolvable, and usable as evidence.
  Anyone evaluating this proposal should reject the stronger claim.
- **Naming pressure shifts to participles.** English past participles
  (`Inserted`, `Removed`, `Written`, `Joined`) do heavy lifting.
  Some operations resist nouning (`skipWs` → `SkippedWs = ParsePos`?).
  The JSON parser is the stress test: ~20 same-signature
  `(Int * String) -> ParseStep` steps, each needing a newtype
  (`ParsedValue = ParseStep`, `ParsedArrayTail = ParseStep`, …). It
  works — the current camelCase names are exactly these participles
  already (`parseValue`, `parseArrayTail`) — but the port is the
  single biggest chunk of mechanical work, and porting it *first* is
  the best way to discover whether the style holds up. If it doesn't,
  that's the evidence to stop at the halfway rule (below).
- **Overload resolution enters the checker.** Constructor families
  selected by input product are a form of overloading; Canon has so
  far avoided it (no inference keeps everything simple). Mitigation:
  selection uses only *declared* types, never inferred ones (there is
  no inference), disjointness is checked at declaration, and the
  commutative binding rule already implements most of the matching
  logic.
- **Loader relaxation.** "Ambiguity is a hard error" softens for
  constructor families: a reference loads all declaring files, and
  the error moves to input-product overlap. This must not soften for
  plain types — two `Key = String` declarations in two files stay a
  hard error.
- **Error-message quality.** "no constructor of `Json` accepts
  `(Bool * Int)`; families declared: `(Bool)`, `(Float)`, `(Int)`,
  `(String)`" must be as good as "unknown function `fromBool`" was.
  Budget real work here.

## The Halfway Rule (fallback position)

If full removal proves too costly in practice, there is a coherent
stopping point that captures most of the value and is independently
worth enforcing:

> A camelCase function's return type must appear in its input product
> (modulo newtype chains and `Result`/`Option`/`Future` wrapping), or
> be `Unit`. Any function producing a type it wasn't given must be
> named after what it produces.

"Verbs transform, constructors create." Endomorphisms and sinks keep
verbs (the one place a name is the only information channel);
everything type-changing becomes a constructor. Every stdlib violation
listed above is caught by this rule; `insert`/`remove`/`concat`
survive. Slices 1–3 below are identical under both endpoints, so the
decision can be deferred until after the JSON-parser port.

## Migration Plan

1. **Constructor families, in-file** — relax the single-validated-
   constructor rule; overload selection by input product; checker
   disjointness errors. Prerequisite for everything else.
   (Test surface: `Json(Bool/Float/Int/String)` replacing `from*`.)
2. **Stdlib cleanup to current spec** — convert the existing
   § Conversions violations (`from*`, `parse`, `openFile`, `create`,
   `nowRfc3339`, `toString`, `asString`, `assert`, `body`, `method`,
   `path`) to constructors. No language change beyond slice 1; fixes
   the DESIGN.md turbofish example (`"…".parse::<List<Int>>()` →
   `"…".List<Int>()`). Ships value regardless of the endgame.
3. **Cross-file constructor families** — loader relaxation +
   link-time coherence by (name, input product).
4. **Core vocabulary** — `Sum`/`Product`/`Difference`/…, `Eq`/`Lt`/…,
   `Length`, `Mapped`/`Filtered`, `Joined`, `Print` shape. Initially
   thin aliases over the existing builtins (`I64Add` etc. unchanged
   in codegen); the camelCase spellings stay as deprecated aliases
   until slice 6.
5. **Stdlib port** — file by file, `json.can` first (it is the stress
   test; see Costs). Decision checkpoint: full removal vs halfway
   rule, based on how the parser port reads.
6. **Enforcement** — camelCase outside binding files: warning, then
   error. Delete the deprecated aliases. Entry-point worlds become
   shape implementations.
7. **Spec rewrite** — fold this document into DESIGN.md; delete the
   Naming Conventions section; rewrite Functions/Traits as the
   unified declaration.

## Open Questions

- `fold` and any operation whose result is a bare type parameter:
  shape declaration is the answer, but the shape/constructor split
  needs a crisp spec sentence ("if the return type is a type
  parameter, declare a shape").
- Comparison surface: keep `Eq`/`Lt`/`Le`/`Gt`/`Ge` shapes, or force
  everything through `Ord` + dispatch? (Proposal: keep the shapes;
  parser-style code does too much `eq` for dispatch ceremony.)
- Whether `Print`-style sink shapes survive capability entry points
  or are replaced by capability-threading (`Stdout.Print("a")`
  returning `Stdout`). Proposal: the latter, when capabilities land.
- HTML helpers (`div`, `h1`, `li`, `span`, all `(String) -> Html`):
  tag newtypes (`Div = Html`; `"hi".Div()`) or lean entirely on HTML
  literals and delete the helpers. Proposal: delete — literals
  already won.
- `canon fmt` autofix: can the formatter mechanically rename
  known-pattern verbs (`insert` → `Inserted`) during migration? Worth
  a one-off `canon upgrade` fixer even if not.
