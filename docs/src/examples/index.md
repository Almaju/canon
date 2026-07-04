# Examples

Every program in this section lives in the repository's
[`examples/`](https://github.com/Almaju/canon/tree/main/examples)
directory and is exercised by `just examples`: real, running programs,
not illustrative pseudo-code. Each page shows the complete source, what
it prints, and which language ideas it demonstrates.

Examples exist to show **real-world usage**: I/O, networking, things
with environment-dependent output. Small deterministic feature
demonstrations live in the repository's `tests/runtime/` fixtures
instead, where CI pins their exact output.

| Example | What it shows | Page |
|---|---|---|
| `clock`, `now`, `random`, `exit-code` | WASI capabilities as one-line constructors; honest exit codes | [CLI Basics](./cli-basics.md) |
| `read-file` | file I/O via the `Path → File → String` chain | [Reading a File](./read-file.md) |
| `fetch-url` | HTTP client via a validated `Url` constructor | [Fetching a URL](./fetch-url.md) |
| `multifile` | modules: one type per file, `use` imports | [A Multi-File Project](./multifile.md) |
| `notes-api` | a JSON API as a `wasi:http/service` component | [notes-api](./notes-api.md) |

## Running Them

From a checkout of the repository:

```sh
canon run examples/random          # any single example
just examples                       # compile + run all, report pass/fail
```

Every example is an ordinary package, a `canon.toml` plus
`src/main.can`, so each one doubles as a template for starting your own
project.
