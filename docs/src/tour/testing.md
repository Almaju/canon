# Testing

The test framework is a union type, one helper function, and a CLI verb.
There are no attributes, no test macros, no runner configuration.

## `TestResult`

A test is any function with the signature `() -> TestResult`:

```canon
testAddPositive = () -> TestResult {
    1
        .add(2)
        .eq(3)
        .TestResult("1 + 2 should be 3")
}
```

`TestResult` and its constructor are ordinary stdlib definitions,
written in pure Canon:

```canon
Fail = String

Pass = Unit

TestResult = Fail + Pass

(Bool * String) -> TestResult
```

The `TestResult` constructor turns a `Bool` and a message into a
`TestResult`: `True` becomes `Pass()`, `False` becomes `Fail(message)`
(the assertion *is* construction).  The message only appears when
the assertion fails.

## Running Tests

```sh
$ canon test note_test.can
running 1 test(s) from note_test.can
[ ok ] testRenderWrapsTitle
```

`canon test` discovers every `() -> TestResult` function in the file.
Discovery is **by type signature**, not by name; the `test` prefix is a
convention, not a requirement. This is the entry-point rule again: a
function returning `Response` makes the program an HTTP service, a
function returning `TestResult` makes it a test.

A failing test prints its message and fails the process:

```
[FAIL] testRenderWrapsTitle: render should wrap the title in a JSON object
```

The exit code is honest: `0` when everything passes, `1` when anything
fails. `canon test` slots directly into CI and shell scripts.

## The Shape of a Test

Without local variables, a test is a single chain ending in a
`TestResult`, typically `.eq(expected).TestResult(message)`:

```canon
Note = String

render = (Note) -> String {
    "{\"title\":\""
        .concat(Note)
        .concat("\"}")
}

testRenderWrapsTitle = () -> TestResult {
    Note("ship it")
        .render()
        .eq({"title":"ship it"})
        .TestResult("render should wrap the title in a JSON object")
}
```

One test, one assertion. This is a constraint today (`?` doesn't yet
short-circuit across multiple assertions in a test body), but it is also
a decent discipline: a test that asserts one thing has one reason to
fail.

## What to Test

Only the entry point may return a world type (`Unit` for CLI, `Response`
for HTTP), so everything else in a program is a pure function over
values, and pure functions are trivially testable. Put logic in helpers
that take values and return values, keep the entry thin, and the testable
surface falls out for free. The [tutorial's testing
chapter](../tutorial/05-testing.md) walks through this on a real API.
