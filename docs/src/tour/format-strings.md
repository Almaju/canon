# Strings With Holes

A backtick string is a **format string**: each `{…}` hole holds any
expression, which is converted with `-> String` and spliced in. Write
`{{` and `}}` for literal braces.

An ordinary `"…"` string has no holes. It is inert text, always — which
is why a plain quote is safe to paste user data into and a backtick is
the one place interpolation can happen.

The same hole belongs to HTML literals, where it escapes what it
splices, so `<p>{Name}</p>` cannot inject markup. Interpolation is
construction all the way down.

```canon,run
Unit => Program {
    `two plus three is {2 -> Sum(3)}` -> Print
    `{Uppercased("ada")} says {{hello}}` -> Print
}
```
