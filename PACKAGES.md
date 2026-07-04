# Package Management — Design (RFC)

Status: **proposal — slices 1–3, 7, 8a implemented; design amended
(Jul 2026)**. Slices 1–3 landed as originally specified (the `deps/`
search root, the `package` directive, registry-backed
`canon install <ns>:<pkg>[@ver]`, and `canon publish`). The amendment
removes packaging directives from the language in favor of
**path-carried identity** (`deps/<ns>/<name>@<ver>/`): the `package`
keyword is **deleted** (slice 7, implemented), binding files are
recognized by **shape** (body-less declarations) with their URN derived
from the path (slice 8a, implemented), and the planned `component`
directive becomes a tool-written **`.component` file** (slice 5). The
`bindings` keyword survives *only* as the escape hatch for URNs no
path can spell — deleting it too is slice 8b, blocked on choosing that
escape hatch's replacement spelling (see Open questions). This
document describes the amended design throughout; when accepted it
**supersedes DESIGN.md § Package Manifests** (`canon.toml` is deleted
entirely) and amends § Imports and § Binding Files. It assumes the
removal of the `use` keyword: a reference to a type `User` that is not
defined in the current file resolves by convention to `user.can`.
Package management below is that same rule with one more search
location — nothing else.

This document is a peer of `STREAMING.md` / `WEB-TARGET.md`: design
first, then implementation slices at the end.

---

## The design in one paragraph

There is no manifest, no lockfile, no registry protocol of our own —
and no packaging keywords in the language. `canon install acme:http`
fetches a package from a standard OCI registry (the substrate the
WebAssembly component ecosystem has standardized on) and **vendors its
Canon source into the project tree** under `deps/acme/http@1.2.3/`.
The directory name is the pin: the bytes on disk plus the path they
live at say everything a manifest ever said, a fresh clone builds
offline, and dependency updates are reviewed the way everything else in
this repository is reviewed — `git diff`. `canon publish
acme:http@1.3.0` pushes the current directory's source (plus, when the
package builds to one, its compiled component) back to the registry.
Identity is a property of *publication*, supplied at the command line
like a git tag — source never states its own name, not even vendored
source. Everything derivable lives under a gitignored build directory
and can be deleted at any time.

## The principle: no file states a fact about itself

Canon's stance is that source is the documentation and the compiler
removes discretion. The `use`-removal committed the language to "the
name is the file": type `User` lives in `user.can`. Package management
extends the same rule upward, uniformly:

> **The file stem names the type or interface. The directory names the
> package. `@` names the version.**

One naming rule from a local type all the way to the registry.
Wherever a file would state a fact about itself — its package, its
version, its binding target — the fact moves into where the file
lives or what shape it has. Two statements that must agree are one
statement too many; the original directive design needed a checker
rule verifying that all files of a vendored package carried the same
`package` line, which is the tell that the information lived in the
wrong place. Under path-carried identity a package has exactly one
directory name, so a half-updated vendor directory is not a detectable
state but an **unrepresentable** one.

## Why `canon.toml` dies

A manifest file is a second language whose every field turns out to be
either derivable from source or a fact a tool recorded:

| `canon.toml` field | Replacement |
|---|---|
| `[deps]` | Deleted. A dependency **is** its vendored files under `deps/`. Presence on disk is the declaration; the exact version is in the directory name (`deps/acme/http@1.2.3/`). |
| `[imports]` | Deleted. A binding file is recognized by its shape (body-less declarations) and bound to the WIT interface its path spells. |
| `from` / `sha256` | A `.component` file in the vendored package directory (see below). The binary lives in the content-addressed cache, never in the repository. |
| `name` / `version` | Deleted from disk. Supplied to `canon publish` at the command line; recorded by the registry. |
| `[workspace]` | Deleted. Directories are directories. |
| project-root marker | The entry file. Resolution is rooted at the directory of the file handed to `canon run` / `canon build` / `canon check`. |

