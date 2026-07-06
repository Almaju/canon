# args-echo

The canonical CLI entry shape, `Args => Exit`: the command's argument
vector flows in as `Args` (`List<String>`, bound by the compiler from
`wasi:cli/environment`), and an exit status flows out as `Exit`
(`= Int`) — the mirror of the HTTP world's `Request => Response`.

```canon
Args => Exit {
    Args
        -> Length
        -> Print
    Args
        -> Json
        -> Print
    0 -> Exit
}
```

Run it, forwarding arguments after the target:

```sh
canon run examples/args-echo one two three
# 3
# [one,two,three]
```

With no arguments the vector is empty (`0` and `[]`). `Exit(0)` reports
success (process exit 0); any nonzero `Exit` reports failure (exit 1).
For an exact nonzero exit code, use `Exited(n)`.
