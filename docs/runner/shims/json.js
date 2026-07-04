// Browser shim for canon:builtins/json@0.1.0 - the host bridge behind
// canon/std/Json. The interface passes JSON values as raw JSON text, so
// the browser's own JSON object covers all of it.
export const json = {
  parse(input) {
    try {
      JSON.parse(input);
      return { tag: "ok", val: input };
    } catch (e) {
      return { tag: "err", val: String(e.message || e) };
    }
  },
  fromString(v) {
    return JSON.stringify(v);
  },
  fromInt(v) {
    return String(v);
  },
  fromFloat(v) {
    return Number.isFinite(v) ? String(v) : "null";
  },
  fromBool(v) {
    return v ? "true" : "false";
  },
  fromNull() {
    return "null";
  },
  field(input, name) {
    try {
      const o = JSON.parse(input);
      if (o === null || typeof o !== "object" || Array.isArray(o) || !(name in o))
        return { tag: "err", val: `missing field ${name}` };
      return { tag: "ok", val: JSON.stringify(o[name]) };
    } catch (e) {
      return { tag: "err", val: String(e.message || e) };
    }
  },
  toString(input) {
    try {
      const v = JSON.parse(input);
      if (typeof v !== "string") return { tag: "err", val: "not a JSON string" };
      return { tag: "ok", val: v };
    } catch (e) {
      return { tag: "err", val: String(e.message || e) };
    }
  },
};
