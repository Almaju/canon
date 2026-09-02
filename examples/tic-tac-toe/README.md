# tic-tac-toe

A browser game, and the smallest program that shows the web target
working with the ecosystem packages: `canon/ui` renders the board as
buttons wired to messages, and `canon/prng` gives the computer its
move — a seeded generator, because a browser-side Canon program has no
host randomness to import, and none is needed: the board is the seed.

```sh
canon run examples/tic-tac-toe
```

Open <http://127.0.0.1:8080>: click a cell to play X, the computer
answers with O at once, **Play again** resets.

## What it demonstrates

- **A game is a fold.** The whole state is a nine-character `Board`;
  a click is the message `Play:5`; `update` places the X, then the
  computer's O, and `view` redraws. No mutation anywhere.
- **Packages in the browser.** `deps/canon/ui@…` and
  `deps/canon/prng@…` are vendored with `canon add`; the compiled
  bundle imports nothing but the print stubs.
- **Deterministic randomness.** `Board -> Seed -> Next -> Below(...)`
  picks the k-th free cell — the same position gets the same reply,
  which also makes the game trivially testable.
- **Rules as constructors.** `Won`, `Over`, `Free`, `Filled` are
  `Bool`/`Int` result newtypes over the board string; the eight
  winning lines are three-digit strings read with `ByteAt`.
