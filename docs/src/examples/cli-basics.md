# CLI Basics: Time, Randomness, Exit Codes

Four programs, each about five lines, each the same pattern: **a WASI
capability surfaced as an ordinary constructor**. No setup, no context
objects. Construct the value, use it.

## `now`: Wall-Clock Time

```canon
main = () -> Unit {
    Now().print()
}
```

```sh
$ canon run examples/now
2026-07-03T03:35:45Z
```

`Now = String` is a newtype whose constructor reads the wall clock and
formats RFC 3339. Because it is a `String` underneath, `.print()` is
inherited straight through the alias.

## `clock`: Monotonic Time

```canon
main = () -> Unit {
    Instant().print()
}
```

`Instant = Int`: nanoseconds from the monotonic clock
(`wasi:clocks/monotonic-clock`). It is an `Int` newtype, so arithmetic
works: `Instant().sub(start)` is a duration. `canon/std/time/Unix`
provides wall-clock Unix seconds.

## `random`: A Random Integer

```canon
main = () -> Unit {
    Random().print()
}
```

`Random()` draws from the host CSPRNG via `wasi:random/random`. The
reverse is impossible: nothing conjures randomness without constructing
a `Random`. This is the [effects story](../tour/effects.md) in
miniature.

## `exit-code`: Honest Process Exits

```canon
main = () -> Unit {
    "exiting cleanly".print()
    Exit(0).exit()
}
```

`Exit = Int`; `.exit()` terminates the process with that code, riding
the real `wasi:cli/exit` interface. Try `Exit(3)` and check `echo $?`.
Canon programs are shell-scriptable and CI-safe; `canon test` uses the
same mechanism to fail builds.

## The Common Shape

All four are the same sentence with a different noun: construct the
domain value, transform it, print it. Each capability's signature is
documented in the [Standard Library reference](../reference/stdlib.md).
