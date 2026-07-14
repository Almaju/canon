# Expressions and Dispatch

## Precedence

Type-level, tightest first:

1. `T^N`, `T^*`: postfix repetition / Kleene star
2. `T<...>`: generic application
3. `*`: product
4. `+`: union

So `A + B * C^3` parses as `A + (B * (C^3))`.

Expression-level, tightest first:

1. `.`: field access (and FFI binding calls — the one place `.`
   still executes, because camelCase means *foreign*)
2. `()`: application
3. `?`: postfix propagation
4. `*`: value-level product (only inside a constructor argument)

So `user.Birthday.String?` is `((user.Birthday).String)?`.

## Construction vs Observation

Field names and constructors are both PascalCase, so the dot syntax
needs one disambiguation rule, and it is the parentheses:

- `user.Birthday`: **field access**, reads the `Birthday` component.
- `user.Birthday()`: **constructor call**, calls `Birthday` with `user`
  as the receiver, producing a new value.

`()` signals *producing*; its absence signals *observing*. In type
position (signatures, dispatch patterns) a bare PascalCase name is
always the type.

## Canonical Call Form

A call applies one PascalCase name to an input product. The three
spellings -- `Name(a * b)`, `a.Name(b)`, and `a -> Name(b)` -- denote the
*same* call (the receiver / left value fills the first slot of the input
product). Since the choice between them is discretionary, the compiler
picks one canonical form and `canon check --fix` rewrites the rest to it,
backstopped by the checker -- the same instrument that enforces
alphabetical ordering.

**Values flow through pipes; literals are born in the parens.**

The pipe carries a value that already exists -- a parameter reference, a
prior result. A scalar literal (string, int, float, backtick)
springs into existence at the call site, so it rides inside the call
instead of pretending to flow:

```canon
Greeting("hi")                    # not  "hi" -> Greeting
Name("toto") -> Display          # the construction flows into the next step
value -> Person(30)              # a computed value pipes; the literal rides
list -> Mapped(f)                # computed values always pipe
```

The full rule, case by case:

- **A computed first input pipes**, and the rest ride in the parens:
  `a -> Name(b)` reads as "apply `Name`, which already carries `b`, to
  `a`" -- a partial application fed the flowing value. `B(A)` with a
  computed `A` is never canonical: it becomes `A -> B`.
- **A lone scalar literal never pipes into a construction** --
  `"hi" -> Greeting` is rewritten to `Greeting("hi")`, `42 -> Show` to
  `Show(42)`. A chain then *starts* with that construction and
  *continues* with `->`: `Path("./data.json") -> File? -> Read?`.
- **Wrapping a literal in its own primitive constructor is ceremony** --
  `Int(3)`, `String("foo")`, `Float(1.5)` unwrap to the
  bare literal (which already desugars to exactly that construction),
  the same way a hole-less backtick string collapses to a plain one.
  Cross-kind construction (`String(42)` decimal rendering, `Int("42")`
  parsing) is a real conversion and stays.
- **Builtins keep the pipe** -- `Sum`, `Print`, `Joined`, and the rest of
  the compiler's builtin vocabulary are receiver-oriented machine
  operations, not constructions; they have no prefix call form, so
  `1 -> Sum(2)` and `"hello" -> Print` stay pipes. The set shrinks as
  builtins migrate to stdlib newtypes (`Maximum(3 * 5)` already
  constructs).
- **Zero-input calls stay prefix** -- `Now()`, `Map()`, `None()`.
- **`List(...)` keeps its elements** -- a list is an ordered sequence, not
  a subject-bearing call, so `List(1 * 2 * 3)` is left as written.
- **Operand order is positional and never reordered.** The pipe receiver
  is always the first operand (`0 -> Difference(5)` is -5), and literal
  operands keep their written order -- untagged same-typed components
  bind by declaration order, so reshuffling them would change which
  field gets which value. Only an all-computed input list (where every
  operand carries its type syntactically) is sorted for determinism
  before the first pipes.

Because the spellings denote the same call, every rewrite is
semantics-preserving: the compiler treats a piped call to a type
constructor exactly as the prefix construction `Name(A * rest)`.

## Function Bodies

A body is a newline-separated sequence of expressions; the **last
expression is the return value**. Non-final expressions are evaluated
and discarded; they exist for effects and for `?` propagation. With no
local variables, the way a value threads through several operations is a
method chain:

```canon
ReadConfig = (File * Path) => Result<Config, IoError + ParseError> {
    File
        -> Read(Path)?
        -> Parse?
        -> Validate
}
```

## Dispatch

Dispatch is the language's only branching construct. The scrutinee (a
union value) **pipes into** the arm group with `->`; the arms are its
handlers:

```canon
Ord -> (
    * Equal => Sign { Zero() }
    * Greater => Sign { Positive() }
    * Less => Sign { Negative() }
)
```

