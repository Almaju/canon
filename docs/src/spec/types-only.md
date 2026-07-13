# Types-Only Canon

> **Status: adopted direction, migration in progress.** The rest of the specification describes the language as implemented today; this page describes where it is going and governs new design decisions. Pieces land in slices; until a slice lands, the existing form remains valid. This page was the `DESIGN.md S Types-Only Canon` section before the docs consolidation.

Canon removed local variables with the argument "names lie; types don't." The same argument now applies to the last remaining name-space: **camelCase function names are removed from the language**. Every named callable is a PascalCase declaration -- a type. What is today

```
insert = (Map * String * Value) => Map { ... }
map.insert("k", "v")
```

becomes

```
Inserted = Map

Inserted = (Map * String * Value) => Inserted { ... }

map.Inserted("k" * "v")
```

The result is a language in which **the only names are type names**. Values are already unnamed (no `let`); operations become unnamed too -- an operation is identified by *what it produces*, never by a verb. Function names lie (`nowRfc3339 = () => Now` was in the stdlib -- named after RFC 3339, returning `Now`); a constructor named after its return type is checked by the compiler. The naming treadmill (`fromX`, `intoX`, `parseX`, `tryX`) disappears because there is nothing left to name.

What survives unchanged: the dot syntax, commutative calling, dispatch, `?`, lambdas. Removing methods removes the *names*, not the calling convention -- `x.Foo(y)` still reads left to right, lambdas stay anonymous (dispatch arms and `map` arguments never had names), and `x.Foo` vs `x.Foo()` stays field-access vs construction.

### The Unified Declaration

Every declaration is `PascalName = rhs`, and the RHS shape decides the meaning:

| RHS shape | Meaning | Formerly |
|---|---|---|
| type expression (`A * B`, `A + B`, `T^N`, alias) | type definition | type definition |
| signature, no body | **shape** -- a named function type others implement | trait declaration / callback type |
| signature + body, name = return type | **constructor** -- an implementation of the type | validated constructor |
| signature + body, name = a declared shape | **shape implementation** | trait implementation |

A bodied declaration must be named after its return type or after a declared shape. Anything else is a compile error -- that error is the entire enforcement mechanism, checkable from signatures alone. (If the return type is a bare type parameter, as in `fold`, declare a shape.) *Implemented for free (receiver-less) declarations:* the checker rejects a bodied free declaration whose name is not the type it constructs (modulo `Result`/`Option`/`Future` peeling and newtype chains) -- `a bodied declaration is named after the type it constructs`.

### Anonymous Constructors -- `(A) => B { ... }` *(implemented)*

A constructor named after its return type repeats information the signature already carries (`Url = (String) => Result<Url, InvalidUrl>` spells `Url` twice). So the constructor declaration form is the **anonymous arrow** -- the typed edge itself, with no name at all:

```
String => Result<Url, InvalidUrl> { ... }        # the Url constructor
Bool => Json { ... }                             # family members are just arrows
Int => Json { ... }
Unit => Map { Empty() }                        # the empty-map constructor
Request => Response { ... }                      # an entire HTTP service
```

The constructed type is the return type with `Result`/`Option`/`Future` peeled, so fallible constructors stay anonymous too. Call sites are unchanged -- a constructor is still invoked by its output type (`Url("...")?`, `"...".Url()?`); the arrow only removes the redundant *declaration-site* name.

`Unit` is the name of "no input": a nullary constructor is `Unit => Map`, not `() => Map` -- `()` is not a declaration form. The CLI entry pairs an input world type with an output one, mirroring the HTTP handler: `Args => Exit` (`Args = List<String>`, `Exit = Int`, both from `canon/std`) is the argument-vector-in, exit-status-out shape, selected by its signature exactly as the HTTP handler `Request => Response` is. The arg-less `Unit => Program` (`Program = Unit`) remains valid for programs that read no arguments.

This is not a second function syntax -- it is the language's **only** function form, appearing at every level: top level it declares a constructor, in expression position it is a lambda, and every dispatch arm is one (`* (False) => Unit { ... }`). Named declarations (`Inserted = (Set * String) => Set { ... }`) remain for exactly one thing: shapes and their implementations, where the name carries the only information the types cannot -- which is precisely the endomorphism boundary above. The rule reads: *if the types fully determine the operation, it has no name; if they don't, the name is a shape.*

Coherence: at most one arrow per (input product, constructed type) pair -- the same family-disjointness rule, with nothing left to name a conflict after.

