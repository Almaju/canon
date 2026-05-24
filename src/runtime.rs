//! WASM runtime — embeds wasmtime and runs the compiled component.
//!
//! Targets the **Component Model** with **WASI Preview 3**. Components produced
//! by `codegen::generate` must export `wasi:cli/run.run` as an async function.
//!
//! All WASI capabilities (clocks, random, filesystem, sockets, …) are wired up
//! through wasmtime's own host implementations via `wasmtime_wasi::p3`.
//!
//! Output is reached natively through `wasi:cli/stdout` (the codegen
//! emits the canonical-ABI stream sequence around `write-via-stream`);
//! no `oneway:host/console` bridge is registered. A handful of
//! `oneway:builtins/*` bridges remain for cases without a WASI
//! equivalent (math, strings, clock RFC-3339, URL parse) and for the
//! Phase-5 http-server stub — each is documented in its own submodule
//! and will be replaced with native WASI as the canonical-ABI lowerings
//! for those interfaces land.

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p3::bindings::Command;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Per-store state — owns the WASI context and the component resource table.
struct State {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

/// Runs a WASM Component Model component using WASI Preview 3.
///
/// `bytes` must be a valid component produced by `codegen::generate`. It must
/// export `wasi:cli/run.run`. `args` are forwarded as the program's arguments.
pub fn run_component(bytes: &[u8], args: &[&str]) {
    // The async component-model + tokio combo is required to run a `wasi:cli`
    // command on wasmtime 45 — `Command::instantiate_async` only exists in the
    // async API.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: could not start tokio runtime: {e}");
            std::process::exit(1);
        });

    runtime.block_on(async move {
        if let Err(err) = run_component_async(bytes, args).await {
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    });
}

async fn run_component_async(bytes: &[u8], args: &[&str]) -> wasmtime::Result<()> {
    // Engine: enable Component Model + async canonical ABI. `async_support`
    // is implied by `wasm_component_model_async` in wasmtime 45.
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    // `stream.write` / `future.read` etc. without the `async` canonical
    // option require this feature flag ("more async builtins"). We use
    // synchronous `stream.write` inside `print_str` to keep the body of
    // `wasi:cli/run.run` simple — the host completes immediately because
    // wasmtime-wasi installs its background pump synchronously when
    // `write-via-stream` is called.
    config.wasm_component_model_more_async_builtins(true);
    // `run` is lifted as `canon lift … async` without a callback —
    // i.e. async-stackful. That spelling is also feature-gated.
    config.wasm_component_model_async_stackful(true);
    let engine = Engine::new(&config)?;

    // Linker: wire up all WASI P3 imports (cli, clocks, filesystem, sockets,
    // random) plus the compiler-managed `oneway:*` host bridges that don't
    // yet have a native WASI replacement (math, string ops, clock RFC-3339
    // formatter, URL parse, the Phase-5 http-server stub). The `.print`
    // builtin is now compiled directly against `wasi:cli/stdout` — no host
    // bridge needed for output.
    let mut linker: Linker<State> = Linker::new(&engine);
    // Opt into `wasi:cli/exit#exit-with-code` so guest code can request a
    // non-zero exit status. Without this the host serves only the
    // `exit(result)` form (0 or 1), which isn't expressive enough for a
    // real CLI. The flag is upstream-gated as "unstable"; we treat it as
    // stable because the alternative is shipping a stdlib with no exit
    // codes.
    let mut p3_options = wasmtime_wasi::p3::bindings::LinkOptions::default();
    p3_options.cli_exit_with_code(true);
    wasmtime_wasi::p3::add_to_linker_with_options(&mut linker, &p3_options)?;
    host_builtins::add_to_linker(&mut linker)?;
    host_builtin_clock::add_to_linker(&mut linker)?;
    host_builtin_string::add_to_linker(&mut linker)?;
    host_builtin_filesystem::add_to_linker(&mut linker)?;
    host_builtin_cli::add_to_linker(&mut linker)?;
    host_builtin_http::add_to_linker(&mut linker)?;
    host_builtin_http_server::add_to_linker(&mut linker)?;
    host_builtin_url::add_to_linker(&mut linker)?;

    // WASI context: inherit stdio/env/args from the host process so users see
    // output and can pass CLI arguments through `oneway run …`.
    let mut builder = WasiCtxBuilder::new();
    builder
        .inherit_stdio()
        .inherit_env()
        .inherit_network()
        .allow_ip_name_lookup(true);
    if !args.is_empty() {
        builder.args(args);
    }
    let ctx = builder.build();

    let state = State {
        ctx,
        table: ResourceTable::new(),
    };
    let mut store = Store::new(&engine, state);

    let component = Component::new(&engine, bytes)
        .map_err(|e| wasmtime::Error::msg(format!("invalid wasm component: {e:?}")))?;

    let command = Command::instantiate_async(&mut store, &component, &linker).await?;

    let result = store
        .run_concurrent(async move |store| command.wasi_cli_run().call_run(store).await)
        .await??;

    match result {
        Ok(()) => Ok(()),
        Err(()) => std::process::exit(1),
    }
}

