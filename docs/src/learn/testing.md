# Testing

The test framework is one union type and a CLI verb. No attributes, no
macros, no runner configuration — a test is a type, like everything
else in Canon:

```canon
SumAddsOperands = TestResult

Unit => SumAddsOperands {
    1
        -> Sum(2)
        -> Eq(3)
        -> TestResult
}
```

Declare a newtype of `TestResult` named for the behaviour it asserts,
give it a nullary constructor, and `canon test` discovers it by shape.
The name is the test's identity *and* its failure label — reported as
`[ ok ] SumAddsOperands` or `[FAIL] SumAddsOperands`.

## Assertions Are a Constructor

`TestResult = Fail + Pass`, and piping a `Bool` into it is the
assertion: `True` becomes `Pass`, `False` becomes an empty `Fail` — the
test's name already says what failed. When a diagnostic genuinely
helps, construct `Fail("why")` directly in a dispatch arm.

Because `TestResult` is an ordinary union, you can watch the mechanism
work right here:

```canon,run=learn-testing
DoublingWorks = TestResult

Unit => DoublingWorks {
    21
        -> Product(2)
        -> Eq(42)
        -> TestResult
}

Unit => Program {
    DoublingWorks() -> (
        * Fail => Unit { "[FAIL] DoublingWorks" -> Print }
        * Pass => Unit { "[ ok ] DoublingWorks" -> Print }
    )
}
```

(That dispatch is what `canon test` synthesizes for you, one arm pair
per discovered test.)

## Running

```sh
canon test math_test.can    # one file
canon test tests/           # every *_test.can under a directory
```

The exit code is honest — `0` when everything passes, `1` on any
failure — so `canon test` drops straight into CI. And the design
nudges architecture the right way: logic lives in constructors that
take and return values, so the testable surface falls out for free — a
test is just one more caller.

**Next:** [How Do I…?](./how-do-i.md) — the whole language as a lookup
table.
