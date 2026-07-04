//! Case conversion between WIT and Canon identifier conventions.
//!
//! WIT uses kebab-case for everything; Canon uses camelCase for values
//! and PascalCase for types. File paths are snake_case (matching the
//! existing `kebab_case` helper in `loader.rs`, which produces e.g.
//! `http-server` for `HttpServer` — for WIT input we use `_` instead of
//! `-` to keep the resulting paths legal Canon module identifiers).

/// `monotonic-clock` → `monotonicClock`. `get-random-u64` → `getRandomU64`.
pub fn kebab_to_camel(s: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `monotonic-clock` → `MonotonicClock`. `incoming-request` → `IncomingRequest`.
pub fn kebab_to_pascal(s: &str) -> String {
    let camel = kebab_to_camel(s);
    let mut chars = camel.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `monotonic-clock` → `monotonic_clock`. Used for the file stem so the
/// resulting `wasi/clocks/monotonic_clock.can` is a legal Canon module
/// path (kebab is fine in filenames but `-` is not legal in identifiers,
/// and the loader translates between the two in only one direction).
pub fn kebab_to_snake(s: &str) -> String {
    s.replace('-', "_")
}

/// `getRandomU64` → `get-random-u64`. The inverse of [`kebab_to_camel`]
/// for function names: insert a `-` before each uppercase ASCII letter,
/// then lowercase the whole string. Numbers are passed through (so
/// `getRandomU64` stays `get-random-u64`, not `get-random-u-6-4`).
///
/// Used by the loader to reconstruct the WIT function name from the
/// Canon-side camelCase identifier when patching `extern Wasm` paths
/// against the install index.
pub fn camel_to_kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() && i != 0 {
            out.push('-');
        }
        for lower in c.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

/// Split a Component-Model interface id of the form
/// `namespace:package/interface@version` into its parts. `@version`
/// is optional. Returns `(namespace, package, interface, Some(version))`
/// or `(namespace, package, interface, None)`.
///
/// This is the same format that appears inside `extern Wasm("…")`
/// declarations, minus the trailing `#fn-name` (which the caller is
/// responsible for stripping).
pub fn split_interface_id(id: &str) -> Option<(String, String, String, Option<String>)> {
    let (head, version) = match id.rsplit_once('@') {
        Some((h, v)) => (h, Some(v.to_string())),
        None => (id, None),
    };
    let (ns_pkg, iface) = head.rsplit_once('/')?;
    let (ns, pkg) = ns_pkg.rsplit_once(':')?;
    Some((ns.to_string(), pkg.to_string(), iface.to_string(), version))
}

/// Build the on-disk relative path for an interface's generated file.
///
/// `wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15`
///   → `wasi/clocks@0.3.0-rc-2026-03-15/monotonic_clock.can`
///
/// This is the vendored-package layout PACKAGES.md specifies: the
/// directory name carries the package identity and version (the pin),
/// the file stem names the interface, and the loader derives each
/// binding's URN back from exactly this shape — no directive in the
/// file. Kebab-case WIT names become snake_case path segments (use
/// paths can't carry `-`); the derivation maps them back. A version-
/// less WIT package produces an unversioned directory, which the
/// loader rejects at `use` time with a message naming the fix (version
/// the WIT package).
pub fn interface_file_path(ns: &str, pkg: &str, iface: &str, version: Option<&str>) -> String {
    let at_ver = version.map(|v| format!("@{v}")).unwrap_or_default();
    format!(
        "{}/{}{}/{}.can",
        kebab_to_snake(ns),
        kebab_to_snake(pkg),
        at_ver,
        kebab_to_snake(iface),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel() {
        assert_eq!(kebab_to_camel("monotonic-clock"), "monotonicClock");
        assert_eq!(kebab_to_camel("get-random-u64"), "getRandomU64");
        assert_eq!(kebab_to_camel("now"), "now");
        assert_eq!(kebab_to_camel(""), "");
    }

    #[test]
    fn pascal() {
        assert_eq!(kebab_to_pascal("monotonic-clock"), "MonotonicClock");
        assert_eq!(kebab_to_pascal("incoming-request"), "IncomingRequest");
        assert_eq!(kebab_to_pascal("io-error"), "IoError");
        assert_eq!(kebab_to_pascal("url"), "Url");
    }

    #[test]
    fn snake() {
        assert_eq!(kebab_to_snake("monotonic-clock"), "monotonic_clock");
        assert_eq!(kebab_to_snake("clocks"), "clocks");
    }

    #[test]
    fn camel_to_kebab_roundtrips_function_names() {
        // Round-trip identities: kebab → camel → kebab equals identity.
        // These are the cases the loader actually sees.
        assert_eq!(camel_to_kebab("now"), "now");
        assert_eq!(camel_to_kebab("getResolution"), "get-resolution");
        assert_eq!(camel_to_kebab("getRandomU64"), "get-random-u64");
        assert_eq!(
            camel_to_kebab("getInsecureRandomBytes"),
            "get-insecure-random-bytes"
        );
        assert_eq!(camel_to_kebab("writeViaStream"), "write-via-stream");
    }

    #[test]
    fn camel_to_kebab_handles_first_char_uppercase() {
        // First uppercase char doesn't get a leading `-`. Bindgen only
        // produces camelCase fn names, so this case is theoretical for
        // function-name use — but the helper is well-defined for it.
        assert_eq!(camel_to_kebab("GetThing"), "get-thing");
    }

    #[test]
    fn camel_to_kebab_passes_empty_through() {
        assert_eq!(camel_to_kebab(""), "");
    }

    #[test]
    fn split_iface() {
        let (ns, pkg, iface, ver) =
            split_interface_id("wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15").unwrap();
        assert_eq!(ns, "wasi");
        assert_eq!(pkg, "clocks");
        assert_eq!(iface, "monotonic-clock");
        assert_eq!(ver.as_deref(), Some("0.3.0-rc-2026-03-15"));

        let (ns, pkg, iface, ver) = split_interface_id("acme:foo/bar").unwrap();
        assert_eq!(ns, "acme");
        assert_eq!(pkg, "foo");
        assert_eq!(iface, "bar");
        assert_eq!(ver, None);
    }

    #[test]
    fn file_path() {
        assert_eq!(
            interface_file_path("wasi", "clocks", "monotonic-clock", Some("0.3.0")),
            "wasi/clocks@0.3.0/monotonic_clock.can"
        );
        assert_eq!(
            interface_file_path("wasi", "clocks", "monotonic-clock", None),
            "wasi/clocks/monotonic_clock.can"
        );
    }
}
