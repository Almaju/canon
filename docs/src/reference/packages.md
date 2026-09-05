# Packages

The prelude — the bundled `canon` package the [Standard
Library](./stdlib.md) page describes — is what every program sees by
name. Everything else is a package: vendored under `deps/` by
[`canon add`](../getting-started/building-and-running.md#add-a-dependency),
reached by name like anything else once it is there, and invisible
until then. Packages are pure Canon over the prelude and independent
of one another; `canon add` alone lists the ones that ship with the
toolchain. This page is the prose for each; the generated [API
reference](api/index.html) is the declaration surface of every one.

---

## `canon/ansi` — terminal styling

```sh
canon add canon/ansi
```

```canon
Unit => Program {
    "ok"
        -> Styled(Green())
        -> Print
    "careful"
        -> Styled(Yellow())
        -> Styled(Bold())
        -> Print
}
```

`Style` is the union of the eight colors (`Black` … `White`) and the
attributes `Bold`, `Dim`, `Italic`, `Underline`; `text -> Styled(style)`
wraps the text in the SGR escape sequence and its reset. Styles
compose by nesting, as above.

## `canon/encoding` — Base64 and hex

```sh
canon add canon/encoding
```

```canon
Unit => Result<Program, MalformedBase64> {
    Base64Encoded("Canon") -> Print
    Base64("Q2Fub24=")
        -> Base64Decoded?
        -> Print
    HexEncoded("Canon") -> Print
    Unit() -> Ok
}
```

`Base64Encoded` / `HexEncoded` encode a string's bytes — RFC 4648
base64 with padding, lowercase hex octets — in pure Canon. Decoding is
the validating direction: tag the received text (`Base64(s)` /
`Hex(s)`) and pipe `-> Base64Decoded?` / `-> HexDecoded?`; bad length,
characters outside the alphabet, or padding before the end are the
module's `MalformedBase64` / `MalformedHex` error. Uppercase hex
digits decode fine; encoding always emits lowercase.

## `canon/prng` — seeded pseudo-random numbers

```sh
canon add canon/prng
```

```canon
Unit => Program {
    Seed("any text")
        -> Next
        -> Below(Bound(6))
        -> Sum(1)
        -> Print
}
```

Randomness without a host: the prelude's `Random()` draws from
`wasi:random`, which the browser and the HTTP handler world cannot
import, and a game or a test wants a reply it can replay. `Seed` is
an `Int`; `String -> Seed` hashes text into one, `seed -> Next` is
the next state of a linear congruential generator (31-bit, so the
arithmetic never overflows), and `seed -> Below(Bound(n))` reads a
number in `0 … n-1` from its high bits. The same seed always gives
the same sequence — `examples/tic-tac-toe` seeds the computer's move
from the board itself.

## `canon/router` — HTTP request routing

```sh
canon add canon/router
```

```canon
Request => Response {
    Request.path() -> (
        * None { NotFound() }
        * Some<String> {
            String -> Segments -> First -> (
                * None { Location("/notes") -> Redirect }
                * Some<String> {
                    String -> (
                        * "notes" { {"notes":[]} -> JsonResponse }
                        * String { NotFound() }
                    )
                }
            )
        }
    )
}
```

Pure Canon over the prelude's handler surface. Reading the request
target: `-> Segments` is the path's segments as a `List<String>` (the
query dropped, empty segments skipped), so a route is dispatch on
`First` and its parameters are `At(2)`, `At(3)`, …; `-> Query` is the
text after `?`, and `query -> Param(Key("id"))` looks a parameter up
(`Option<Param>`, no percent-decoding). Building the response:
`HtmlResponse`, `JsonResponse`, and `TextResponse` are a 200 with the
matching `content-type`; `Status(422) -> TextResponse("…")` picks the
status; `NotFound()` is a 404 and `Location("/x") -> Redirect` a 303.
Each is `= Response`, so a helper that computes one is an ordinary
constructor (`RoutePath => Routed`, see `examples/notes-api`).

## `canon/ui` — HTML elements

```sh
canon add canon/ui
```

```canon
Card = Html

String => Card {
    H2("Hello")
        -> Joined(String -> Escaped -> P)
        -> Joined(Href("/more") -> A("read more"))
        -> Div
        -> Class("card")
}

Unit => Program {
    Card("a <b>user</b> wrote this") -> Print
}
```

One constructor per element, each `= Html` and named after its tag:
`A`, `Button`, `Code`, `Div`, `Em`, `Form`, `H1`, `H2`, `H3`, `Img`,
`Input`, `Li`, `Ol`, `P`, `Pre`, `Span`, `Strong`, `Table`, `Td`, `Th`,
`Tr`, `Ul`, and `El` / `ElAttr` for any other tag. Content is inserted
as written — pipe user text through the prelude's `Escaped` first; the
attribute newtypes (`Href`, `Src`, `Alt`, `Placeholder`) escape
themselves. `Class` is a message: `-> Class("card")` adds a class to
any element's opening tag. The [web target](./web-target.md#events)'s wiring is built in:
`Button` and `Form` take the `Msg` they send, `Input` takes one to
report its value.

## `canon/markdown` — Markdown to HTML

```sh
canon add canon/markdown
```

`Markdown -> Html` renders a document in pure Canon — the renderer this
site is built with. It has its own page: [Markdown](./markdown-renderer.md).
