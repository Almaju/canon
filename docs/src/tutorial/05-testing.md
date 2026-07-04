# Testing the API

The refactor in the last chapter did more than tidy up: it made the
interesting logic pure. `render` takes a `Note` and returns a `String`,
with nothing about HTTP anywhere near it. Pure functions are what
Canon's test framework wants.

## A Test File

Add `src/note_test.can`:

```canon
use Note
use canon/std/TestResult

testRenderWrapsTitle = () -> TestResult {
    Note("ship it")
        .render()
        .eq({"title":"ship it"})
        .assert("render should wrap the title in a JSON object")
}
```

Run it:

```sh
$ canon test notes-api/src/note_test.can
running 1 test(s) from notes-api/src/note_test.can
[ ok ] testRenderWrapsTitle
```

A test is **any function with the signature `() -> TestResult`**.
Discovery is by type, not by name: the same signature-driven selection
that picks the program's entry point. The test file is an ordinary
module: `use Note` imports the real `note.can` sitting next to it, so
the test exercises the code the server runs, not a copy.

## Anatomy of the Assertion

```canon
TestResult = Fail + Pass

assert = (Bool * String) -> TestResult
```

`TestResult` is a stdlib union (`Fail` carries a message, `Pass` is
empty), and `assert` converts a `Bool` into one. The chain reads
top-to-bottom: render the note, compare with `.eq` (giving a `Bool`),
convert with `.assert`. When the bool is `False`, the message surfaces:

```sh
[FAIL] testRenderWrapsTitle: render should wrap the title in a JSON object
```

The process exits `1`: `canon test` is honest to shells, so wiring it
into CI is a one-liner.

## Keep the Entry Thin

You can't call `serve` from a test: it returns `Response`, a world
type, and the language offers no way to construct a fake `Request`.
That is by design, and it points at the architecture the entry-point
rule has been nudging toward all along:

- **helpers** hold the logic, take values, return values → *test these*;
- **the entry** routes and wraps → keep it too thin to get wrong.

If a branch of `serve` feels like it needs a test, extract the branch's
work into a helper that returns a `Body` or a `String`, test the helper,
and let `serve` stay a table of routes.

One more chapter: [turning this into an artifact you can deploy
anywhere](./06-ship-it.md).