During migration the named form (`Url = (String) => ...`) remains legal and means the same thing; it is deprecated and will be removed with slice 6. `canon fmt` preserves whichever form is written until then.

### The Value-Level Pipe -- `value -> B` *(implemented)*

The declaration arrow has a call-site mirror: `value -> B` sends a value along a declared arrow. It is the third spelling of the commutative call -- `B(value)`, `value.B()`, and `value -> B` are the same call -- and it composes with everything postfix:

```
7 -> Greeting -> Loud.print()          # pipe chains; `.` continues on the result
"41" -> Int?.add(1)                    # a fallible arrow yields the Result; `?` is just `?`
map -> Value("k")                      # remaining components ride in parens
list -> Json                           # pipes reach methods/shapes too, by receiver type
```

`-> B?` needs no special rule: the pipe produces the `Result<B, E>` and the ordinary postfix `?` propagates it. The grammar is unambiguous because Canon has no parenthesized grouping expression -- a `(` at expression *start* is always a lambda (which consumes its own arrow), so a postfix `->` can only be a pipe. The right-hand side must be a PascalCase name; `canon fmt` breaks long pipe chains onto continuation lines exactly like `.` chains:

```
Map()
    .Inserted("a", "1")
    -> Keys
    -> Json
    .print()
```

Three spellings of one call is a migration-period surplus; which become canonical (and whether `fmt` rewrites the others) is decided at slice 6 together with the named-constructor deprecation.

### The One-Operator Endgame -- `->` executes, `.` reads, `=>` declares *(landing)*

> **Status: the declaration/execution split has landed.** `=>` declares (constructors, shapes, lambdas, dispatch arms); `->` executes (the value-level pipe). `canon fmt` writes `=>` for every declaration, and the whole tree is migrated; `->` in a declaration position is still *accepted* so mixed sources parse during the remaining work. Still ahead: retiring the `.`-method-call and `B(a)` prefix-call forms in favour of `->`, and the LSP discovery providers.

The migration collapses to a single execution operator. Three symbols, three non-overlapping jobs:

| Symbol | Job | Editor completion after it |
|---|---|---|
| `->` | **execute** -- the only call / pipe / construct / dispatch form | functions whose input product contains the left value's type |
| `.` | **read** -- field access only | the value's fields/components |
| `=>` | **declare** -- every constructor / shape / lambda / dispatch-arm definition | -- |

The point of the split: `.` and `->` stop competing to mean "call." `.` only *reads* a component; `->` *applies* a function **and** pipes a scrutinee into a dispatch (`value -> ( * ... )`). So typing `.` offers fields and typing `->` offers functions -- autocompletion on both, for different things. And `=>` vs `->` gives declaration a spelling distinct from execution: **`=>` defines a mapping, `->` flows a value through one.**

Every construct in one table:

| Migration-era form | Endgame form |
|---|---|
| `"hi".Print()` | `"hi" -> Print` |
| `String.A().B().C()` | `String -> A -> B -> C` |
| `alice.Compare(bob)` / `bob.Compare(alice)` | `alice -> Compare(bob)` / `bob -> Compare(alice)` |
| `Now()` (zero input) | `-> Now` |
| `"41".Int()?.add(1)` | `"41" -> Int? -> Add(1)` |
| declaration `(A * B) => C { ... }` | `(A * B) => C { ... }` |
| shape `Show = () => String` | `Show = () => String` |
| lambda `(Int) => Int { ... }` | `(Int) => Int { ... }` |
| dispatch arm `* (False) => Unit { ... }` | `* (False) => Unit { ... }` |
| dispatch `bool.( ... )` | `bool -> ( ... )` |
| field `user.Birthday` | unchanged |

The declaration/execution mirror is the payoff -- the same shape, `=>` when defining, `->` when running:

```
(End * Start * String) => String { ... }        # declare
string -> Substring(Start(1), End(4))          # execute
```

Commutativity is preserved: `(A * B) => C` is reachable from either side (`a -> C(b)` or `b -> C(a)`), because the pipe fills one component of the input product and the rest follow. A function stays linked to every type in its input, never bound to a single receiver.

**Two decisions gated implementation; both have landed:**

1. *Multi-input spelling* -- the **parens tail**, `a -> C(b)` (best for chaining and editor discovery -- enter from any component, editor completes the rest). `canon fmt` canonicalizes every call to this form (the `canon_expr` pass in `src/formatter.rs`): `B(a)` -> `a -> B`, `B(a * c)` -> `a -> B(c)`, `a.B(c)` -> `a -> B(c)`.
2. *Declaration arrow* -- `=>` (minimal, "maps to"). `->` at a declaration site is now a parse error (`expect_decl_arrow` in `src/parser/parser.rs`): declarations use `=>`, execution uses `->`.

