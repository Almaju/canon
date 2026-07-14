# Canon for AI

Canon's constraints were adopted for human clarity — and they happen to
make it an unusually good target for **AI code generation**. The same
rules that give a person one obvious way to write a program give a
model a small, regular target and a compiler that turns its mistakes
into precise, fixable errors. Nothing here is retrofitted; every
property is a design decision documented elsewhere in this book.

| Design choice | Why it helps a model |
|---|---|
| One canonical form | There is usually one correct output, not a cloud of stylistic variants — and generated diffs touch only the lines that must change |
| Tiny surface area | The spec fits in context beside the task; no loop forms or keywords to confuse |
| No imports | The most reliable model error — hallucinated paths and symbols — cannot be expressed |
| Types are the only names | No naming scheme to invent and then contradict three lines later |
| Exhaustive, duplicate-free dispatch | Forgotten cases and leftover scaffolding are compile errors, not production surprises |
| Enforced ordering + `canon check --fix` | A fixpoint the agent can run to normalize its own output |
| In-process compiler, golden tests | The write–check–fix loop is fast, local, and unambiguous |
| Capabilities as values | Generated code is sandboxed by construction — its reach is the values you passed in |

Two of these deserve a sentence more.

**No imports.** A reference to an undefined name resolves name-to-file
automatically, so `Path`, `File`, and `Print` are never imported — an
entire category of generation mistake has no syntax to occur in.

**Safe to run.** A Canon program compiles to a WebAssembly component
with no ambient authority. Untrusted, machine-authored code can only
touch the filesystem, network, or clock if you handed it that
capability — your safety does not depend on trusting the model.

Canon wasn't designed *for* AI — it was designed to have one obvious
way to do everything. That it is also a good language to hand a model
is the same idea seen from the other side.
