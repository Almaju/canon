# Effects Are Values

Canon has no effect annotations, no permission system, and no `IO`
monad. Effects fall out of an ordinary fact:
**you must hold the value to do the thing**, and the only way to get
the value is to construct it.

Constructing the `File` *is* opening it. You cannot read something that
is not a `File`, and a `File` comes from nowhere but a `Path` — the
construction chain is the access control. A function that performs an
effect takes the effectful value as an input, so its signature already
declares its effects; there is no dependency injection, because a
`Database` either arrives as an argument or the function cannot exist.

Effects can also *produce* proof. If a write returns a `Written`, a
function taking `(Written)` demands evidence the write happened before
it will run — "do A before B", with no ordering machinery.

The discipline survives compilation: the finished component's only
powers are the WASI interfaces its host chooses to satisfy.

```canon
Unit => Program {
    Path("./data.json")
        -> File?
        -> Read?
        -> Print
}
```
