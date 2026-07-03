# Package Management — Design (RFC)

Status: **proposal — not yet implemented**. This document defines how
Canon fetches, resolves, and publishes packages. When accepted it
**supersedes DESIGN.md § Package Manifests** (`canon.toml` is deleted
entirely) and amends § Imports. It assumes the in-flight removal of the
`use` keyword: a reference to a type `User` that is not defined in the
current file resolves by convention to `user.can`. Package management
below is that same rule with one more search location — nothing else.

This document is a peer of `STREAMING.md` / `WEB-TARGET.md`: design
first, then implementation slices at the end.

---

## The design in one paragraph

There is no manifest, no lockfile, and no registry protocol of our own.
`canon install acme:http` fetches a package from a standard OCI
registry (the substrate the WebAssembly component ecosystem has
standardized on) and **vendors its Canon source into the project tree**
under `deps/acme/http/`, each file stamped with a one-line provenance
directive (`package "acme:http@1.2.3"`). The bytes on disk are the pin:
a fresh clone builds offline, and dependency updates are reviewed the
way everything else in this repository is reviewed — `git diff`.
`canon publish acme:http@1.3.0` pushes the current directory's source
(plus, when the package builds to one, its compiled component) back to
the registry. Identity is a property of *publication*, supplied at the
command line like a git tag — source never states its own name.
Everything derivable lives under a gitignored build directory and can
be deleted at any time.

## Why `canon.toml` dies

Canon's stance is that source is the documentation and the compiler
removes discretion. A manifest file is a second language whose every
field turns out to be either derivable from source or a fact a tool
recorded:

| `canon.toml` field | Replacement |
|---|---|
| `[deps]` | Deleted. A dependency **is** its vendored files under `deps/`. Presence on disk is the declaration; the exact version is in each file's `package` directive. |
| `[imports]` | Deleted. Already solved by the `bindings "<urn>@<version>"` directive that every generated binding file carries. |
| `from` / `sha256` | A `component` directive in the vendored binding file (see below). The binary lives in the content-addressed cache, never in the repository. |
| `name` / `version` | Deleted from disk. Supplied to `canon publish` at the command line; recorded by the registry. |
| `[workspace]` | Deleted. Directories are directories. |
| project-root marker | The entry file. Resolution is rooted at the directory of the file handed to `canon run` / `canon build` / `canon check`. |

DESIGN.md was already halfway here: "the hash is computed by `canon
install` … humans don't type it", and "the lockfile is the manifest".
This design takes the last step: humans don't type *any* of it, so the
file does not exist.

## Substrate: OCI registries and the component ecosystem

The WebAssembly ecosystem converged on **OCI registries** as its
package distribution layer (the CNCF Wasm OCI Artifact format; the
earlier bespoke Warg protocol is no longer developed). The Bytecode
Alliance ships `wasm-pkg-tools`: the `wkg` CLI plus the
`wasm-pkg-client` Rust crate for fetching/publishing to OCI registries,
with `namespace:name@version` package naming — the same URN grammar
Canon's `bindings` directives already use.

Consequences we inherit for free:

- **No registry infrastructure.** ghcr.io, Docker Hub, or any OCI
  registry hosts Canon packages from day one. Auth rides on Docker
  credential helpers.
- **Content addressing.** Every fetch is digest-verified by the OCI
  protocol itself — DESIGN.md's "no fetch and trust" rule, natively.
- **Bidirectional interop.** A published Canon package carries a
  compiled component alongside its source, so it is consumable from
  Rust/JS/Python/Go via standard tooling; and any WIT or component
  package anyone publishes (all of `wasi:*` included) is consumable
  from Canon via the existing bindgen.
- **`wasm-pkg-client` is a Bytecode Alliance crate**, inside the
  dependency orbit this project allows. (It does pull an HTTP client —
  reqwest + rustls — transitively; accepted, see Trade-offs.)

## Package identity

A package name is `namespace:name` (OCI/wkg convention), versions are
semver: `acme:http@1.2.3`. The namespace is a publisher-scoped prefix
(an org, a person); registries control who may publish to a namespace.
Canon's own packages publish under `canon:` (`canon:std`). The `wasi:`
namespace resolves to the upstream WASI WIT packages.

On disk a package occupies `deps/<namespace>/<name>/` — the colon
becomes a path separator, mirroring how binding files already lay out
`wasi:random/random` as `wasi/random/random.can`.

## The `package` directive

The `bindings` directive generalized. Every vendored source file begins
with one line naming the package and exact version it came from:

```ow
package "acme:http@1.2.3"

