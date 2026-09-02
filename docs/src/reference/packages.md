# Packages

The prelude — the bundled `canon` package the [Standard
Library](./stdlib.md) page describes — is what every program sees by
name. Everything else is a package: vendored under `deps/` by
[`canon add`](../getting-started/building-and-running.md#add-a-dependency),
reached by name like anything else once it is there, and invisible
until then. Packages are pure Canon over the prelude and independent
of one another; `canon add` alone lists the ones that ship with the
toolchain. This page is the prose for each.

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
        -> Classed(Class("card"))
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
themselves. `Classed(Class(…))` adds a class to any element's opening
tag. The [web target](./web-target.md#events)'s wiring is built in:
`Button` and `Form` take the `Msg` they send, `Input` takes one to
report its value.

## `canon/markdown` — Markdown to HTML

```sh
canon add canon/markdown
```

`Markdown -> Html` renders a document in pure Canon — the renderer this
site is built with. It has its own page: [Markdown](./markdown-renderer.md).
