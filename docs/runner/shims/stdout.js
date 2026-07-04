// Browser shim for wasi:cli stdout/stderr (WASI P3 rc), sufficient for
// Canon programs that only print. The transpiled component hands us a
// stream reader; we drain it, split lines, and forward them to whatever
// sink the page installed (docs/theme/run.js sets globalThis.__canonSink
// around each run) — console.log otherwise.
const dec = new TextDecoder();

function normalize(r) {
  if (r && typeof r === "object" && "done" in r) return r;
  return { value: r, done: r === null };
}

function emit(line, isErr) {
  const sink = globalThis.__canonSink;
  if (sink) sink(line, isErr);
  else (isErr ? console.error : console.log)(line);
}

// One line buffer per channel, shared across writeViaStream calls: the
// guest opens a fresh stream per write ("hello" and its "\n" arrive on
// two streams), so per-call buffers would emit phantom empty lines.
// Canon's print always terminates with "\n", so lines only ever flush
// on a newline.
const bufs = { out: "", err: "" };

async function drain(reader, isErr) {
  const key = isErr ? "err" : "out";
  for (;;) {
    const { value, done } = normalize(await reader.read({ count: 65536 }));
    if (done) break;
    if (value && value.length) {
      const bytes = value instanceof Uint8Array ? value : Uint8Array.from(value);
      bufs[key] += dec.decode(bytes, { stream: true });
      let nl;
      while ((nl = bufs[key].indexOf("\n")) >= 0) {
        emit(bufs[key].slice(0, nl), isErr);
        bufs[key] = bufs[key].slice(nl + 1);
      }
    }
  }
  return { tag: "ok", val: undefined };
}

export const stdout = {
  async writeViaStream(reader) {
    return drain(reader, false);
  },
};

export const stderr = {
  async writeViaStream(reader) {
    return drain(reader, true);
  },
};
