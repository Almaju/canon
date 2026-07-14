# How Do I…?

The whole language as a lookup table. Each entry is the idiom plus the
page that explains it.

## …print something?

```canon
"hello" -> Print
```

Works on strings and numbers; no capability token needed.

## …name an intermediate value?

There are no variables — give the value a type instead. Declare a
newtype (`Subtotal = Int`) and construct it mid-pipe:
`price -> Product(quantity) -> Subtotal`. See
[Types & Values](./types-and-values.md).

## …branch on a condition?

Dispatch on the `Bool` (or better: on your own union, so the cases have
names):

```canon
count -> Gt(0) -> (
    * False => Unit { "empty" -> Print }
    * True => Unit { "has items" -> Print }
)
```

See [Branching & Loops](./branching-and-loops.md).

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

For anything a collection operation doesn't cover, use recursion with
dispatch as the base case — see
[Branching & Loops](./branching-and-loops.md).

## …build a string from pieces?

A format string, or `Joined` for two pieces:

```canon
`{Count} items, total {Total}`
"hello, " -> Joined(Name)
```

## …parse a number?

```canon
Int("42")?
```

Conversion is construction, and fallible conversion returns a
`Result` — see [Errors & Options](./errors-and-options.md).

## …handle an error / an absent value?

Propagate with `?`, or handle with dispatch on the `Result` /
`Option`. See [Errors & Options](./errors-and-options.md).

## …compare two values?

`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` — one spelling for every comparable
type: `price -> Le(limit)`. (There is no `lte`/`gte`.)

## …read or write a file?

```canon
Path("./notes.txt") -> File? -> Read? -> Print
Contents("hi") -> Written(Path("/tmp/out.txt"))?
```

See [Capabilities](./capabilities.md) and the
[standard library](../reference/stdlib.md).

## …get the time, or a random number?

`Now()` (RFC 3339 wall clock), `Instant()` (monotonic nanoseconds),
`Random()` (CSPRNG integer). All in the
[standard library](../reference/stdlib.md).

## …make an HTTP request?

```canon
Url("http://example.com")? -> Fetched? -> Print
```

## …serve HTTP?

Declare one arrow returning `Response`; routing is literal dispatch on
the path. See [Programs & Modules](./programs-and-modules.md) and the
worked [notes-api example](../examples/notes-api.md).

## …produce or consume JSON?

JSON literals are first-class expressions; `{…}` holes interpolate via
`ToJson`. Read fields back with `Field` and `Decoded`:

```canon
{"id":1,"title":"ship canon v1"}
Json("[1, 2, 3]")?
ToJson(42)
```

See the [standard library](../reference/stdlib.md).

## …render HTML, or build a web page?

HTML literals produce `Html` values (holes escape strings, pass `Html`
through). A whole browser app is three arrows — view, init, update. See
[The Web Target](../reference/web-target.md) and the
[todo list example](../examples/todolist.md).

## …render Markdown?

```canon
Markdown("# hi") -> Html
```

Referencing `Intro` loads a sibling `intro.md` as a `Markdown` value at
compile time. See [Markdown](../reference/markdown-renderer.md).

## …keep a key-value store or a set?

`Map` and `Set` — sorted, immutable, pure Canon:

```canon
Map() -> Inserted("a" * "1") -> Value("a")?
Set() -> Added("x") -> Contains("x")
```

## …write a test?

A newtype of `TestResult` plus its nullary constructor; run with
`canon test`. See [Testing](./testing.md).

## …call a host / WASI API?

Drop the WIT file under `wit/`, run `canon install`, reference the
generated constructor by the type it produces. See
[Using WASI Interfaces](../reference/wasi.md).

## …format my code?

```sh
canon check --fix
```

Non-canonical formatting is a compile error, so this is also how you
*fix* ordering errors — never by hand. All commands:
[The canon CLI](../getting-started/building-and-running.md).

## …start a project?

A directory with `src/main.can` is a package; there is nothing else to
set up. See [Programs & Modules](./programs-and-modules.md).
