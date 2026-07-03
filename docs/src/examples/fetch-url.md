# Fetching a URL

[`examples/fetch-url`](https://github.com/Almaju/canon/tree/main/examples/fetch-url)
— an HTTP GET in one chain, built on a **validated constructor**.

```canon
use canon/std/http/HttpError
use canon/std/http/Url

main = () -> Unit {
    Url("http://example.com")?
        .get()?
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
Url = (String) -> Result<Url, InvalidUrl>
get = (Url) -> Result<String, HttpError>
```

The first `?` handles *parse* failure: `Url` declares its own
constructor, so `Url("not a url")` doesn't give you a `Url` — it gives
you a `Result` you must unwrap. Malformed URLs are unrepresentable
downstream of this line; `get` never has to re-validate its input.

The second `?` handles *network* failure. Different failure, different
error type, same operator.

This is the pattern the [Errors chapter](../tour/errors.md) formalizes:
fallibility lives in the type, and the inline error union
(`InvalidUrl + HttpError`) composes at the signature when a caller
propagates both.

## Status

`.get()` is currently a blocking GET over `http://` backed by a
temporary host bridge; TLS and the async `wasi:http/outgoing-handler`
lowering are tracked in the [Standard Library
reference](../reference/stdlib.md). Programs using it run under
`canon run` (see [Deploying](../reference/deploying.md) for the
portability boundary).