Auto-discovery is the headline feature, not a side effect: `->` completion queries "functions whose input product mentions this type," `.` completion queries the value's fields. Building both in the LSP is its own slice, since it is the reason for the split.

Details still to pin: first-class function references (today `Int.Double` for passing a function as a value) collide with `.`-means-field, so those become either bare names resolved by expected type or plain lambdas; and zero-input application spelled `-> Now` (leading arrow) needs a parser rule for statement-initial `->`.

**Constructors form families.** A type may have any number of constructor implementations, distinguished by input product: `Json` has `(Bool)`, `(Float)`, `(Int)`, and `(String) => Result<Json, MalformedJson>` constructors. Coherence generalizes from traits: at most one implementation per (name, input product) pair in the whole program, checked at link time. Traits and constructor overloads thereby collapse into one concept -- *a PascalCase name is a family of implementations selected by input product* -- and the trait system above is the shape/implementation half of it.

### What Replaces Each Kind of Function

- **Conversions and creations** are already constructors (S [Conversions](#conversions)): `fromBool` -> `Json(Bool)`, `openFile` -> `File(Path)`, `create` -> `HttpServer(Port)`.
- **Accessors** construct the accessed thing: `get = (Map * String) => Option<Value>` becomes a `Value` constructor -- `map.Value("k")?` reads "the Value in this Map at this key, which might not exist." `keys` -> `Keys = List<Key>` + `Keys = (Map) => Keys`.
- **Endomorphisms** -- operations whose output type equals an input type, the one place a type genuinely underdetermines the function -- take **result newtypes**: `Inserted = Map`, `Removed = Map`, `Joined = String`. Newtype substitutability makes chaining free: `Map().Inserted("a" * "1").Removed("a")` composes because `Inserted` flows anywhere `Map` is expected. The verb's information relocates into a name that is now checked, sorted, globally resolvable, and usable in downstream signatures.
- **Effects produce evidence**: `write` becomes `Written = Path` + `Written = (Contents * Path) => Result<Written, IoError>`. A downstream function that accepts `(Written)` instead of `(Path)` *requires proof the write happened* -- capability-style sequencing with no new machinery, and the pattern composes with [capability entry points](#capability-entry-points-planned) (the capability joins the constructor's input product). Pure sinks (`-> Unit`) are shape implementations (`Print = () => Unit` -- `"hi".Print()`) until capabilities land, after which printing returns the capability (`Stdout.Print("a").Print("b")`) and sinks disappear.
- **Shared vocabulary** -- operations whose meaning spans types -- are shapes with per-type implementations, exactly what traits were built for: `Length` (`Map`, `Set`, `String`, `List`), the comparison surface `Eq`/`Lt`/`Le`/`Gt`/`Ge`, ordering via the `Ord` constructor (`a -> Ord(b) -> ( * Less => ... )` -- `compare` was already constructor-shaped). Arithmetic takes the noun vocabulary: `Sum`, `Product`, `Difference`, `Quotient`, `Remainder`, `Minimum`, `Maximum` -- `2.Sum(3)`, `price.Product(quantity)`, non-commutative cases disambiguated by `OtherInt` under the ordinary binding rule.
- **Higher-order operations** take generic result newtypes: `Mapped<U> = List<U>` + `Mapped = <T, U>(((T) => U) * List<T>) => Mapped<U>`; `Filtered<T> = List<T>`. First-class references generalize: `Int.Doubled` replaces `Int.double`. `Fold` stays a shape (its result is a bare parameter).
- **Entry points**: the world registry becomes core shapes -- the CLI entry `Args => Exit`, the HTTP handler `Request => Response`, and the web triple `Model => Html` (view) with `Unit => Init` and `Model * Msg => Update`. A program is a module implementing a world shape; selection is by declared implementation -- the CLI entry is chosen by returning `Exit` (with argv arriving as `Args`) just as the HTTP handler is chosen by returning `Response`. The web triple is type-selected too: it anchors on the view (the sole `Model => Html` with a user-type receiver), with `Init` / `Update` model-alias markers giving init and update distinct constructor keys. No world keys on a function name.
- **The FFI boundary keeps camelCase.** WIT functions are kebab-case verbs; no mechanical mapping can invent result nouns. Binding files keep the mechanical camelCase mapping and become the **only** place camelCase is legal. camelCase in a Canon program then has exactly one meaning -- *this identifier is foreign* -- the way `unsafe` marks a boundary in Rust.

### Name Resolution Under Types-Only

Moving every operation into the type namespace multiplies name pressure (`Item`, `Value`, `Key` recur across files), so the resolution rules sharpen:

1. **Duplicate declarations are legal; ambiguity is an error only at a reference site.** Two files may each declare `Item`. The hard error moves from "two files declare `Item`" to "a third file references `Item` bare while both are in scope." (No shadowing, no precedence -- the ambiguous *reference* is still a hard error.)
   **Structurally identical duplicates are one type** *(implemented)*: type equality is syntactic (alphabetical order gives every type one canonical spelling), so `Length = Int` declared by both `map.can` and `set.can` merges into a single type -- the co-declared `(Map) => Length` and `(Set) => Length` arrows are then ordinary family members selected by receiver (`map -> Length`, `set -> Length`; pinned by `tests/runtime/map_set_length.can`). Only *differing* bodies under one name clash, and `Owner.Item` qualification stays reserved for that case.
2. **Riding-along types resolve lexically, in their defining file's scope.** `set.List()` returns `List<Item>` where `Item` is set.can's `Item`, because the `List` constructor was declared there. Consumers never spell it -- Canon has no local variables, so types are only written in signatures.
3. **Qualification through the owner, when a signature genuinely must name a foreign helper type**: `Set.Item` in type position. This reuses existing syntax -- `user.Birthday` is already "the Birthday of user" -- so no module system and no new grammar. Needing it is a signal the helper leaked; newtype substitutability (`Item = String` flows into a `String` slot) usually makes the underlying type the better signature.
4. **Constructor families cross files.** `List = (Set) => List<Item>` and `List = (Map) => List<Pair>` coexist: a reference to a family name loads *all* declaring files, and the error is input-product overlap, not co-declaration. Plain type definitions do **not** get this relaxation -- two reachable `Key = String` definitions referenced bare remain an error under rule 1.

Distinct same-shaped operations stay distinct the same way same-typed parameters do: newtypes. Two different `String -> Int` operations are two result newtypes (`Length = Int`, `ByteCount = Int`) -- which is a feature: `Length` and `Age` can no longer accidentally interchange.

### Worked Example

`map.can` ported (excerpt -- see `packages/canon/std/src/map.can` once slice 5 lands):

```
Inserted = Map

Inserted = (Map * String * Value) => Inserted {
    Map.(
        * (Empty) => Inserted { Node(String, Empty(), Value) }
        * (Node) => Inserted {
            String.Lt(Node.Key).(
                * (False) => Inserted { ... Node.Rest.Inserted(String * Value) ... }
                * (True) => Inserted { Node(String, Map, Value) }
            )
        }
    )
}

Value = String

Value = (Map * String) => Option<Value> {
    Map.(
        * (Empty) => Option<Value> { None() }
        * (Node) => Option<Value> {
            Node.Key.Eq(String).(
                * (False) => Option<Value> { Node.Rest.Value(String) }
                * (True) => Option<Value> { Some(Node.Value) }
            )
        }
    )
}
```

Call site: `Map().Inserted("a" * "1").Inserted("b" * "2").Value("a")?`. Note `Value` carries two constructors -- the total newtype wrap `Value(String)` and the fallible lookup `Value(Map * String)` -- a family with disjoint inputs, which is the feature working, not a collision. The recursive call stores `Inserted` into the `Rest = Map` field through ordinary newtype substitutability.

### Costs, Named Honestly

The information doesn't disappear; it relocates -- `Inserted` is `insert` wearing PascalCase, and the claim is only that the relocation makes the name checked, sorted, resolvable, and usable as evidence. Naming pressure shifts to English participles, and recursive helper chains (the pure-Canon JSON parser: ~20 same-signature `(Int * String) -> ParseStep` steps) are the stress test -- the migration ports that file early on purpose. Overload resolution enters the checker, mitigated by the absence of inference: selection uses declared types only, and disjointness is checked at declaration.

If the full removal proves too costly in practice, the fallback that keeps most of the value: *a camelCase function's return type must appear in its input product (modulo newtype chains and `Result`/`Option`/`Future` wrapping) or be `Unit`; anything producing a type it wasn't given must be named after what it produces.* Verbs transform, constructors create. Slices 1-3 below are identical under both endpoints.

### Minimal Primitives

The compiler supplies a builtin only when it touches something the language *cannot* express. The test is mechanical -- a builtin is justified by exactly one of:

1. **wasm numerics** -- `Int`/`Float` arithmetic and the base comparisons (`lt`, `eq`); machine ops have no decomposition.
2. **linear-memory layout** -- `String`'s `byteAt`/`length`/`substring`/`concat`, `List`'s slot access and growth (`get`/`first`/`append`/`concat`/`map`); Canon values don't expose their own bytes.
3. **canonical-ABI machinery** -- `Parallel`/`Race` waitable sequences, `Handle` resources, async lift/lower.
4. **a host boundary** -- `print` (stdout; becomes an ordinary binding once capability threading lands), the `canon:builtins/*` and `wasi:*` imports.

Everything else lives in Canon source, like any user code. Already pure Canon: `Map`, `Set`, the entire JSON parser and validator, `Int(String)` parsing, `TestResult`, the HTML element vocabulary -- and now `Bool`'s algebra (`And`/`Or`/`Not` in `canon/std/bool.can` are three dispatches; the compiler's `i32.and`/`or`/`eqz` arms are deleted). Note the boolean *operations* need no primitives at all -- dispatch on the union is the machine op.

Queued to move out of the compiler as their blockers clear:

| Builtin today | Moves when |
|---|---|
| derived comparisons (`ne`, `le`, `gt`, `ge` on `Int`/`Float`/`String`) | with the comparison-vocabulary rename (each is one dispatch over `lt`/`eq`) |
| `String(Int)` decimal rendering | needs a load trigger (nothing in `String(42)` references a file); one digit-recursion over `div`/`rem` + `String(Byte)` |
| `List<String>.Json()` | blocked on the `list.get`-on-`List<String>` codegen gap (element types erased) |
| `print` | becomes a binding + `Print` wrapper when capability entry points land |

### Migration Plan

1. **Constructor families, in-file** -- [x] landed. Several self-named constructors per type, selected by the first input's type; duplicate (receiver, name, first-input) definitions are a checked error (`tests/runtime/ctor_family.can`).
   1b. **Anonymous arrows** -- [x] landed. `(A) => B { ... }` declares the `B` constructor (`tests/runtime/ctor_arrow.can`); the named form stays legal until slice 6.
2. **Stdlib cleanup** -- [x] landed. Every wrapper-layer camelCase function is ported except `cli/exit` (a `-> Unit` sink, exempt until capabilities). Accessors and predicates are constructors (`Value`, `Keys`, `Length`, `Contains = Bool`); HTML helpers are tag newtypes (`Button`/`Div`/... `= Html`, escaping via `Escaped`); `assert` is the `Bool => TestResult` constructor (tests themselves are result newtypes of `TestResult` named for what they assert, each with an anonymous nullary constructor); URL fetch is the `Fetched = Body` evidence constructor in its own file. camelCase survives only in binding files -- the FFI boundary, as designed.
3. **Cross-file constructor families** -- [~] partially landed. A name declared *only* as function bodies co-resolves across files (all declaring files load; the checker's coherence guard reports real conflicts). Remaining: `Owner.Item` type-position qualification, reference-site-only ambiguity for type names.
4. **Minimal primitives** -- [~] in progress. A compiler builtin is justified only by wasm numerics, linear-memory layout, canonical-ABI machinery, or a host boundary (see [Minimal Primitives](#minimal-primitives)); everything else moves to stdlib Canon. `Bool`'s `And`/`Or`/`Not` landed (pure dispatch). Remaining: the derived comparisons, `String(Int)` rendering, `print`.
5. **Stdlib port** -- [x] landed, `json.can` included. The recursive-descent parser is ~30 anonymous arrows over result newtypes and the style held; **checkpoint verdict: full removal** (the fallback rule is retired).
6. **Enforcement** -- [x] landed. camelCase outside binding files (and non-test functions) is a hard checker error, not a warning (`check_function`/`check_type_def` in `src/checker/mod.rs`); `canon fmt` canonicalizes every call spelling (`B(a)` / `a.B()` / `a -> B`) to the parens-tail `a -> B(c)` form; entry points are anonymous shape implementations selected by their world-shaped return (`main` as a literal name is itself a checker error unless synthesized). The larger syntax decision -- pipe-only execution plus a distinct `=>` declaration arrow -- is implemented and enforced by the parser.
7. **Spec rewrite** -- this page's content folds into the Functions / Traits / Ordering spec pages; the pre-migration descriptions there are replaced.