/// `oneway:builtins/math` — a tiny standard library of pure math functions
/// that compiled programs can call via `extern Wasm("oneway:builtins/math…")`.
///
/// Keeps a few common operations (min/max) out of the language proper while
/// the codegen learns to inline them. Once Oneway's stdlib grows real
/// implementations of these, this module can be removed.
mod host_builtins {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface math {
                min: func(a: s64, b: s64) -> s64;
                max: func(a: s64, b: s64) -> s64;
                abs: func(value: s64) -> s64;
            }
            world host-shim {
                import math;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::math::Host for State {
        fn min(&mut self, a: i64, b: i64) -> i64 {
            a.min(b)
        }
        fn max(&mut self, a: i64, b: i64) -> i64 {
            a.max(b)
        }
        fn abs(&mut self, value: i64) -> i64 {
            value.wrapping_abs()
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::math::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/clock` — a string-returning host bridge that demonstrates
/// the canonical-ABI indirect-return path. `now-rfc3339()` reads the host's
/// `SystemTime`, formats it manually as `YYYY-MM-DDTHH:MM:SSZ`, and returns
/// the resulting `String` to the guest. The component wrapper attaches the
/// `Realloc` canonical option to this function's lower so wasmtime can
/// allocate the result buffer inside the guest's linear memory.
mod host_builtin_clock {
    use super::State;
    use std::time::{SystemTime, UNIX_EPOCH};
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface clock {
                now-rfc3339: func() -> string;
                now-unix-seconds: func() -> s64;
            }
            world host-shim {
                import clock;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::clock::Host for State {
        fn now_rfc3339(&mut self) -> String {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            format_rfc3339_utc(secs)
        }

        fn now_unix_seconds(&mut self) -> i64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        }
    }

    /// Format a UNIX timestamp (seconds since 1970-01-01T00:00:00Z) as a
    /// minimal RFC 3339 string. No timezone offset, no fractional seconds —
    /// just enough to validate the host→guest string path end-to-end.
    fn format_rfc3339_utc(unix_secs: i64) -> String {
        // Civil-from-days algorithm (Howard Hinnant). Returns (y, m, d).
        let days = unix_secs.div_euclid(86_400);
        let secs_of_day = unix_secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = if m <= 2 { y + 1 } else { y };

        let h = (secs_of_day / 3600) as u32;
        let mm = ((secs_of_day / 60) % 60) as u32;
        let s = (secs_of_day % 60) as u32;

        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, m, d, h, mm, s)
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::clock::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/string` — simple string transforms. Exercises the
/// `string → string` canonical-ABI path: the guest passes a UTF-8 buffer in
/// its linear memory, the host reads it via the `Memory` option, computes
/// the result, allocates a new buffer in guest memory via `cabi_realloc`,
/// and writes `(ptr, len)` to the guest-provided return area.
mod host_builtin_string {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface strings {
                to-lowercase: func(input: string) -> string;
                to-uppercase: func(input: string) -> string;
            }
            world host-shim {
                import strings;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::strings::Host for State {
        fn to_lowercase(&mut self, input: String) -> String {
            input.to_lowercase()
        }

        fn to_uppercase(&mut self, input: String) -> String {
            input.to_uppercase()
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::strings::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/filesystem` — minimal filesystem operations exposed as
/// `string → string` functions. Errors are reported as empty strings
/// (`""`) until the codegen learns to lower `result<string, error>`. The
/// host has full POSIX-style access; sandboxing happens at the WASI level
/// for everything else.
mod host_builtin_filesystem {
    use super::State;
    use std::fs;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface filesystem {
                /// Open a file by path. Returns the path string back as the
                /// `File` handle on success, or a diagnostic message on
                /// failure. The handle is just the path — actual reading
                /// happens in `read-file-handle`.
                open-file: func(path: string) -> result<string, string>;

                /// Read the contents of a previously-opened `File`. Takes
                /// the same string handle returned by `open-file`.
                read-file-handle: func(file: string) -> result<string, string>;
            }
            world host-shim {
                import filesystem;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::filesystem::Host for State {
        fn open_file(&mut self, path: String) -> Result<String, String> {
            if std::path::Path::new(&path).is_file() {
                Ok(path)
            } else {
                Err(format!("file not found: {path}"))
            }
        }

        fn read_file_handle(&mut self, file: String) -> Result<String, String> {
            fs::read_to_string(&file).map_err(|e| e.to_string())
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::filesystem::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/http` — a minimal blocking HTTP GET. Written against
/// `std::net::TcpStream` to avoid pulling in an HTTP client dependency.
/// Only handles `http://`/`https://` URLs of the shape `scheme://host/path`,
/// returns the response body on 2xx, otherwise an empty string. Until the
/// codegen lowers `result<string, error>`, this is the cleanest shape.
mod host_builtin_http {
    use super::State;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface http {
                /// HTTP GET on a previously-parsed `Url`. Returns the
                /// response body or an error message.
                get: func(url: string) -> result<string, string>;
            }
            world host-shim {
                import http;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::http::Host for State {
        fn get(&mut self, url: String) -> Result<String, String> {
            http_get(&url).ok_or_else(|| format!("HTTP GET failed for {url}"))
        }
    }

    fn http_get(url: &str) -> Option<String> {
        let (host, path) = parse_http_url(url)?;
        let mut stream = TcpStream::connect((host.as_str(), 80)).ok()?;
        let request = format!(
            "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: oneway/0.1\r\nConnection: close\r\nAccept: */*\r\n\r\n",
            path, host
        );
        stream.write_all(request.as_bytes()).ok()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).ok()?;
        let text = String::from_utf8_lossy(&response).into_owned();
        // Split off headers from body.
        let (_, body) = text.split_once("\r\n\r\n")?;
        Some(body.to_string())
    }

    /// Parses a bare `http://host[:port]/path` URL into `(host, path)`. HTTPS
    /// is rejected (TLS isn't included). Returns `None` for malformed input.
    fn parse_http_url(url: &str) -> Option<(String, String)> {
        let rest = url.strip_prefix("http://")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if host.is_empty() {
            return None;
        }
        Some((host.to_string(), path.to_string()))
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::http::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/http-server` — stub host bridge for the Phase-5 HTTP
/// server stdlib. The signatures match what the codegen emits for
/// `std/http-server-wasm.ow`: `get`/`post` are imported as sync
/// `(string, string, s32) -> string` and return the server-handle
/// argument unchanged for chained calls.
///
/// Status: stub. No routes are actually registered, and `.serve()` is
/// `extern Wasm.async`, currently skipped by `collect_extern_imports`.
/// The bodies return the server-handle string back so chained
/// `.get(…).post(…)` builds an opaque-but-stable handle. This lets
/// `examples/http-server/` instantiate end-to-end without the runtime
/// complaining about missing imports — the program then runs and exits
/// cleanly. Real serve semantics arrive with async-canonical-ABI
/// lowering and host-driven handler invocation.
mod host_builtin_http_server {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    // `serve` is declared `async func(…)` in the WIT (it pairs with
    // `CanonicalOption::Async` on the canon.lower emitted by the codegen).
    // The `async: { only_imports: [...] }` option tells wasmtime's bindgen
    // to generate an `async fn` host-trait method for it, so wasmtime can
    // drive it through its task scheduler. `get` and `post` remain sync
    // and use the regular blocking trait method.
    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface http-server {
                get: func(arg0: string, arg1: string, arg2: s32) -> string;
                post: func(arg0: string, arg1: string, arg2: s32) -> string;
                serve: async func(arg0: string) -> s32;
                echo: async func(input: string) -> string;
                slow-echo: async func(input: string) -> string;
            }
            world host-shim {
                import http-server;
            }
        ",
        require_store_data_send: true,
    });

    // Sync functions go on the `Host` trait.
    impl oneway::builtins::http_server::Host for State {
        fn get(&mut self, server: String, _path: String, _handler: i32) -> String {
            server
        }
        fn post(&mut self, server: String, _path: String, _handler: i32) -> String {
            server
        }
    }

    // Async functions go on the `HostWithStore` trait, with an
    // `Accessor<T, Self>` first arg instead of `&mut self`. The trait is
    // generated by the bindgen macro above; we impl it on
    // `HasSelf<State>` so the data getter `|state| state` we already pass
    // into `add_to_linker` resolves the right way.
    impl oneway::builtins::http_server::HostWithStore for HasSelf<State> {
        async fn serve<U: Send>(
            _accessor: &wasmtime::component::Accessor<U, Self>,
            _server: String,
        ) -> i32 {
            // Stub: until route registration + handler dispatch land,
            // `.serve()` is a no-op that returns `0` (the canonical-ABI
            // discriminant for `result::ok`). Real serve semantics need
            // a way for the host to invoke guest handler lambdas —
            // either function-table indirect calls or resource-keyed
            // handler tables.
            0
        }

        async fn echo<U: Send>(
            _accessor: &wasmtime::component::Accessor<U, Self>,
            input: String,
        ) -> String {
            // Used by `tests/runtime/async_echo.ow` to exercise the
            // guest-side async call sequence (alloc ret-area, call,
            // check status, decode result). The Future completes
            // immediately so the guest's sync-completion fast path
            // hits the `Returned` branch.
            input
        }

        async fn slow_echo<U: Send>(
            _accessor: &wasmtime::component::Accessor<U, Self>,
            input: String,
        ) -> String {
            // Used by `tests/runtime/async_slow_echo.ow` to exercise
            // the *async-suspend* path of `emit_async_call`: the host
            // future yields before producing a result, so wasmtime has
            // to return a Started subtask handle to the guest. The
            // guest's generated code then enters the waitable-set.wait
            // block, blocks until this future resolves, and reads the
            // result out of the ret-area. A 1ms sleep is enough to
            // force at least one Pending poll on essentially every
            // executor.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            input
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::http_server::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/url` — URL parsing. Validates the shape of a `Url`
/// string (must start with `http://` or `https://` and have a non-empty
/// host). Returns the same string back as the `Url` handle on success or a
/// diagnostic message on failure.
mod host_builtin_url {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface url {
                parse: func(input: string) -> result<string, string>;
            }
            world host-shim {
                import url;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::url::Host for State {
        fn parse(&mut self, input: String) -> Result<String, String> {
            let scheme_ok = input.starts_with("http://") || input.starts_with("https://");
            if !scheme_ok {
                return Err(format!(
                    "invalid URL: expected http:// or https:// prefix, got {input:?}"
                ));
            }
            let rest = input
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            let host = rest.split('/').next().unwrap_or("");
            if host.is_empty() {
                return Err(format!("invalid URL: empty host in {input:?}"));
            }
            Ok(input)
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::url::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `oneway:builtins/cli` — a thin shim for command-line concerns that
/// either aren't yet served by `wasmtime_wasi::p3` (e.g. timezone) or
/// trip current codegen gaps (e.g. `wasi:cli/exit#exit-with-code` uses a
/// `u8` parameter, and Oneway always lowers `Int` as `u64`). Bridging
/// through `s64` here sidesteps the width mismatch.
///
/// When the underlying codegen learns to honor sub-u64 WIT widths, the
/// stdlib wrapper switches its `extern Wasm` path to point at
/// `wasi:cli/exit` directly and this shim retires.
mod host_builtin_cli {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package oneway:builtins@0.1.0;
            interface cli {
                /// Terminate the program with the given exit code.
                /// Maps directly onto `wasi:cli/exit#exit-with-code` but
                /// takes the code as `s64` so it lines up with Oneway's
                /// canonical `Int` lowering. Values are clamped to the
                /// 0..=255 range expected by POSIX-shaped hosts.
                exit-with-code: func(status-code: s64);
            }
            world host-shim {
                import cli;
            }
        ",
        require_store_data_send: true,
    });

    impl oneway::builtins::cli::Host for State {
        fn exit_with_code(&mut self, status_code: i64) {
            // POSIX exit codes are 8-bit; clamp to that range to match
            // every embedder's expectations. `std::process::exit` skips
            // wasmtime's cleanup paths, which is the right semantics here:
            // the guest asked to terminate immediately.
            let code: i32 = status_code.clamp(0, 255) as i32;
            std::process::exit(code);
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        oneway::builtins::cli::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}