The `->` is the same pipe that carries a value into a constructor: the
scrutinee flows into the dispatch. The parentheses group the arms -- they
isolate the match, they do not declare arguments. (The legacy spelling
`Ord.( ... )` has been retired -- it is now a parse error; `.` no longer
executes anything.)

Rules:

- Each arm is a lambda whose single parameter is one variant type; arms
  are separated by `*`. The leading `*` on the first arm is optional.
- Arms must appear in the union's **variant order** (alphabetical), and
  every variant must be handled; there is no wildcard arm.
- Dispatch is an expression; all arms must produce the same type.

Algebraically, dispatch is the isomorphism

```
(A + B + C) -> R  ~=  (A -> R) * (B -> R) * (C -> R)
```

made literal: a sum value applied to a product of handlers.

### Payload Binding

When a variant carries data, the arm body sees the payload under a
name determined by the pattern:

- **Stdlib containers** (`Ok<T>`, `Err<E>`, `Some<T>`): write the type
  argument explicitly; it binds the *unwrapped* value.

  ```canon
  result -> (
      * Err<IoError> => String { IoError -> Message }
      * Ok<String> => String { String }
  )
  ```

- **User-defined variants** with their own definition (`Branch = Left *
  Right * Value`): write just the variant name; the matched value is in
  scope under that name, fields accessible through it.

Dispatch also follows newtype alias chains: given
`MessageContent = Option<Content>`, a `MessageContent` value dispatches
on `(None, Some<Content>)` directly.

**Shadowing.** An arm binding is an ordinary lexical binding: inside
the arm body it shadows any outer component of the same type name. A
function that already has a `String` component and dispatches over a
`Result<String, E>` sees the *payload* as `String` inside the
`Ok<String>` arm. When both values are needed in the same arm,
disambiguate the outer one with a newtype alias before dispatching,
the same rule as same-typed parameters.

### Literal Dispatch

Dispatch extends to **equality dispatch on `String` and `Int`**
scrutinees: arms are literals, and the final arm is a **mandatory
catch-all** naming the scrutinee's type:

```canon
Route = (String) => String {
    String -> (
        * "/notes" => String { "index" }
        * "/notes/1" => String { "note one" }
        * String => String { "not found: " -> Joined(String) }
    )
}
```

Rules:

- The scrutinee must be `String` or `Int`, directly or through a
  newtype alias chain (`Path = String` dispatches with a `(Path)`
  catch-all).
- Literal arms can never be exhaustive, so **totality comes from the
  catch-all**: it is required, and it is always the last arm.
- Literal arms follow canonical order (alphabetical for strings,
  ascending for ints); duplicates are a compile error. `canon check --fix`
  sorts the arms automatically.
- Inside every arm body (including literal arms) the scrutinee value is
  in scope under its type name, exactly like a bound payload.