DESIGN.md was already halfway here: "the hash is computed by `canon
install` … humans don't type it", and "the lockfile is the manifest".
This design takes the last step: humans don't type *any* of it, so the
file does not exist — and neither does the directive.

## Substrate: OCI registries and the component ecosystem

The WebAssembly ecosystem converged on **OCI registries** as its
package distribution layer (the CNCF Wasm OCI Artifact format; the
earlier bespoke Warg protocol is no longer developed). The Bytecode
Alliance ships `wasm-pkg-tools`: the `wkg` CLI plus the
`wasm-pkg-client` Rust crate for fetching/publishing to OCI registries,
with `namespace:name@version` package naming — the same URN grammar
Canon's paths spell.

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

On disk a package occupies `deps/<namespace>/<name>@<version>/` — the
colon becomes a path separator and the `@` comes along verbatim (legal
on every filesystem; it is how Go's module cache and pnpm's store spell
the same fact). The full URN is reconstructed by reading the path
backwards, mirroring how binding files already lay out
`wasi:random/random` as `wasi/random/random.can`.

## Identity lives in the path

The directory name carries what the `package` directive used to say.
A vendored file:

```
deps/acme/http@1.2.3/get.can
```

```ow
get = (Url) -> Result<Response, HttpError> {
    …
}
```

Pure Canon source, no header. Rules:

- The layout is written by `canon install`; rewritten (renamed) by
  `canon update`. Never arranged by hand — though hand-vendoring a
  package into the same shape is not forbidden; the path grammar is
  the whole contract.
- Version agreement across a package's files is structural: a
  directory has one name. There is nothing for `canon check` to
  verify and no directive to misplace — a file in the project tree
  proper carries no version because there is nowhere to write one.
- **Two versions of one package on disk are an error.** Sibling
  directories `http@1.2.3/` and `http@1.3.0/` under the same
  namespace fail `canon check` naming both (at most one version of a
  package per project, as before — previously structural, now a
  trivially detected sibling scan, traded for the stronger guarantee
  that a package can never half-update).
- `canon update` shows in `git diff` as a directory rename plus
  content changes — a better review surface than N identical
  one-line edits.

This is deliberately *not* a lockfile: there is no separate file whose
contents can drift from what actually compiles. The pin and the code
are the same bytes — and the same path.

## Binding files are recognized by shape

A binding file is a file whose function declarations have no bodies.
That fact is visible in the grammar, not in a header — so the
`bindings` directive deletes too. The vendored WIT interface:

```
deps/wasi/random@0.3.0-rc-2026-03-15/random.can
```

```ow
getRandomBytes = (Int) -> List<Int>
getRandomU64 = () -> Int
```

The binding target is the path read backwards through the mechanical
mapping bindgen already uses forwards: directory →
`wasi:random@0.3.0-rc-2026-03-15`, file stem → interface `random`,
each declaration → `#<kebab-case name>` (`getRandomU64` →
`get-random-u64`; snake_case file stems un-kebab the same way —
`monotonic_clock.can` → `monotonic-clock`). The compiler verifies each
signature against the WIT interface when the package is loaded, as
before.

Disambiguation is by case and location, both already load-bearing in
the language:

- A **camelCase** name bound to a function type with no body, in a
  file under `deps/`, is an extern binding.
- A **PascalCase** name bound to a function type is a type alias (a
  callback type), everywhere — unchanged.
- A camelCase body-less declaration **outside** `deps/` is what it is
  today: a plain function-type alias, untouched. (Before the
  amendment only an active `bindings` header distinguished these two
  readings; location now does.)

An escape hatch remains necessary for bindings that defy the path
convention — a bespoke host interface (`canon:builtins/*` in the
stdlib's hand-written wrappers), and one-shot renames where the Canon
name can't kebab back to the WIT name (`ToJson = (Bool) -> Json` binds
`#from-bool`; four `ToJson` overloads can't all derive distinct WIT
names). Today that escape hatch **is** the `bindings` directive — the
original amendment text claimed the legacy per-function
`extern Wasm("<urn>#<fn>")` annotation "survives" for this, but that
syntax has since been deleted from the grammar entirely; there is
nothing to fall back to. So the directive stays, demoted from
"canonical form on every generated file" to "escape hatch on the
files whose URN no path can spell": `canon install` no longer emits
it (the loader re-derives path-spellable URNs), and it appears only
in hand-written host-bridge wrappers and test fixtures. Deleting
`KwBindings` is slice 8b, gated on designing its per-function
replacement (see Open questions).

With `package` gone and `bindings` reduced to the escape hatch, a
vendored binding file and an ordinary Canon file are grammatically
identical: `KwPackage` has left the lexer, and the loader's rewrite
keys on shape and path, with a directive overriding the path-derived
base only where one is written.

## The `.component` file

Some dependencies are not Canon source — they are compiled components
(any language) whose exports Canon calls through generated bindings.
Vendoring a binary into git is not acceptable, so the package
directory pins it by digest instead — the one fact that is neither
derivable from source nor spellable in a path:

