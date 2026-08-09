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

```text
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
Config = Json

File => Result<Config, IoError + MalformedJson> {
    File
        -> Read?
        -> Json?
        -> Config
        -> Ok
}
```

## Dispatch

Dispatch is the language's only branching construct. The scrutinee (a
union value) **pipes into** the arm group with `->`; the arms are its
handlers:

```canon
Ord = Equal
  + Greater
  + Less

Sign = Negative
  + Positive
  + Zero

Ord => Sign {
    Ord -> (
        * Equal => Sign { Zero() }
        * Greater => Sign { Positive() }
        * Less => Sign { Negative() }
    )
}
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
Message = String

Outcome = Result<String, IoError>

Outcome => Message {
    Outcome -> (
        * Err<IoError> => Message { IoError }
        * Ok<String> => Message { String }
    )
}
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
String => String {
    String -> (
        * "/notes" => String { "index" }
        * "/notes/1" => String { "note one" }
        * String => String { `not found: {String}` }
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
(see the [notes-api example](../examples/notes-api.md)).

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

## Literals With Holes

Three literal forms carry interpolation — the backtick **format
string**, the **JSON** literal, and the **HTML** literal. They share one
rule set:

- A hole `{...}` holds an arbitrary Canon expression, converted through
  the family named for the target: `-> String` in a format string,
  `-> Encoded` (`Encoded = Json`) in JSON, `-> Escaped` (`Escaped = Html`)
  in HTML. A hole is a piped construction; interpolation is construction
  all the way down.
- `{{` and `}}` escape literal braces.
- Literal holes (`{42}`, `{"a"}`) fold to static text at parse time, so
  an all-constant literal costs one string constant at runtime.
- **The literal is the only spelling of a static document.** Feeding the
  validating `Json("…")` or `Html("…")` constructor a static string the
  literal form can already express is a checker error -- the parse can
  never fail, so the constructor would be a second way to write the
  literal. Those constructors are for strings built at runtime.
- **Holes break like code.** A hole whose expression would push its line
  past the width limit opens onto its own indented lines -- the braces
  stay glued to the surrounding text, which is content and never moves:

  ```text
  `<td>{
      1 -> Inline(String)
  }</td>`
  ```

  A bare reference (`{Model}`, `{Node.Rest}`) never breaks, and a hole
  inside an indented HTML literal indents from the markup around it, not
  from the code margin.

### Format Strings

Backticks are the opt-in: an ordinary double-quoted string stays inert,
where `{` is just a brace.

```canon
Greeting = String

Report = String

String => Greeting {
    `hello, {String}!`
}

Int => Report {
    `count is {Int}, doubled {Int -> Product(2)}`
}
```

A hole converts through `String` construction -- an `Int` renders as
decimal digits, a `String` passes through. This replaces hand-written
`Joined` chains, and the replacement is enforced: `canon check --fix`
folds a `Joined` chain containing literal text into a format string
(`"<" -> Joined(x) -> Joined(">")` becomes `` `<{x}>` ``). An
all-computed chain keeps the pipe -- `Joined` is also list
concatenation, and only literal text proves the chain builds a string.
A backtick string with no holes is rewritten to the plain-quoted form.
``\` `` escapes a backtick; `\n` / `\t` / `\\` / `\u….` work as in a
double-quoted string, and a format string may span source lines.

Unlike `Json` and `Html`, a format string needs no prelude -- `String`
construction is intrinsic -- so it works in every world, including
`wasi:http/service` handlers.

### JSON Literals

Object and array literals produce `Json` values. The compiler knows
`Json = String` intrinsically and the loader pulls in `canon/std/Json`
the moment a program uses its machinery (interpolation, the validating
constructor, or the `Encoded` family).

```canon
Label = Json

Int => Label {
    {"answer":Int,"doubled":Int -> Product(2),"ok":True()}
}
```

Static members (strings, numbers, `true`/`false`/`null`, nested static
literals) bake into a constant at parse time, so a fully static literal
imposes no imports and works in every world. Layout is canonical like
all Canon code: no spaces after `:` or `,`. The `Encoded` family's
`Float` member is host-backed (`canon:builtins/json`), which the HTTP
world cannot satisfy yet -- an interpolating handler fails at build with
an error naming the unsatisfiable import.

### HTML Literals

An HTML literal starts at a `<` immediately followed by a lowercase tag
name -- a position where `<` is never valid Canon, since generic
arguments are PascalCase types -- and spans one root element, closing
tag included. Everything that is not a hole (attributes, quotes, nested
tags, comments, void elements) is raw markup.

```canon
Model = Int

Model => Html {
    <div>
        <h1>Counter</h1>
        <button data-msg="Increment">+</button>
        <span>{Model -> String}</span>
    </div>
}
```

The `Escaped` member selected by a hole's type escapes a `String` or
`Int` and passes an `Html` value through unchanged, so composing
literals never double-escapes:

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

Holes work in attribute values too (`<button data-msg="{Msg}">`). The
stdlib's tag-newtype constructors (`Button`, `Div`, … `= Html`) are for
markup *computed* from values; static structure belongs in the literal.
`Html` is a prelude type (`Html = String` intrinsically), and HTML
literals power the [web target](../reference/web-target.md)'s `view`.

## Operator and Sigil Glossary

| Symbol | Meaning |
|---|---|
| `+` | union (sum) |
| `*` | product (type-level and value-level) |
| `T^N` | fixed repetition |
| `T^*` | unbounded repetition (Kleene star) |
| `<T>` | generic parameter |
| `.` | field access — reads a component (dot-*calls* survive only for camelCase FFI bindings) |
| `-> ( )` | dispatch: pipe the scrutinee into an arm group |
| `?` | propagate `Result` / `Option` failure |
| `"..."` | string literal |
| `` `...{expr}...` `` | format string (interpolating) |
| `{"k":v}` / `[v]` | JSON literal |
| `<tag>...</tag>` | HTML literal |