Nested dispatch composes: dispatch on a union, then literal-dispatch
the payload inside an arm. This is the shape of every HTTP route table
(see [Serving HTTP](../guide.md#serving-http)).

## The `?` Operator

Postfix `?` propagates failure and absence:

- On `Result<T, E>`: if `Err`, the enclosing function returns the error
  immediately; if `Ok`, the expression evaluates to the unwrapped `T`.
- On `Option<T>`: if `None`, the enclosing function returns `None`;
  otherwise unwraps to `T`.

The enclosing function's return type must be able to carry the
short-circuited value (a `Result` whose error slot includes `E`, or an
`Option`). Inline error unions compose at the signature:
`Result<Unit, HttpError + InvalidUrl>` accepts short-circuits from both
`Url(...)?` and `-> Fetched?`.

**Error union widening.** Inline error unions widen along
`?`-propagation: a `Result<T, IoError>` propagates out of a function
declared `Result<U, IoError + ParseError>` without ceremony. `?` lifts
the error into the wider union whenever the callee's error variants are
a subset of the caller's. Alphabetical enforcement makes the subset
test purely syntactic: every union has exactly one canonical spelling,
so `IoError + NotFound` *is* the same type everywhere it appears.

`Option<T>` and `Result<T, E>` are deliberately distinct: `None` means
*absent*, `Err` means *failed*.

## JSON Literals

JSON object and array literals are first-class expressions producing
`Json` values. No import is required: like `Option` and `Result`, JSON
support is part of the prelude. The compiler knows `Json = String`
intrinsically, and the loader pulls in `canon/std/Json` automatically
the moment a program uses its machinery (interpolation, the validating
`Json(...)` constructor, or `.ToJson()`):

```canon
Label = (Int) => Json {
    {"answer":Int,"doubled":Int -> Product(2),"ok":True()}
}
```

- **Static** members (strings, numbers, `true`/`false`/`null`, nested
  static literals) are baked into a constant at parse time. A fully
  static literal is a constant: it imposes no imports and no host
  requirements, so it works in every world (including
  `wasi:http/service` handlers).
- **Interpolated** members are ordinary Canon expressions converted at
  runtime via their `ToJson` instance. The instances are host-backed
  (`canon:builtins/json`), which the HTTP world can't satisfy yet; a
  handler program using interpolation fails at build with an error
  naming the unsatisfiable imports.
- Literal layout is canonical like all Canon code: no spaces after `:`
  or `,` (`{"k":v}`, not `{"k": v}`). `canon check --fix` enforces it.
- **The literal is the only spelling of a static document.** Feeding the
  validating `Json("…")` constructor a static string literal the
  literal form can already express is a checker error -- the parse can
  never fail, so the constructor spelling would be a second way of
  writing the literal. `Json(String)` exists for strings built at
  runtime; scalar documents (`Json("42")` -- no literal form) and
  runtime-computed strings pass through untouched.

## HTML Literals

HTML literals are first-class expressions producing `Html` values, the
markup mirror of JSON literals. A literal starts at a `<` immediately
followed by a lowercase tag name -- a position where `<` is never valid
Canon, since generic arguments are PascalCase types -- and spans one root
element, closing tag included. `{...}` is an interpolation hole holding an
arbitrary Canon expression; everything else (attributes, quotes, nested
tags, comments, void elements like `<br>`) is raw markup:

```canon
Model => Html {
    <div>
        <h1>Counter</h1>
        <button data-msg="Increment">+</button>
        <span>{Model -> String}</span>
    </div>
}
```

Interpolated values convert through the `ToHtml` trait: a `String` or
`Int` is HTML-escaped (via the stdlib's `Escaped`), while an `Html` value
passes through unchanged, so composing literals never double-escapes:

```canon
Listing = Html

Row = Html

String => Listing {
    <ul>{String -> Row}</ul>
}

String => Row {
    <li>{String}</li>
}
```

- Holes work in attribute values too (`<button data-msg="{Msg}">`).
- Literal interpolations (`{42}`, `{"a & b"}`) fold to static text at
  parse time -- escaped where escaping applies -- so an all-constant
  literal costs one string constant at runtime, exactly like an
  all-static JSON literal.
- `{{` and `}}` escape literal braces.
- **The literal is the only spelling of static markup.** `Html("…")`
  fed a static string the literal form can already express is a checker
  error, exactly like `Json` above -- write `<button>+</button>`, not
  `Html("<button>+</button>")`. The stdlib's tag-newtype constructors
  (`Button`, `Div`, … `= Html`) are for markup *computed* from values;
  static structure belongs in the literal, dynamic composition in the
  constructors.

Like `Json`, `Html` is a prelude type (`Html = String` intrinsically);
the loader pulls in `canon/std/web/Html` the moment a literal carries a
hole or `.ToHtml()` is called. HTML literals power the
[web target](../reference/web-target.md)'s `view`.

## Format Strings

A **format string** is a backtick-delimited string literal with `{...}`
interpolation holes -- the plain-`String` mirror of the JSON and HTML
literals above. Ordinary double-quoted strings stay inert (`{` is just a
brace), so backticks are the opt-in: reach for them exactly when you
want a hole.

```canon
String => Greeting {
    `hello, {String}!`
}

Int => Report {
    `count is {Int}, doubled {Int -> Product(2)}`
}
```

- A hole holds an arbitrary Canon expression whose value is converted
  through `String` construction and concatenated into the surrounding
  text: an `Int` renders as its decimal digits, a `String` passes
  through unchanged. This replaces hand-written `-> Joined(...)` chains
  (`` `<{x}>` `` instead of `"<" -> Joined(x) -> Joined(">")`).
- `{{` and `}}` escape literal braces; ``\` `` escapes a backtick, and
  the usual `\n` / `\t` / `\\` / `\u….` escapes work as in a
  double-quoted string. A format string may span multiple source lines.
- Literal holes (`{42}`, `{"a"}`) fold to static text at parse time, so
  a fully constant backtick string costs one string constant at runtime,
  exactly like an all-static JSON or HTML literal. A backtick string
  with no holes is just a string constant: `canon check --fix` rewrites it to
  the plain-quoted form (`` `hi` `` → `"hi"`).

Unlike `Json` and `Html`, a format string needs no prelude -- `String`
construction (including the built-in int-to-string) is intrinsic -- so
interpolation works in every world, including `wasi:http/service`
handlers.

## Operator and Sigil Glossary

| Symbol | Meaning |
|---|---|
| `+` | union (sum) |
| `*` | product (type-level and value-level) |
| `T^N` | fixed repetition |
| `T^*` | unbounded repetition (Kleene star) |
| `<T>` | generic parameter |
| `<T: Tr>` | generic with trait constraint |
| `.` | field access — reads a component (dot-*calls* survive only for camelCase FFI bindings) |
| `-> ( )` | dispatch: pipe the scrutinee into an arm group |
| `?` | propagate `Result` / `Option` failure |
| `"..."` | string literal |
| `` `...{expr}...` `` | format string (interpolating) |
| `{"k":v}` / `[v]` | JSON literal |
| `<tag>...</tag>` | HTML literal |
