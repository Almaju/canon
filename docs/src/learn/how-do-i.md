# How Do I…?

The whole language as a lookup table: the idiom, plus the page that
explains it.

## …print something?

```canon
"hello" -> Print
```

## …name an intermediate value?

There are no variables — give the value a type. Declare a newtype
(`Subtotal = Int`) and construct it mid-pipe:
`price -> Product(quantity) -> Subtotal`.
([Types & Values](./types-and-values.md))

## …branch on a condition?

Dispatch — on the `Bool`, or better, on your own union so the cases
have names ([Branching & Loops](./branching-and-loops.md)):

```canon
count -> Gt(0) -> (
    * False => Unit { "empty" -> Print }
    * True => Unit { "has items" -> Print }
)
```

## …match on a string or number?

Literal dispatch, catch-all last:

```canon
Route -> (
    * "/health" => Body { Ok() }
    * String => Body { NotFound() }
)
```

## …loop over a list?

```canon
List(1 * 2 * 3) -> Mapped((Int) => Int { Int -> Product(2) })
```

For everything else: recursion, with dispatch as the base case
([Branching & Loops](./branching-and-loops.md)).

## …build a string from pieces?

A format string, or `Joined` for two pieces:

```canon
`{Count} items, total {Total}`
`hello, {Name}`
```

## …parse a number?

```canon
Int("42")?
```

Conversion is construction; the fallible direction returns a `Result`.

## …handle an error / an absent value?

Propagate with `?`, or dispatch on the `Result` / `Option`
([Errors & Options](./errors-and-options.md)).

## …compare two values?

`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` — one spelling for every comparable
type: `price -> Le(limit)`. (There is no `lte`/`gte`.)

## …read or write a file?

```canon
Path("./notes.txt") -> File? -> Read? -> Print
Contents("hi") -> Written(Path("/tmp/out.txt"))?
```

([Capabilities](./capabilities.md); [stdlib](../reference/stdlib.md))

## …get the time, or a random number?

`Now()` (RFC 3339 wall clock), `Instant()` (monotonic nanoseconds),
`Random()` (CSPRNG integer).

## …make an HTTP request?

```canon
Url("http://example.com")? -> Fetched? -> Print
```

## …serve HTTP?

Declare one arrow returning `Response`; routing is literal dispatch on
the path. ([Programs & Modules](./programs-and-modules.md); worked
example: [notes-api](../examples/notes-api.md))

## …produce or consume JSON?

Literals are first-class expressions with `{…}` interpolation holes;
read back with `Field` and `Decoded` ([stdlib](../reference/stdlib.md)):

```canon
{"id":1,"title":"ship canon v1"}
Json("[1, 2, 3]")?
Encoded(42)
```

## …render HTML, or build a web page?

HTML literals produce `Html` (holes escape strings, pass `Html`
through). A whole browser app is three arrows — view, init, update.
([The Web Target](../reference/web-target.md); worked example:
[todo list](../examples/todolist.md))

## …render Markdown?

```canon
Markdown("# hi") -> Html
```

Referencing `Intro` loads a sibling `intro.md` as a `Markdown` value at
compile time. ([Markdown](../reference/markdown-renderer.md))

## …keep a key-value store or a set?

`Map` and `Set` — sorted, immutable, pure Canon:

```canon
Map() -> Inserted("a" * "1") -> Value("a")?
Set() -> Added("x") -> Contains("x")
```

## …write a test?

A newtype of `TestResult` plus its nullary constructor; run with
`canon test`. ([Testing](./testing.md))

## …call a host / WASI API?

Drop the WIT file under `wit/`, run `canon install`, reference the
generated constructor by the type it produces.
([Using WASI Interfaces](../reference/wasi.md))

## …format my code?

```sh
canon check --fix
```

Non-canonical formatting is a compile error, so this is also how you
fix ordering errors — never by hand.
([The canon CLI](../getting-started/building-and-running.md))

## …start a project?

A directory with `src/main.can` is a package; there is nothing else to
set up.
