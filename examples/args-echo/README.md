# args-echo

Reading the argument vector. The entry takes no input — at the ABI
level `wasi:cli/run.run` passes none — so argv is fetched, not
received: `Args()` (`= List<String>`, from `canon/std`) reads it via
`wasi:cli/environment`.

```canon
Unit => Program {
    Args()
        -> Length
        -> Print
    Args()
        -> Json
        -> Print
}
```

Run it, forwarding arguments after the target:

```sh
canon run examples/args-echo one two three
# 3
# [one,two,three]
```

With no arguments the vector is empty (`0` and `[]`). Reaching the end
of the body is success (process exit 0); an exact exit code is
`Exited(n)`.
