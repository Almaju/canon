# dice-roll

Demonstrates `Random`, the standard library's cryptographically-secure
random integer (`packages/canon/std/src/random.can`, backed by
`wasi:random/random#get-random-u64`). `Random` is a newtype over `Int`
(`Random = Int`), so it erases to `Int` at the value level and takes
ordinary arithmetic methods directly.

```canon
Unit => Program {
    Random()
        -> Remainder(6)
        -> Sum(6)
        -> Remainder(6)
        -> Sum(1)
        -> Print
}
```

`Random()` draws a fresh signed 64-bit value, positive or negative, so
a single `Remainder(6)` alone would print a number in `-5..5`. Adding
`6` before remaindering again folds that into `0..5` regardless of
sign (the same shift-then-remainder idiom `time/now.can` uses to turn
a signed remainder into a wall-clock field), and the trailing `Sum(1)`
turns it into an ordinary six-sided die roll, `1..6`.

Run it:

```sh
canon run examples/dice-roll
# 4
```

Each run prints a different value.
