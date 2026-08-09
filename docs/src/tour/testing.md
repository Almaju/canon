# Tests Are Types

The test framework is one union type and a CLI verb — no attributes, no
macros, no runner config. Declare a newtype of `TestResult` named for
the behaviour it asserts, give it a nullary constructor, and
`canon test` finds it by shape. The name is the test's identity *and*
its failure label: `[ ok ] DoublingWorks`.

`TestResult = Fail + Pass`, so piping a `Bool` into it is the
assertion. When a diagnostic genuinely helps, construct `Fail("why")`
in a dispatch arm instead.

Because `TestResult` is an ordinary union, the program below is the
dispatch `canon test` synthesizes — one arm pair per discovered test.
Break the assertion and run it again.

```canon,run
DoublingWorks = TestResult

Unit => DoublingWorks {
    21
        -> Product(2)
        -> Eq(42)
        -> TestResult
}

Unit => Program {
    DoublingWorks() -> (
        * Fail { "[FAIL] DoublingWorks" -> Print }
        * Pass { "[ ok ] DoublingWorks" -> Print }
    )
}
```
