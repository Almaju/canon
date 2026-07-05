# Fetching a URL

[`examples/fetch-url`](https://github.com/Almaju/canon/tree/main/examples/fetch-url):
an HTTP GET in one chain, built on a **validated constructor**.

```canon
main = () -> Unit {
    Url("http://example.com")?
        -> Fetched?
        .print()
}
```

```sh
$ canon run examples/fetch-url
<!doctype html>
<html>
…
```

## Two Failure Modes, Two `?`s

```canon
(String) -> Result<Url, InvalidUrl>

(Url) -> Result<Fetched, HttpError>
```

The first `?` handles *parse* failure. `Url` declares its own
constructor, so `Url("not a url")` produces a `Result`, not a `Url`.
Malformed URLs are unrepresentable downstream of this line; the
`Fetched` constructor never has to re-validate its input.

The second `?` handles *network* failure. Different failure, different
error type, same operator.

The [Errors chapter](../tour/errors.md) formalizes the pattern:
fallibility lives in the type, and the inline error union
(`InvalidUrl + HttpError`) composes at the signature when a caller
propagates both.

## Status

`url -> Fetched` (`Fetched = Body`, the fetch-evidence constructor —
see [Types-Only Canon](../spec/types-only.md)) is currently a blocking
GET over `http://` backed by a
temporary host bridge; TLS and the async `wasi:http/outgoing-handler`
lowering are tracked in the [Standard Library
reference](../reference/stdlib.md). Programs using it run under
`canon run` (see [Deploying](../reference/deploying.md) for the
portability boundary).