```
deps/acme/image-decoder@1.0.0/
  .component          # sha256:ab12cd34…  (one line, tool-written)
  decoder.can         # body-less declarations, pure source
```

`.component` is a *record*, not a manifest. The discipline that keeps
it one:

- **Content is exactly one OCI digest string** — `sha256:<hex>` plus a
  trailing newline. No keys, no sections, no second line. The moment
  it has two fields it is a manifest again.
- **Written by `canon install`, read by `canon build`, never
  hand-authored.** Hand edits are not forbidden (same posture as
  hand-editing vendored source); they show as drift in `git diff`.
- **Exists iff the package is a binary component.** WIT-only packages
  (`wasi:*` — the host satisfies the imports at run time) and
  Canon-source packages don't get one. Its presence is itself
  information: "this package's implementation is not in this repo."
- Any future fact that genuinely needs recording argues for its own
  file on its own merits; nothing is ever appended to `.component`.

The name is settled (was an open question): `.component`, not
`component.sha256` or `.sha256`. The filename says what fact the file
records — the package's component — not how the content is encoded;
the encoding is the content's first word (`sha256:`). And the dotfile
correctly reads as "tool-owned, not source": the review surface for
the security-critical digest is `git diff`, which shows dotfiles like
anything else.

The `.wasm` itself lives in the global content-addressed cache
(`~/.canon/cache/<sha256>.wasm`), populated by `canon install`, shared
across projects. `canon build` with a cache miss is a hard error
naming the exact `canon install` invocation that repairs it — no
implicit network at build time (unchanged from DESIGN.md). At build
time the component is composed into the output artifact as a nested
instance (the `wac plug` role, built in — unchanged from DESIGN.md).

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
  main.can                  # entry point; its directory roots resolution
  notes.can                 # project source, resolved by the auto-import rule
  deps/                     # vendored dependency source — COMMITTED
    acme/http@1.2.3/*.can   #   pure Canon source; the path is the pin
    wasi/random@0.3.0/*.can #   body-less binding files; URN = the path
    acme/image-decoder@1.0.0/
      .component            #   sha256 digest of the cached binary
      *.can                 #   body-less binding files
  .canon/                   # build directory — GITIGNORED, disposable
    out/                    #   compiled artifacts (today's <stem>.wasm etc.)
    index.toml              #   derived: file → URN map (today's _install.toml)
~/.canon/cache/             # global content-addressed component cache
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
canon install acme:http           # latest release → deps/acme/http@<ver>/
canon install acme:http@1.2       # newest 1.2.x
canon install wasi:random@0.3     # WIT package → bindgen → deps/wasi/random@<ver>/
canon install                     # no args: re-fetch everything deps/ pins
                                  #   (fresh-clone cache repair for binary deps;
                                  #   otherwise a no-op since deps/ is committed)

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
   own dependency list (recorded at publish time — see `canon
   publish`). Install resolves the full closure before writing
   anything.
4. **Version selection**: at most one version of a package per project.
   Within a compatible range, minimal version selection (Go-style):
   the smallest version satisfying every requirement in the closure —
   deterministic, no solver, no surprise upgrades. Incompatible
   requirements (two majors) fail the install with both requirers
   named.
5. **Collision check**: the flat type namespace across the closure
   (plus the project tree and `canon:std`) is verified *now*. A
   collision fails the install naming both packages and the type.
6. Write `deps/<ns>/<name>@<ver>/` for every package in the closure:
   Canon source, pure of directives; WIT packages additionally run
   through bindgen (body-less binding files); binary components get
   binding files plus a `.component` digest and the `.wasm` goes to
   the global cache. Replacing a version removes the old versioned
   directory in the same operation.

Steps 4–6 make install the moment every cross-package invariant is
checked. Build and check never need the network and never re-litigate
resolution — they just read files.

### `canon update`

`canon install` with a widened constraint: re-resolve to the newest
version compatible with each vendored major (or the explicit argument),
rewrite `deps/` (directory renames carry the version bumps), re-run the
closure and collision checks. The diff is the review surface.

### `canon publish`

1. The package is the `.can` files under the current directory,
   excluding `deps/` and derived trees (`bindgen/`, `.canon/`,
   `target/`, hidden dirs). No manifest to read — the argument supplies
   `namespace:name@version`; bare `namespace:name` patch-bumps the
   registry's latest, or starts at `0.1.0` (mirroring this repository's
   own auto-release convention).
2. The publisher's `deps/` directory names are read and recorded
   **inside the artifact** (see the format note below) — the
   machine-written dependency list consumers' step 3 uses. Humans
   still author nothing.
3. Preflight: every file must parse and be canonically formatted; when
   the package has a `main.can` entry the full checker runs too, and a
   package that doesn't check is refused. (Pure libraries have no entry
   point to check from; their errors surface in consumers, per
   DESIGN.md's dead-code stance.)
4. **Artifact format** (implementation refinement over the original
   layers-plus-annotations sketch): a registry release is exactly one
   wasm blob on every backend, so a Canon source package publishes as a
   minimal wasm module whose custom sections carry the coordinate
   (`canon:package`), the dependency list (`canon:deps`), and one
   `canon:src/<rel-path>` section per source file. One digest-verified
   artifact, identical semantics on OCI and `local` registries, dep
   metadata in-band instead of in OCI annotations (which the `local`
   backend has nowhere to store). `canon install` recognizes the
   `canon:package` section and vendors the embedded source; artifacts
   without it (WIT packages) take the bindgen path. When entry-point
   packages learn to attach their compiled component, the same custom
   sections ride on the real component instead of an empty module —
   one artifact serves both Canon consumers and the wider ecosystem.
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
- The `package` directive and `KwPackage` — implemented in slice 1,
  **deleted by slice 7 (done)**. With it went the deps-only placement
  rule, the per-package agreement check, and the
  `package_directive_outside_deps` fixture: all states the directive
  could get wrong are unrepresentable under path-carried identity.
- The `bindings` directive on generated files — **done (slice 8a)**:
  binding files are recognized by shape and bound by path;
  `canon install` emits no header. The directive itself and
  `KwBindings` remain as the escape hatch for path-unspellable URNs
  (hand-written `canon:builtins/*` wrappers, one-shot `#fn` renames)
  until slice 8b designs its per-function replacement — the previously
  named candidate, `extern Wasm("<urn>#<fn>")`, no longer exists in
  the grammar. DESIGN.md § Binding Files amends when 8b lands.
- The separate `bindgen/` output directory in user projects — bindings
  are just vendored packages under `deps/` now. (`packages/canon/std`'s
  committed `bindgen/` tree is a compiler-internal build detail and
  migrates to the same versioned-path layout in slice 8b.)
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

**Provenance directives in source files** (the original shape of this
RFC: `package "acme:http@1.2.3"` and `bindings "<urn>"` headers on
vendored files). Rejected by the amendment: nearly the entire payload
of both directives duplicated the file's path — a file stating
metadata about itself, against this design's own "source never states
its own name" rule — and the duplication needed checker rules
(deps-only placement, per-package agreement) to police states that
path-carried identity makes unrepresentable. Two keywords whose only
non-redundant content was a version string, better spelled once in the
directory name.

**A single unified directive** (`from "<urn>@<ver>"` covering both the
provenance and binding cases). Rejected: dominated by path-carried
identity — one keyword with two meanings is muddier than either, and
it still duplicates the path.

**Committing the binary `.wasm` into `deps/`** (the "bytes are the
pin" purist endpoint, deleting `.component` too). Rejected for now:
undiffable megabyte blobs in git history forever, against
digest-in-sidecar's near-equivalent offline story (one `canon install`
repairs a fresh clone's cache, with a hard error naming exactly that
command). Recorded here so the trade is explicit if it is ever
revisited.

**Deno-style URLs in source.** Not available: with `use` removed there
is no import site to carry a URL — and per-reference versioning was the
part of Deno's design Deno itself walked back. The Deno *goal* (no
manifest, self-describing source) is achieved instead by the path the
vendored files live at.

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
- **Two versions of one package become representable on disk** (two
  `@`-suffixed siblings), where the unversioned layout made that
  impossible. Traded knowingly: detection is a trivial sibling scan,
  and in exchange version agreement *within* a package becomes
  structural instead of checked.
- **A file copied out of `deps/` carries no provenance.** Already true
  under the directive design, which forbade the directive outside
  `deps/` and so forced stripping it on copy. No actual loss.
- **Versioned directory names churn paths on update.** Renames plus
  edits are noisier in some diff views than in-place edits. Cosmetic;
  Go's module cache has spelled versions in paths for years.
- **Version-range *intent* is not recorded.** "Stay on 1.x" lives in
  how you invoke `canon update`, not in a file. If this ever hurts, the
  intent gets its own tool-written record on its own merits — still no
  manifest, and nothing appended to `.component`.
- **`wasm-pkg-client` dependency weight** (OCI client, reqwest,
  rustls). Accepted over hand-rolling an OCI client on the existing
  hyper stack; it is the ecosystem-blessed, Bytecode Alliance path and
  only the CLI's install/publish paths pay for it.

## Implementation slices

Each slice is a self-contained PR that keeps `cargo test` green.
Slices 1–3 are **implemented** (as originally specified, with
directives); slices 7 and 8a (**implemented**) migrated them onto the
amended design; 8b interlocks with 6 and waits on the escape-hatch
decision.

| Slice | Contents | Proof |
|---|---|---|
| **0. This RFC** | `PACKAGES.md`; no code. | Review. |
| **1. `deps/` + `package` directive** — *implemented; directive deleted by slice 7* | Loader gains the `deps/` search root; parser accepts the `package` directive; checker enforces deps-only placement and per-package agreement; ambiguity errors. | Checker fixtures (`package_directive_*.can`), runtime fixture with a hand-vendored `deps/`. |
| **2. Registry fetch for WIT packages** — *implemented* | `canon install <ns>:<pkg>[@ver]` fetches a WIT package via `wasm-pkg-client` and lands bindgen output under `deps/`; namespace config; content cache. | Integration test against a local OCI registry fixture (or a vendored artifact file driven through the same code path). |
| **3. `canon publish`** — *implemented* | Source-carrying artifact; recorded dep list; component layer when an entry point exists; auth via credential helpers; patch-bump default. | Round-trip test: publish to a temp/local registry, install into a fresh project, run. |
| **4. Closure + MVS + collision check** | Transitive resolution from recorded dep lists; minimal version selection; install-time flat-namespace verification. | Fixtures with conflicting/diamond closures asserting exact error text. |
| **5. Binary component deps** | `.component` digest file; global cache; build-time composition of the nested instance. | Runtime test calling into a vendored non-Canon component. |
| **6. Delete `canon.toml`** | Migrate `packages/canon/std` and `examples/`; remove manifest parsing from the loader path; update DESIGN.md (§ Package Manifests replaced by a pointer here). | The tree contains no `canon.toml`; full suite green. |
| **7. Path-carried identity** — *implemented* | `canon install` writes `deps/<ns>/<name>@<ver>/` (replacing any prior version) and stamps no directive; loader derives identity from the path; the placement/agreement checks and their fixtures are deleted; unversioned-dir, malformed-version, and two-siblings errors; `KwPackage` left the lexer; `tests/deps/` fixtures migrated; publish records deps from directory names. | `tests/deps_test.rs` (versioned fixtures incl. the two-siblings error); registry install/publish suites; grep shows no `KwPackage`. |
| **8a. Shape-recognized bindings** — *implemented* | Loader binds body-less camelCase declarations in files directly under `deps/<ns>/<name>@<ver>/` to the path-derived URN (PascalCase stays a type alias there — only *directive* bases rewrite PascalCase, so vendored callback types can't be hijacked); `canon install` stops emitting the `bindings` header whenever the URN is path-derivable, keeping it otherwise (the escape hatch working as designed). | `tests/deps/ok_bindings` (hand-vendored, header-free, runs against the host builtin); `registry_install_test` asserts header-free vendored output that checks cleanly. |
| **8b. Delete `KwBindings`** | Blocked on the escape-hatch decision (see Open questions). Then: migrate the stdlib's hand-written `canon:builtins/*` wrappers and test fixtures to the replacement; migrate `packages/canon/std/bindgen` to the versioned-path layout (interlocks with slice 6's manifest removal); delete the keyword. | Existing binding-consuming runtime fixtures stay green over the migrated layout; grep shows no `KwBindings`. |

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
- **The escape-hatch spelling (blocks slice 8b).** Path-derivation
  covers every generated binding, but hand-written host-bridge
  wrappers and one-shot renames need a per-function URN annotation,
  and the grammar currently has exactly one way to write it: the
  `bindings` directive. Deleting `KwBindings` means choosing its
  replacement — resurrecting `extern Wasm("<urn>#<fn>")` re-adds two
  keywords to delete one; an annotation form (`min = (Int * Int) ->
  Int @ "canon:builtins/math@0.1.0#min"`?) is new grammar of its own;
  relocating the stdlib's host bridges under a deps-shaped path only
  works for base URNs, not for overloaded one-shot renames. A file-
  level directive whose every remaining occurrence is irreducible
  information may also just be the honest resting point, mirroring
  `.component`. Needs a decision before 8b; nothing else blocks on it.