get = (Url) -> Result<Response, HttpError> {
    …
}
```

Rules:

- Written by `canon install`; rewritten by `canon update`. Never typed
  by hand.
- Appears **only** in files under `deps/`. A file in the project tree
  proper must not carry one (checker error) — your own code has no
  version, publication gives it one.
- All files of one vendored package carry the same directive; `canon
  check` verifies agreement (a half-updated vendor directory is a
  detectable state, not a mystery).
- It is one line of source, `canon fmt`-stable, alphabetized with
  nothing (it precedes all declarations, like `bindings`).

This is deliberately *not* a lockfile: there is no separate file whose
contents can drift from what actually compiles. The pin and the code
are the same bytes.

## The `component` directive

Some dependencies are not Canon source — they are compiled components
(any language) whose exports Canon calls through generated bindings.
Vendoring a binary into git is not acceptable, so the binding files pin
it by digest instead:

```ow
package "acme:image-decoder@1.0.0"
bindings "acme:image-decoder/decoder@1.0.0"
component "sha256:ab12cd34…"

decode = (Bytes) -> Result<Image, DecodeError>
```

- The `.wasm` itself lives in the global content-addressed cache
  (`~/.canon/cache/<sha256>.wasm`), populated by `canon install`,
  shared across projects.
- `canon build` with a cache miss is a hard error naming the exact
  `canon install` invocation that repairs it. No implicit network at
  build time (unchanged from DESIGN.md).
- At build time the component is composed into the output artifact as a
  nested instance (the `wac plug` role, built in — unchanged from
  DESIGN.md).

WIT-only packages (`wasi:*`) are the degenerate case: `bindings`
directive, no `component` (the host satisfies the imports at run time).
This is exactly what today's `canon install` emits into `bindgen/`,
relocated to `deps/` and stamped with `package`.

## Resolution: one rule, one new search location

Given a reference to type `Z` not defined in the current file
(post-`use`-removal, name→file):

1. Resolve within the project tree (the auto-import rule, whatever its
   final scoping — this RFC does not define it).
2. Else resolve against `deps/**/z.can`.
3. Else resolve against the bundled `canon:std` sources.
4. Else: compile error. The error suggests `canon install`, and — only
   when the user asks (`canon install --suggest Z` or an explicit
   flag) — consults the registry index to name candidate packages. The
   compiler itself never touches the network.

**Ambiguity is a hard error, not a precedence.** If `Z` resolves in
more than one location — two packages, a package and a local file, a
package and the stdlib — compilation fails naming every candidate.
There is no shadowing. The consequence is stated plainly:

> **Type names are globally unique across a project's dependency
> closure.**

This is a radical constraint and a deliberate one, of a kind with
no-comments and no-locals. The `use`-removal already committed the
language to a flat name→file namespace locally; this extends the same
bet to packages. Package authors under flat namespaces name types
distinctively (the pressure is healthy); collisions surface at
**install time** (see below), not as downstream mysteries. If an
escape hatch is ever genuinely needed, an install-time rename that
wraps a package's surface in newtypes can be designed later; it is
explicitly out of scope now.

## Directory layout

```
my-app/
  main.can              # entry point; its directory roots resolution
  notes.can             # project source, resolved by the auto-import rule
  deps/                 # vendored dependency source — COMMITTED
    acme/http/*.can     #   each file: package "acme:http@1.2.3"
    wasi/random/*.can   #   each file: package + bindings directives
  .canon/               # build directory — GITIGNORED, disposable
    out/                #   compiled artifacts (today's <stem>.wasm etc.)
    index.toml          #   derived: file → URN map (today's _install.toml)
~/.canon/cache/         # global content-addressed component cache
  <sha256>.wasm
```

`deps/` is source: committed, readable, greppable, formatted by `canon
fmt`, reviewed by `git diff`. `.canon/` is cache: derived, deletable,
never authoritative. `src/` does not exist as a concept — project files
sit wherever the author puts them, as the `examples/` tree already
demonstrates.

Hand-editing files under `deps/` is not forbidden (they are just
source) but any edit shows as drift in `git diff` against what `canon
update` would regenerate — the same posture as the committed
`bindgen/` tree today.

## The commands

```sh
canon install acme:http           # latest release → deps/acme/http/
canon install acme:http@1.2       # newest 1.2.x
canon install wasi:random@0.3     # WIT package → bindgen → deps/wasi/random/
canon install                     # no args: re-fetch everything deps/ pins
                                  #   (fresh-clone repair; normally a no-op
                                  #   since deps/ is committed)

canon update                      # every dep → newest semver-compatible
canon update acme:http            # one dep → newest compatible
canon update acme:http@2          # explicit major crossing

canon publish acme:http@1.3.0     # push cwd's package to the registry
canon publish acme:http           # patch-bump over the registry's latest
```

### `canon install`

1. Resolve `namespace:name[@constraint]` against the registry
   (namespace → registry mapping, see Configuration).
2. Fetch the artifact; OCI verifies the digest.
3. **Walk the transitive closure.** A published artifact carries its
   own dependency list as OCI annotations (recorded at publish time —
   see `canon publish`). Install resolves the full closure before
   writing anything.
4. **Version selection**: at most one version of a package per project.
   Within a compatible range, minimal version selection (Go-style):
   the smallest version satisfying every requirement in the closure —
   deterministic, no solver, no surprise upgrades. Incompatible
   requirements (two majors) fail the install with both requirers
   named.
5. **Collision check**: the flat type namespace across the closure
   (plus the project tree and `canon:std`) is verified *now*. A
   collision fails the install naming both packages and the type.
6. Write `deps/<ns>/<name>/` for every package in the closure: Canon
   source stamped with `package` directives; WIT packages additionally
   run through bindgen (`bindings` directive); binary components get
   binding files with `component` digests and the `.wasm` goes to the
   global cache.

Steps 4–6 make install the moment every cross-package invariant is
checked. Build and check never need the network and never re-litigate
resolution — they just read files.

### `canon update`

`canon install` with a widened constraint: re-resolve to the newest
version compatible with each vendored major (or the explicit argument),
rewrite `deps/`, re-run the closure and collision checks. The diff is
the review surface.

### `canon publish`

1. The package is the `.can` files under the current directory,
   excluding `deps/` and `.canon/`. No manifest to read — the argument
   supplies `namespace:name@version`; bare `namespace:name` patch-bumps
   the registry's latest (mirroring this repository's own auto-release
   convention).
2. The publisher's `deps/` directives are read and recorded as OCI
   annotations — the machine-written dependency list consumers' step 3
   uses. Humans still author nothing.
3. `canon check` must pass; publishing a package that doesn't check is
   refused.
4. Layers pushed: **source** (a custom media type carrying the `.can`
   tree — the layer Canon consumers use) and, when the package has an
   entry point, the **compiled component** (standard Wasm OCI media
   type — the layer the rest of the ecosystem uses). Pure libraries
   publish source-only.
5. Auth: Docker credential helpers, via `wasm-pkg-client`. `canon
   publish` to a namespace you can't write to fails with the registry's
   error.

## Configuration

Per-user, never per-project (a project directory stays manifest-free):
the namespace→registry mapping reuses the `wasm-pkg` config file
(`$XDG_CONFIG_HOME/wasm-pkg/config.toml`) so `canon` and `wkg` see the
same world — `wasi:` resolves to the upstream WASI registry out of the
box, and a default registry (e.g. ghcr.io) catches the rest. Zero
config for consumers; publishers log in once with their registry
credentials.

## What this deletes

- `canon.toml` — the file, DESIGN.md § Package Manifests, and
  eventually `src/manifest.rs` (the parser survives only as long as
  the migration needs it).
- The separate `bindgen/` output directory in user projects — bindings
  are just vendored packages under `deps/` now. (`packages/canon/std`'s
  committed `bindgen/` tree is a compiler-internal build detail and
  migrates on its own schedule.)
- `_install.toml` as a committed artifact — its content becomes the
  derived index under `.canon/`.
- The `[workspace]` concept, `from`/`sha256` manifest fields, and the
  project-root-marker role of the manifest.
- Any future `canon.lock` — explicitly rejected, see below.

## Rejected alternatives

**A manifest in Canon syntax** (`package.can` with declarations, Zig's
`build.zig` direction). Rejected: it grows the language (Canon has no
top-level data-literal form and would need one solely for metadata) and
it relocates discretion instead of removing it — config-as-code invites
logic into configuration, the opposite of Canon's stance.

**A lockfile** (`canon.lock`, go.sum, package-lock.json). Rejected: a
lockfile is a second statement of truth that can drift from the source
it describes and still needs the network to be honored on a fresh
clone. Vendored source is a strictly stronger pin: the clone *contains*
the dependency. The only cost is diff volume, which Canon's small-
surface packages keep proportionate.

**Deno-style URLs in source.** Not available: with `use` removed there
is no import site to carry a URL — and per-reference versioning was the
part of Deno's design Deno itself walked back. The Deno *goal* (no
manifest, self-describing source) is achieved instead by the `package`
directive on vendored files.

**Warg or a bespoke registry protocol.** The ecosystem consolidated on
OCI; Warg is unmaintained. Riding OCI means zero infrastructure and
interop with every component publisher, including `wasi:*` itself.

## Trade-offs accepted

- **Dependency updates are diffs in your repository.** Framed as a
  feature: `git diff` is already this project's review surface for
  goldens; now it reviews supply-chain changes too. Large closures
  would make this heavy; Canon's ethos of tiny packages is load-bearing
  here.
- **Flat type namespace across the closure.** Two packages exporting
  the same type name cannot coexist in one project. Surfaced at
  install time; escape hatch deliberately deferred.
- **Version-range *intent* is not recorded.** "Stay on 1.x" lives in
  how you invoke `canon update`, not in a file. If this ever hurts, the
  constraint joins the `package` directive
  (`package "acme:http@1.2.3" from "1.x"`) — still in-file, still no
  manifest.
- **`wasm-pkg-client` dependency weight** (OCI client, reqwest,
  rustls). Accepted over hand-rolling an OCI client on the existing
  hyper stack; it is the ecosystem-blessed, Bytecode Alliance path and
  only the CLI's install/publish paths pay for it.

## Implementation slices

Each slice is a self-contained PR that keeps `cargo test` green. The
first two are independent of the `use` removal (the `deps/` search root
works under today's `use` resolution exactly as `bindgen/` does);
slices 4+ interlock with it.

| Slice | Contents | Proof |
|---|---|---|
| **0. This RFC** | `PACKAGES.md`; no code. | Review. |
| **1. `deps/` + `package` directive** | Loader gains the `deps/` search root (beside today's `bindgen/` lookup); parser accepts the `package` directive (mirror of `bindings`); checker enforces deps-only placement and per-package agreement; ambiguity errors. | Checker fixtures (`package_directive_*.can`), runtime fixture with a hand-vendored `deps/`. |
| **2. Registry fetch for WIT packages** | `canon install <ns>:<pkg>[@ver]` fetches a WIT package via `wasm-pkg-client` and lands today's bindgen output under `deps/` with directives; namespace config; content cache. Existing local-path install keeps working during migration. | Integration test against a local OCI registry fixture (or a vendored artifact file driven through the same code path). |
| **3. `canon publish`** | Source-layer media type; annotation-recorded dep list; component layer when an entry point exists; auth via credential helpers; patch-bump default. | Round-trip test: publish to a temp/local registry, install into a fresh project, run. |
| **4. Closure + MVS + collision check** | Transitive resolution from annotations; minimal version selection; install-time flat-namespace verification. | Fixtures with conflicting/diamond closures asserting exact error text. |
| **5. Binary component deps** | `component` directive; global cache; build-time composition of the nested instance. | Runtime test calling into a vendored non-Canon component. |
| **6. Delete `canon.toml`** | Migrate `packages/canon/std` and `examples/`; remove manifest parsing from the loader path; update DESIGN.md (§ Package Manifests replaced by a pointer here). | The tree contains no `canon.toml`; full suite green. |

## Open questions

- **Search scoping for step 1 of resolution** is owned by the
  `use`-removal design (same directory? whole project tree?). This RFC
  only appends `deps/` and `canon:std` after whatever it decides.
- **Should `canon publish` verify reproducibility** (rebuild the
  component from the source layer and compare digests) before pushing?
  Cheap honesty; deferred to slice 3 review.
- **Namespace registration UX** — who may publish `acme:*` is a
  registry concern (ghcr.io: org membership). Whether Canon wants a
  blessed default registry with its own namespace policy is a
  community question, not a compiler one.
