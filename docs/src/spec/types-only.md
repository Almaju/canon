# Types-Only Canon

Canon removed local variables with the argument "names lie; types
don't." The same argument removes the last name-space: **there are no
function names**. Every named callable is a PascalCase declaration -- a
type. What other languages spell

```
insert = (Map * String * Value) => Map { ... }
map.insert("k", "v")
```

Canon spells

```
Insert = Key * Value

Map * Insert => Map { ... }

map -> Insert(Key("k") * Value("v"))
```

A query is identified by *what it produces*, never by a verb: a
constructor named after its return type is checked by the compiler. A
command — an operation that gives its input back — is identified by
the **message** it takes, a type declared for the operation, and is
applied by piping the value into that message. Either way the name is a
type the compiler checks, and a function name that could drift from
what the function does never exists.

## The Unified Declaration

Every declaration is `PascalName = rhs`, and the RHS decides the
meaning: a type expression declares a type; a body-less function-type
signature (a "shape") is a checker error --
[Canon has no traits](./functions.md#no-traits).

A bodied declaration named after anything but the type it constructs
is a compile error, and since that name is redundant, the declaration
is the **anonymous arrow**:

```
String => Result<Url, InvalidUrl> { ... }        # the Url constructor
Unit => Map { ... }                              # the empty-map constructor
Request => Response { ... }                      # an entire HTTP service
```

The constructed type is the return type with `Result`/`Option`/`Future`
peeled. Call sites are unchanged (`Url("...")?`). This is the
language's **only** function form: top level it declares a constructor,
in expression position it is a lambda, and every dispatch arm is one.
A body-less signature naming a type family is a checker error
([Functions § Operations Have No Names](./functions.md#operations-have-no-names));
there are no exceptions -- even the literal-interpolation hooks are
ordinary result-newtype families (`Encoded = Json`, `Escaped = Html`;
a hole is a construction, exactly as a format-string hole is
`-> String`).

**Constructors form families.** A type may have any number of
constructor implementations, distinguished by input product (`Json`
has `(Bool)`, `(Float)`, `(Int)`, and `(String)` constructors), with
at most one implementation per (name, input product) pair in the whole
program. Traits and overloads collapse into one concept: *a PascalCase name is
a family of implementations selected by input product*.

## Three Operators

| Symbol | Job | Editor completion after it |
|---|---|---|
| `->` | **execute** -- the only call / pipe / construct / dispatch form | functions whose input product contains the left value's type |
| `.` | **read** -- field access only | the value's fields/components |
| `=>` | **declare** -- every constructor / lambda / dispatch-arm definition | -- |

`.` and `->` do not compete to mean "call", so `.` completion offers
fields and `->` completion offers every operation reachable from the
value's type -- discovery is the payoff of the split. `=>` gives
declaration a spelling distinct from execution; a `->` at a
declaration site is a parse error:

```
(From * String * To) => String { ... }         # declare
string -> Substring(From(1) * To(4))           # execute
```

## What Replaces Each Kind of Function

- **Conversions and creations** are constructors: `openFile` ->
  `File(Path)` ([Types § Conversions](./types.md#conversions)).
- **Accessors** construct the accessed thing: `map -> Value("k")?`
  reads "the Value in this Map at this key, which might not exist."
- **Commands** (output type = an input type, the one place types
  underdetermine the operation) take a **message**: `Insert = Key *
  Value`, `Map * Insert => Map`. Checked, not conventional: an arrow
  constructing one of its inputs must take exactly one other input, a
  declared, non-primitive type that is not a part of the value it
  applies to. A command is reached only by piping the value into its
  message, so chaining is free --
  `Map() -> Insert(Key("a") * Value("1")) -> Remove("a")` -- and the
  message is data that can be stored and applied later.
- **Effects produce evidence**: a write returns `Written = Path`; a
  function accepting `(Written)` requires proof the write happened
  ([Effects](./effects-and-async.md)).
- **Shared vocabulary** is a merged result newtype with a family:
  every container declares `Length = Int` and contributes its arrow.
  Arithmetic and comparison are the pipe vocabulary (`Sum`, `Product`,
  `Eq`, `Lt`, ...).
- **Entry points** are world shapes selected by signature
  ([Functions § The Entry Point](./functions.md#the-entry-point)); a
  literal `main` name is an error.
- **The FFI boundary**: generated bindings mint a result newtype per
  WIT function (`Int => ExitWithCode { "exit-with-code" }`), so even
  the boundary is types-only; camelCase in a Canon program means
  exactly "this identifier is foreign"
  ([Compilation § Binding Files](./compilation.md#binding-files)).

## Name Resolution

Type names recur across files (`Value`, `Key`, `Length`), so:

1. **Structurally identical declarations are one type** -- `Length =
   Int` in both `map.can` and `set.can` merges; the co-declared arrows
   become family members. *Differing* bodies under one name are a hard
   error.
2. **Function-only names co-resolve across files**; the error is input
   overlap, not co-declaration.
3. **Distinct same-shaped operations take distinct newtypes**
   (`Length = Int`, `ByteCount = Int`) -- which is a feature: they can
   no longer accidentally interchange. A message shared by two types
   (`Remove = String` on both `Map` and `Set`) is one type by the same
   rule; each receiver contributes its own command for it.
