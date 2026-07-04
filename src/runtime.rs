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
//! no `canon:host/console` bridge is registered. A handful of
//! `canon:builtins/*` bridges remain for cases without a WASI
//! equivalent (math, strings, clock RFC-3339, URL parse) and for the
//! Phase-5 http-server stub — each is documented in its own submodule
//! and will be replaced with native WASI as the canonical-ABI lowerings
//! for those interfaces land.

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use std::future::Future;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};

use wasmtime_wasi::{TrappableError, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::p3::{RequestOptions, WasiHttpCtxView, WasiHttpHooks, WasiHttpView};
use wasmtime_wasi_http::WasiHttpCtx;

/// Per-store state — owns the WASI context and the component resource
/// table, plus the WASI HTTP context for `wasi:http/handler` exports.
///
/// The HTTP fields are always allocated even when we're driving a
/// `wasi:cli/command` (so the existing `run_component` path keeps
/// working). `WasiHttpCtx` is cheap; the only real cost is the
/// per-component-instance `add_to_linker` registration of
/// `wasi:http/{types,client}`, which is harmless for guests that don't
/// import them.
struct State {
    ctx: WasiCtx,
    table: ResourceTable,
    http: WasiHttpCtx,
    http_hooks: OneswayHttpHooks,
    /// Optional reference to the guest's `canon:http-handler/handler.handle-request`
    /// component export, when the program defined a `handleRequest`
    /// function. The HTTP server runtime calls this Func per incoming
    /// request to compute the response body. See `DYNAMIC-HANDLERS.md`.
    handler_func: Option<wasmtime::component::Func>,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for State {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

/// Canon's `WasiHttpHooks` implementation.
///
/// We disable wasmtime-wasi-http's `default-send-request` feature (which
/// pulls in rustls + webpki + tokio-rustls), so the hook's outbound
/// `send_request` becomes required rather than defaulted. Today Canon
/// programs use the `canon:builtins/http` host bridge for outbound HTTP
/// (see `host_builtin_http`), so a guest calling `wasi:http/client.send`
/// is out-of-band — we return `internal-error` rather than mask the
/// architectural gap with a silently routed request.
///
/// When `wasi:http/client` migration lands (replacing the
/// `canon:builtins/http` bridge), this hook becomes a real outbound
/// client — either by re-enabling `default-send-request` or by routing
/// through a hyper client of our own.
struct OneswayHttpHooks;

impl WasiHttpHooks for OneswayHttpHooks {
    fn send_request(
        &mut self,
        _request: http::Request<UnsyncBoxBody<Bytes, ErrorCode>>,
        _options: Option<RequestOptions>,
        _fut: Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (
                        http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
                        Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
                    ),
                    TrappableError<ErrorCode>,
                >,
            > + Send,
    > {
        Box::new(async {
            Err(ErrorCode::InternalError(Some(
                "wasi:http/client outbound requests are not routed by the \
                 Canon runtime yet: use `canon:builtins/http` (via \
                 `canon/std/Url`) for now"
                    .to_string(),
            ))
            .into())
        })
    }
}

impl State {
    fn new(ctx: WasiCtx) -> Self {
        Self {
            ctx,
            table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            http_hooks: OneswayHttpHooks,
            handler_func: None,
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
            // A guest `wasi:cli/exit#exit(-with-code)` call surfaces as
            // an `I32Exit` trap — that's a normal termination request,
            // not an error. Propagate the code.
            if let Some(exit) = err.downcast_ref::<wasmtime_wasi::I32Exit>() {
                std::process::exit(exit.0);
            }
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    });
}

async fn run_component_async(bytes: &[u8], args: &[&str]) -> wasmtime::Result<()> {
    let engine = build_engine()?;
    let linker = build_linker(&engine)?;
    let mut store = Store::new(&engine, build_state(args));
    let component = Component::new(&engine, bytes)
        .map_err(|e| wasmtime::Error::msg(format!("invalid wasm component: {e:?}")))?;

    // Instantiate via the linker directly so we have access to the
    // `Instance` handle. We use it to (a) look up the optional
    // `canon:http-handler/handler.handle-request` export and stash it
    // in `State`, and (b) drive `wasi:cli/run.run` ourselves — the
    // bindgen-generated `Command` keeps its inner `Instance` private,
    // and we need both the run call *and* the dynamic-handler export
    // to reference the *same* instance (otherwise the handler `Func`
    // we stash wouldn't match the instance executing `serve`).
    let instance = linker.instantiate_async(&mut store, &component).await?;

    // Optional dynamic-handler export hookup. When present, stash the
    // `Func` in `State` so `host_builtin_http_server::serve` can call
    // it per request. When absent the handler stays unset and the
    // HTTP server falls back to its static-body route table.
    if let Some(iface_idx) =
        instance.get_export_index(&mut store, None, "canon:http-handler/handler@0.1.0")
    {
        if let Some(fn_idx) =
            instance.get_export_index(&mut store, Some(&iface_idx), "handle-request")
        {
            if let Some(func) = instance.get_func(&mut store, fn_idx) {
                store.data_mut().handler_func = Some(func);
            }
        }
    }

    // Look up `wasi:cli/run.run` and call it as an async-stackful
    // function returning `result<_, _>`. Mirrors what `Command::wasi_cli_run().call_run`
    // does internally for the typed-bindings path.
    let run_iface_idx = instance
        .get_export_index(&mut store, None, WASI_CLI_RUN)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing {WASI_CLI_RUN} export")))?;
    let run_fn_idx = instance
        .get_export_index(&mut store, Some(&run_iface_idx), "run")
        .ok_or_else(|| wasmtime::Error::msg("missing wasi:cli/run.run export"))?;
    let run_func: wasmtime::component::TypedFunc<(), (Result<(), ()>,)> =
        instance.get_typed_func(&mut store, run_fn_idx)?;

    let (result,) = store
        .run_concurrent(async move |store| run_func.call_concurrent(store, ()).await)
        .await??;

    match result {
        Ok(()) => Ok(()),
        Err(()) => std::process::exit(1),
    }
}

/// The component-export path for `wasi:cli/run`. Must match what the
/// component wrapper emits in `wasm/component.rs`.
const WASI_CLI_RUN: &str = "wasi:cli/run@0.3.0-rc-2026-03-15";

/// Builds the shared `wasmtime::Engine` for both `run` and `serve` paths.
///
/// All three feature flags are required:
///   * `wasm_component_model_async` — the async canonical ABI itself.
///   * `wasm_component_model_more_async_builtins` — unguarded `stream.write`
///     / `future.read` etc., used by `print_str` and the upcoming
///     `wasi:http` body streams.
///   * `wasm_component_model_async_stackful` — lets `wasi:cli/run.run` be
///     lifted as async-stackful (no callback), which is also what
///     `wasi:http/handler.handle` needs.
fn build_engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    Engine::new(&config)
}

/// Builds a `Linker<State>` populated with every host interface a
/// compiled Canon component might import.
///
/// Shared between `run_component` (command-style guests) and
/// `serve_component` (`wasi:http/service`-style guests). The list is
/// idempotent — registering `wasi:http/{types,client}` on a `wasi:cli`
/// command is harmless because the command imports none of those
/// interfaces; the validator only checks imports the component
/// *actually* references.
fn build_linker(engine: &Engine) -> wasmtime::Result<Linker<State>> {
    let mut linker: Linker<State> = Linker::new(engine);

    // Wire up all WASI P3 imports (cli, clocks, filesystem, sockets,
    // random). Opt into `wasi:cli/exit#exit-with-code` so guest code can
    // request a non-zero exit status — the default linker only registers
    // the `exit(result)` form (0 or 1), which isn't expressive enough
    // for a real CLI. The flag is upstream-gated as "unstable"; we
    // treat it as stable because the alternative is shipping a stdlib
    // with no exit codes.
    let mut p3_options = wasmtime_wasi::p3::bindings::LinkOptions::default();
    p3_options.cli_exit_with_code(true);
    wasmtime_wasi::p3::add_to_linker_with_options(&mut linker, &p3_options)?;

    // WASI HTTP P3 — imported `types` and `client` interfaces. The matching
    // export (`handler`) is consumed via `Service::instantiate_async` in
    // `serve_component_async`; for `run_component` the guest never
    // imports these and the registration is a no-op.
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;

    // Compiler-managed `canon:*` host bridges. Each is a temporary
    // scaffold that will migrate to a `wasi:*` interface as the
    // canonical-ABI shapes (resources, async, streams) become available
    // in the codegen. The `.print` builtin is compiled directly against
    // `wasi:cli/stdout` — no host bridge needed for output.
    host_builtins::add_to_linker(&mut linker)?;
    host_builtin_clock::add_to_linker(&mut linker)?;
    host_builtin_string::add_to_linker(&mut linker)?;
    host_builtin_filesystem::add_to_linker(&mut linker)?;
    host_builtin_http::add_to_linker(&mut linker)?;
    host_builtin_http_server::add_to_linker(&mut linker)?;
    host_builtin_json::add_to_linker(&mut linker)?;
    host_builtin_url::add_to_linker(&mut linker)?;

    Ok(linker)
}

/// Builds a fresh `State` (WASI context + resource table + HTTP context)
/// for one component instantiation.
///
/// `args` becomes the program's `argv` when non-empty; stdio, env, and
/// network access are inherited from the host process so users see
/// printed output and the program can resolve hostnames /
/// `canon:builtins/http` outbound calls.
fn build_state(args: &[&str]) -> State {
    let mut builder = WasiCtxBuilder::new();
    builder
        .inherit_stdio()
        .inherit_env()
        .inherit_network()
        .allow_ip_name_lookup(true);
    if !args.is_empty() {
        builder.args(args);
    }
    State::new(builder.build())
}

/// Runs a WASI HTTP P3 `service` component as an HTTP server.
///
/// `bytes` must be a valid component produced by `codegen::generate` that
/// exports `wasi:http/handler.handle` (i.e. matches the `wasi:http/service`
/// world). The server binds to `addr` and forwards each accepted HTTP/1.1
/// request to the guest's `handle` function via
/// `wasmtime-wasi-http`'s P3 bridge.
///
/// This blocks the calling thread until the listener is shut down
/// (currently never — there's no graceful-shutdown wiring yet; killing
/// the process is the supported way to stop). Errors during accept /
/// dispatch are logged to stderr but don't terminate the server; a
/// fatal bind error is fatal.
pub fn serve_component(bytes: &[u8], addr: std::net::SocketAddr) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: could not start tokio runtime: {e}");
            std::process::exit(1);
        });

    runtime.block_on(async move {
        if let Err(err) = serve_component_async(bytes, addr).await {
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    });
}

async fn serve_component_async(bytes: &[u8], addr: std::net::SocketAddr) -> wasmtime::Result<()> {
    use std::sync::Arc;
    use wasmtime_wasi_http::p3::bindings::ServicePre;

    let engine = build_engine()?;
    let linker = build_linker(&engine)?;
    let component = Component::new(&engine, bytes)
        .map_err(|e| wasmtime::Error::msg(format!("invalid wasm component: {e:?}")))?;

    // Pre-instantiate so per-connection setup is just `instantiate_async`
    // against the already-typed component. Mirrors how `wasmtime serve`
    // and the wasmtime-wasi-http test fixtures structure things.
    let instance_pre = linker.instantiate_pre(&component)?;
    let service_pre = Arc::new(ServicePre::new(instance_pre)?);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| wasmtime::Error::msg(format!("bind {addr}: {e}")))?;
    eprintln!("canon run --addr {addr}: listening on http://{addr}");

    // Accept loop. Each connection gets its own task with its own
    // wasmtime `Store` so guest state is connection-scoped — a panic /
    // trap in one request can't poison another connection. This matches
    // how `wasmtime serve` runs the proxy world today.
    loop {
        let (socket, peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("canon run --addr: accept error: {e}");
                continue;
            }
        };
        let engine = engine.clone();
        let service_pre = service_pre.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_connection(engine, service_pre, socket).await {
                eprintln!("canon run --addr: {peer}: {e:?}");
            }
        });
    }
}

/// Serves a single accepted TCP connection: instantiate the component
/// into a fresh `Store`, run hyper's HTTP/1.1 state machine against it,
/// and dispatch each request through `Service::handle`.
async fn serve_connection(
    engine: Engine,
    service_pre: std::sync::Arc<wasmtime_wasi_http::p3::bindings::ServicePre<State>>,
    socket: tokio::net::TcpStream,
) -> wasmtime::Result<()> {
    use hyper::service::service_fn;
    use std::sync::Arc;

    // One `Store` per connection. We *could* share across requests on
    // the same keep-alive connection, but a fresh store makes the
    // resource-table boundaries crisp and matches the lifetime
    // semantics wasi:http components expect (request resource owned by
    // a single invocation).
    let mut store = Store::new(&engine, build_state(&[]));
    // `Service` is not `Clone`, but its methods take `&self` and the
    // hyper service closure needs to be reusable across keep-alive
    // requests, so we own it through an `Arc`.
    let service = Arc::new(service_pre.instantiate_async(&mut store).await?);
    let store = Arc::new(tokio::sync::Mutex::new(store));

    let io = hyper_util::rt::TokioIo::new(socket);
    let conn_service = service_fn(move |req: http::Request<hyper::body::Incoming>| {
        let service = service.clone();
        let store = store.clone();
        async move { dispatch_request(service, store, req).await }
    });

    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(io, conn_service)
        .await
    {
        return Err(wasmtime::Error::msg(format!("hyper: {e}")));
    }
    Ok(())
}

/// Bridges one hyper request to the guest's `wasi:http/handler.handle`.
///
/// Conversion in/out of `wasmtime-wasi-http`'s P3 `Request` / `Response`
/// is done with the library's own `from_http` / `into_http` helpers — we
/// don't touch resource tables directly. The guest returns either an
/// owned `Response` resource or an `error-code`; the latter is
/// translated to a `500 Internal Server Error` with the WIT error name
/// in the body so problems are debuggable without a separate log.
async fn dispatch_request(
    service: std::sync::Arc<wasmtime_wasi_http::p3::bindings::Service>,
    store: std::sync::Arc<tokio::sync::Mutex<Store<State>>>,
    req: http::Request<hyper::body::Incoming>,
) -> wasmtime::Result<http::Response<UnsyncBoxBody<Bytes, ErrorCode>>> {
    use http_body_util::BodyExt;
    use wasmtime_wasi_http::p3::Request;

    // Promote the hyper body's error type into wasi:http's `ErrorCode`
    // domain so `Request::from_http` accepts it.
    let req = req.map(|body| {
        body.map_err(|e| ErrorCode::InternalError(Some(format!("hyper body: {e}"))))
            .boxed_unsync()
    });
    let (wasi_req, io_fut) = Request::from_http(req);

    let mut guard = store.lock().await;
    // The whole request lifecycle — calling the guest, converting the
    // returned response resource, and consuming its body — must happen
    // inside one `run_concurrent` scope. The guest's body/trailers
    // reach us through host-side pipe tasks registered on the store,
    // and those tasks are only polled while `run_concurrent` drives
    // the store. Collecting the body outside the scope would hang
    // forever on a body channel nobody is feeding.
    //
    // Buffering the full body here caps us at non-streaming responses;
    // when `Stream<T>` response bodies land (STREAMING.md slice 3),
    // this becomes a keep-driving loop that feeds hyper incrementally.
    let response = guard
        .run_concurrent(async |store| -> wasmtime::Result<_> {
            match service.handle(store, wasi_req).await? {
                Ok(resp) => {
                    // `into_http` wires the guest's body stream into a
                    // hyper-compatible body. The `async { Ok(()) }`
                    // future is the host-side completion signal; we
                    // have no late-stage processing to report.
                    let resp = store.with(|mut s| resp.into_http(&mut s, async { Ok(()) }))?;
                    let (parts, body) = resp.into_parts();
                    let collected = body
                        .collect()
                        .await
                        .map_err(|e| wasmtime::Error::msg(format!("guest body: {e:?}")))?;
                    let body = http_body_util::Full::new(collected.to_bytes())
                        .map_err(|never| match never {})
                        .boxed_unsync();
                    Ok(http::Response::from_parts(parts, body))
                }
                Err(err) => Ok(error_response(err)),
            }
        })
        .await
        .and_then(|inner| inner)
        .map_err(|e| {
            // Hyper reports a failed service closure as an opaque
            // "error from user's Service"; log the underlying wasmtime
            // error (trap, missing export, canonical-ABI violation)
            // here where it's still visible.
            eprintln!("canon run --addr: handler dispatch failed: {e:?}");
            e
        })?;

    // Drive the request-body-processing future to completion so the
    // guest sees `Ok(())` (body fully consumed) rather than a dangling
    // future error. We discard the outcome — the guest already
    // returned its response by this point.
    let _ = io_fut.await;

    Ok(response)
}

/// Renders an `error-code` returned by the guest as a `500` response so
/// the failure surfaces over the wire (and in `curl -v`) instead of
/// dropping the connection.
fn error_response(err: ErrorCode) -> http::Response<UnsyncBoxBody<Bytes, ErrorCode>> {
    use http_body_util::{BodyExt, Full};
    let body_str = format!("wasi:http/handler returned error: {err:?}\n");
    let body = Full::new(Bytes::from(body_str))
        .map_err(|never| match never {})
        .boxed_unsync();
    http::Response::builder()
        .status(http::StatusCode::INTERNAL_SERVER_ERROR)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body)
        .expect("static response builder shape is valid")
}

/// `canon:builtins/math` — a tiny standard library of pure math functions
/// that compiled programs can call via `extern Wasm("canon:builtins/math…")`.
///
/// Keeps a few common operations (min/max) out of the language proper while
/// the codegen learns to inline them. Once Canon's stdlib grows real
/// implementations of these, this module can be removed.
mod host_builtins {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
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

    impl canon::builtins::math::Host for State {
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
        canon::builtins::math::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/clock` — a string-returning host bridge that demonstrates
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
            package canon:builtins@0.1.0;
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

    impl canon::builtins::clock::Host for State {
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
        canon::builtins::clock::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/string` — simple string transforms. Exercises the
/// `string → string` canonical-ABI path: the guest passes a UTF-8 buffer in
/// its linear memory, the host reads it via the `Memory` option, computes
/// the result, allocates a new buffer in guest memory via `cabi_realloc`,
/// and writes `(ptr, len)` to the guest-provided return area.
mod host_builtin_string {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
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

    impl canon::builtins::strings::Host for State {
        fn to_lowercase(&mut self, input: String) -> String {
            input.to_lowercase()
        }

        fn to_uppercase(&mut self, input: String) -> String {
            input.to_uppercase()
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        canon::builtins::strings::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/filesystem` — minimal filesystem operations exposed as
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
            package canon:builtins@0.1.0;
            interface filesystem {
                /// Open a file by path. Returns the path string back as the
                /// `File` handle on success, or a diagnostic message on
                /// failure. The handle is just the path — actual reading
                /// happens in `read`.
                open-file: func(path: string) -> result<string, string>;

                /// Read the contents of a previously-opened `File`. Takes
                /// the same string handle returned by `open-file`.
                read: func(file: string) -> result<string, string>;

                /// Write `contents` to the file at `path`, creating it if
                /// absent and truncating if present. Returns the path back
                /// on success so call sites can keep chaining
                /// (`.write(...)?.File()?.read()?`).
                write: func(contents: string, path: string) -> result<string, string>;
            }
            world host-shim {
                import filesystem;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::filesystem::Host for State {
        fn open_file(&mut self, path: String) -> Result<String, String> {
            if std::path::Path::new(&path).is_file() {
                Ok(path)
            } else {
                Err(format!("file not found: {path}"))
            }
        }

        fn read(&mut self, file: String) -> Result<String, String> {
            fs::read_to_string(&file).map_err(|e| e.to_string())
        }

        fn write(&mut self, contents: String, path: String) -> Result<String, String> {
            fs::write(&path, contents.as_bytes())
                .map(|_| path)
                .map_err(|e| e.to_string())
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        canon::builtins::filesystem::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/http` — a minimal blocking HTTP GET. Written against
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
            package canon:builtins@0.1.0;
            interface http {
                /// HTTP GET on a previously-parsed `Url`. Returns the
                /// response body or an error message.
                fetch: func(url: string) -> result<string, string>;
            }
            world host-shim {
                import http;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::http::Host for State {
        fn fetch(&mut self, url: String) -> Result<String, String> {
            http_get(&url).ok_or_else(|| format!("HTTP GET failed for {url}"))
        }
    }

    fn http_get(url: &str) -> Option<String> {
        let (host, path) = parse_http_url(url)?;
        let mut stream = TcpStream::connect((host.as_str(), 80)).ok()?;
        let request = format!(
            "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: canon/0.1\r\nConnection: close\r\nAccept: */*\r\n\r\n",
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
        canon::builtins::http::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/http-server` — host bridge for `std/http/http-server`.
///
/// The API surface is small and stateless: `create(port)` produces an
/// opaque server-handle string, `get`/`post` thread routes through that
/// handle by appending to it, and `serve` parses the encoded routes and
/// runs a real tokio HTTP/1.1 listener.
///
/// ## Why encode routes inside the handle?
///
/// Routes can't be parameterised by guest lambdas yet — there's no
/// host→guest callback path for `extern Wasm` imports. So instead of a
/// host-side state map keyed by handle (which would also require some way
/// for the guest to express handler bodies), we keep everything in the
/// handle string itself: each `.get(…)` / `.post(…)` appends a record to
/// the string and returns the new handle for the next chained call.
/// `.serve()` reads the whole handle, decodes the route table, opens a
/// TCP listener, and replies with the registered body+status for matching
/// requests. Anything unmatched gets a `404 Not Found`.
///
/// ## Wire format of the handle string
///
/// ```text
/// <port> RS <method> US <status> US <path> US <body> RS …
/// ```
///
/// where `RS` is `\x1E` (record separator) and `US` is `\x1F` (unit
/// separator). Both are ASCII control characters that don't appear in
/// normal HTTP route content, so we avoid any escaping. The first record
/// is just the port; every subsequent record is a route.
mod host_builtin_http_server {
    use super::State;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wasmtime::component::{HasSelf, Linker};

    const RS: char = '\x1E';
    const US: char = '\x1F';

    // `serve` is declared `async func(…)` in the WIT (it pairs with
    // `CanonicalOption::Async` on the canon.lower emitted by the codegen).
    // The `async: { only_imports: [...] }` option tells wasmtime's bindgen
    // to generate an `async fn` host-trait method for it, so wasmtime can
    // drive it through its task scheduler. `get`/`post`/`create` remain
    // sync and use the regular blocking trait method.
    //
    // The WIT signatures match what the codegen emits from the stdlib
    // declarations in `std/http/http-server.can`. `Int` lowers to `s64` in
    // the `canon:*` namespace (signed default for non-WASI imports);
    // `HttpServer = String` is a string alias.
    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface http-server {
                create: func(port: s64) -> string;
                get: func(server: string, status: s64, path: string, body: string) -> string;
                post: func(server: string, status: s64, path: string, body: string) -> string;
                serve: async func(server: string) -> s32;
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
    impl canon::builtins::http_server::Host for State {
        fn create(&mut self, port: i64) -> String {
            // First record of the handle is just the port number. Each
            // chained `.get(…)` / `.post(…)` appends one route record.
            port.to_string()
        }

        fn get(&mut self, server: String, status: i64, path: String, body: String) -> String {
            append_route(&server, "GET", status, &path, &body)
        }

        fn post(&mut self, server: String, status: i64, path: String, body: String) -> String {
            append_route(&server, "POST", status, &path, &body)
        }
    }

    // Async functions go on the `HostWithStore` trait, with an
    // `Accessor<T, Self>` first arg instead of `&mut self`. The trait is
    // generated by the bindgen macro above; we impl it on
    // `HasSelf<State>` so the data getter `|state| state` we already pass
    // into `add_to_linker` resolves the right way.
    impl canon::builtins::http_server::HostWithStore for HasSelf<State> {
        async fn serve<U: Send>(
            accessor: &wasmtime::component::Accessor<U, Self>,
            server: String,
        ) -> i32 {
            // Snapshot the (optional) guest-side `handle-request` Func
            // from State. When present, every incoming request is
            // dispatched through this Func via `call_concurrent` from
            // inside the connection loop. When absent, we fall back to
            // the static-body route table (existing behaviour).
            let handler_func: Option<wasmtime::component::Func> =
                accessor.with(|mut access| access.get().handler_func);
            match run_server(accessor, &server, handler_func).await {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("http-server: {e}");
                    1
                }
            }
        }

        async fn echo<U: Send>(
            _accessor: &wasmtime::component::Accessor<U, Self>,
            input: String,
        ) -> String {
            // Used by `tests/runtime/async_echo.can` to exercise the
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
            // Used by `tests/runtime/async_slow_echo.can` to exercise
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
        canon::builtins::http_server::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }

    /// Append one route record to the handle string. See the module-level
    /// comment for the wire format.
    fn append_route(server: &str, method: &str, status: i64, path: &str, body: &str) -> String {
        let mut out =
            String::with_capacity(server.len() + method.len() + path.len() + body.len() + 16);
        out.push_str(server);
        out.push(RS);
        out.push_str(method);
        out.push(US);
        out.push_str(&status.to_string());
        out.push(US);
        out.push_str(path);
        out.push(US);
        out.push_str(body);
        out
    }

    /// A single registered route. Keyed by `(method, path)` in the route
    /// table built from the handle string at `serve` time.
    #[derive(Clone)]
    struct Route {
        status: u16,
        body: String,
    }

    /// Parse the handle string into `(port, routes)`. Returns an error on
    /// malformed input — but in practice the handle is always produced by
    /// our own `create`/`get`/`post` impls, so this is mostly defensive.
    #[allow(clippy::type_complexity)]
    fn decode_handle(handle: &str) -> Result<(u16, HashMap<(String, String), Route>), String> {
        let mut records = handle.split(RS);
        let port_str = records
            .next()
            .ok_or_else(|| "empty server handle".to_string())?;
        let port: u16 = port_str
            .parse()
            .map_err(|_| format!("invalid port `{port_str}`"))?;
        let mut routes = HashMap::new();
        for rec in records {
            let mut fields = rec.split(US);
            let method = fields.next().unwrap_or("").to_ascii_uppercase();
            let status_str = fields.next().unwrap_or("");
            let path = fields.next().unwrap_or("").to_string();
            let body = fields.next().unwrap_or("").to_string();
            let status: u16 = status_str
                .parse()
                .map_err(|_| format!("invalid status `{status_str}`"))?;
            routes.insert((method, path), Route { status, body });
        }
        Ok((port, routes))
    }

    /// Bind a TCP listener and serve registered routes until cancelled.
    /// Logs each accepted request to stderr so the example is
    /// self-documenting (`curl localhost:3000/` shows up as a log line).
    ///
    /// Connections are handled **serially** when a dynamic handler is
    /// present (because the wasmtime callback ABI needs the
    /// `Accessor` we received in `serve`, which can't be cloned
    /// across tasks). For the static-body fallback we keep the
    /// per-connection `tokio::spawn` so multiple clients can be served
    /// concurrently — the route table is `Clone` and bounded‐copy.
    async fn run_server<U: Send>(
        accessor: &wasmtime::component::Accessor<U, HasSelf<State>>,
        handle: &str,
        handler_func: Option<wasmtime::component::Func>,
    ) -> Result<(), String> {
        let (port, routes) = decode_handle(handle)?;
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("bind {addr}: {e}"))?;
        eprintln!("http-server: listening on http://{addr}");
        loop {
            let (mut socket, peer) = match listener.accept().await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("http-server: accept error: {e}");
                    continue;
                }
            };
            match handler_func {
                Some(func) => {
                    // Dynamic-handler path: handle inline so we can
                    // call the guest export via `call_concurrent`. No
                    // `tokio::spawn` because the accessor isn't `Send`
                    // across task boundaries.
                    if let Err(e) = handle_connection_dynamic(&mut socket, func, accessor).await {
                        eprintln!("http-server: {peer}: {e}");
                    }
                }
                None => {
                    // Static-body path: spawn per-connection.
                    let routes = routes.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(&mut socket, &routes).await {
                            eprintln!("http-server: {peer}: {e}");
                        }
                    });
                }
            }
        }
    }

    /// Connection handler for the dynamic-handler path. Reads one HTTP
    /// request, calls the guest's `handle-request` export with the
    /// request body, and writes the returned string as the response
    /// body (status fixed at 200 for now — routing comes later).
    async fn handle_connection_dynamic<U: Send>(
        socket: &mut tokio::net::TcpStream,
        handler_func: wasmtime::component::Func,
        accessor: &wasmtime::component::Accessor<U, HasSelf<State>>,
    ) -> Result<(), String> {
        use wasmtime::component::Val;
        let mut buf = [0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        let head = String::from_utf8_lossy(&buf[..n]);
        let request_line = head.lines().next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_ascii_uppercase();
        let target = parts.next().unwrap_or("");
        let path = target.split('?').next().unwrap_or(target).to_string();
        eprintln!("http-server: {method} {path}");

        // Extract the request body. Naive parser: locate the
        // header/body separator `\r\n\r\n` and treat everything after
        // it as the body. Sufficient for the small payloads we test
        // with; full streaming bodies come later.
        let body_start = head.find("\r\n\r\n").map(|i| i + 4).unwrap_or(n);
        let body = if body_start < n {
            String::from_utf8_lossy(&buf[body_start..n]).into_owned()
        } else {
            String::new()
        };

        // Call into the guest handler.
        let params = vec![Val::String(body)];
        let mut results = vec![Val::String(String::new())];
        let response = match handler_func
            .call_concurrent(accessor, &params, &mut results)
            .await
        {
            Ok(()) => {
                if let Val::String(s) = &results[0] {
                    // Optional Content-Type override: when the handler
                    // wants to return SSE / JSON / HTML / etc., it
                    // prefixes its response with
                    // `Content-Type: <mime>\r\n\r\n` and the rest is
                    // the body. We detect this and honour it. Absent
                    // the prefix we use `text/plain` as before.
                    //
                    // This is the minimum-viable SSE pathway from
                    // `DYNAMIC-HANDLERS.md` slice 3 — a single
                    // event-stream payload returned in one shot. True
                    // multi-event streaming (`SseSender` capability,
                    // events pushed over time) builds on top of this
                    // by passing a host-owned writer handle into the
                    // handler; this MVP proves the Content-Type wiring.
                    let (content_type, body_bytes) = parse_handler_response(s);
                    build_response_with_type(200, content_type, body_bytes)
                } else {
                    build_response(500, "handler returned non-string value")
                }
            }
            Err(e) => {
                eprintln!("http-server: handler error: {e}");
                build_response(500, "handler error")
            }
        };
        socket
            .write_all(response.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        let _ = socket.shutdown().await;
        Ok(())
    }

    /// Read one HTTP/1.1 request, match it against the route table, and
    /// write back the registered response. Only the request line is
    /// inspected — headers and body are read and discarded so the socket
    /// drains cleanly before we close it. `Connection: close` is always
    /// sent, so we don't need to worry about keep-alive bookkeeping.
    async fn handle_connection(
        socket: &mut tokio::net::TcpStream,
        routes: &HashMap<(String, String), Route>,
    ) -> Result<(), String> {
        // Read up to 8 KiB of request data — enough for the request line
        // plus typical headers. Real servers would stream this; for the
        // demo a fixed buffer is fine and bounds memory use per connection.
        let mut buf = [0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        let head = String::from_utf8_lossy(&buf[..n]);
        let request_line = head.lines().next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_ascii_uppercase();
        let target = parts.next().unwrap_or("");
        // Strip any query string — routes match on path only.
        let path = target.split('?').next().unwrap_or(target).to_string();
        eprintln!("http-server: {method} {path}");

        let response = match routes.get(&(method, path)) {
            Some(route) => build_response(route.status, &route.body),
            None => build_response(404, "Not Found"),
        };
        socket
            .write_all(response.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        // Best-effort flush + shutdown so the client sees EOF promptly.
        let _ = socket.shutdown().await;
        Ok(())
    }

    /// Build a minimal HTTP/1.1 response. Body is sent as `text/plain`
    /// with an explicit `Content-Length` so clients don't need chunked
    /// decoding.
    fn build_response(status: u16, body: &str) -> String {
        build_response_with_type(status, "text/plain; charset=utf-8", body)
    }

    /// Build an HTTP/1.1 response with an explicit Content-Type.
    /// Connection is always closed after the response.
    fn build_response_with_type(status: u16, content_type: &str, body: &str) -> String {
        let reason = reason_phrase(status);
        format!(
            "HTTP/1.1 {status} {reason}\r\n\
             Content-Type: {content_type}\r\n\
             Content-Length: {len}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            len = body.len(),
        )
    }

    /// Parse the dynamic-handler's return string for an optional
    /// `Content-Type: <mime>\r\n\r\n` prefix. Returns the chosen
    /// content-type and the trailing body slice.
    ///
    /// This is the minimum-viable hook for SSE / JSON / HTML responses
    /// without inventing a richer return type. Handlers stay
    /// `(String) -> String` at the Canon level; the structure of the
    /// returned string is the only place we look for metadata.
    fn parse_handler_response(s: &str) -> (&str, &str) {
        const DEFAULT: &str = "text/plain; charset=utf-8";
        const PREFIX: &str = "Content-Type: ";
        if !s.starts_with(PREFIX) {
            return (DEFAULT, s);
        }
        // Find the `\r\n\r\n` separator between the header and body.
        let Some(sep_idx) = s.find("\r\n\r\n") else {
            return (DEFAULT, s);
        };
        let header_end = sep_idx;
        let body_start = sep_idx + 4;
        let header_value = s[PREFIX.len()..header_end].trim();
        if header_value.is_empty() {
            return (DEFAULT, s);
        }
        (header_value, &s[body_start..])
    }

    /// Reason phrases for the handful of statuses a demo server actually
    /// emits. Anything else falls back to a generic label — clients only
    /// care about the numeric code, so this is purely cosmetic.
    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "OK",
        }
    }
}

/// `canon:builtins/json` — JSON validation + primitive builders.
///
/// The stdlib type `Json = String` (in `canon/std/json.can`) is just the
/// JSON-encoded text. `parse` validates that a string is well-formed JSON
/// and returns the same string back as the `Json` handle on success.
/// The `from-*` builders emit the JSON text for a single primitive value,
/// handling string escaping and the special-case spellings (`null`,
/// `true`, `false`). Object / array construction lives entirely in Canon
/// — the stdlib wrapper builds those via `String.concat` from the
/// primitive builders.
///
/// Hand-rolled (no `serde_json` dep) to keep the compiler's runtime
/// dependency surface minimal and to match the existing
/// `canon:builtins/url` validator style.
mod host_builtin_json {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface json {
                /// Validate that `input` is well-formed JSON. On success,
                /// returns the same string back (so it can be threaded
                /// through as a `Json` value). On failure, returns a
                /// diagnostic message naming the byte offset of the error.
                parse: func(input: string) -> result<string, string>;

                /// Render a string as a JSON string literal: escape
                /// `\\`, `\"`, control characters, and wrap in double
                /// quotes.
                from-string: func(value: string) -> string;

                /// Render a 64-bit signed integer as a JSON number.
                from-int: func(value: s64) -> string;

                /// Render a 64-bit float as a JSON number. NaN and ±Inf
                /// (which JSON cannot represent) are emitted as `null`.
                from-float: func(value: f64) -> string;

                /// Render a bool as `true` or `false`. The parameter
                /// is `s32` rather than `bool` because Canon's codegen
                /// lowers `Bool` as a flat i32 (0 = False, non-zero =
                /// True) and the canonical-ABI shape for `bool` doesn't
                /// line up with that. Same workaround as
                /// `canon:builtins/cli#exit-with-code`.
                from-bool: func(value: s32) -> string;

                /// Return the literal `null`.
                from-null: func() -> string;

                /// Extract a field's value from a JSON object. `input`
                /// is the JSON text of an object, `name` the field's
                /// unquoted key. On success, returns the field's value
                /// as JSON text (still a `Json` handle, ready to be
                /// re-parsed). On failure (input isn't an object, or
                /// the field is missing), returns a diagnostic message.
                ///
                /// This is the primitive read-side counterpart to the
                /// `from-*` builders — it lets pure-Canon code walk a
                /// parsed JSON tree without owning a per-type parser.
                field: func(input: string, name: string) -> result<string, string>;

                /// Decode a JSON string value into its unquoted contents.
                /// `input` must be JSON text whose top-level value is a
                /// string literal; anything else returns a diagnostic
                /// message. Inverse of `from-string`: escape sequences
                /// like backslash-n become real newlines.
                as-string: func(input: string) -> result<string, string>;
            }
            world host-shim {
                import json;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::json::Host for State {
        fn parse(&mut self, input: String) -> Result<String, String> {
            match validate_json(&input) {
                Ok(()) => Ok(input),
                Err(msg) => Err(msg),
            }
        }

        fn from_string(&mut self, value: String) -> String {
            json_escape_string(&value)
        }

        fn from_int(&mut self, value: i64) -> String {
            value.to_string()
        }

        fn from_float(&mut self, value: f64) -> String {
            if value.is_nan() || value.is_infinite() {
                // JSON has no spelling for these — null is the
                // serde-compatible fallback.
                "null".to_string()
            } else {
                // `to_string` on f64 produces a shortest-round-trip
                // decimal, which is always valid JSON.
                value.to_string()
            }
        }

        fn from_bool(&mut self, value: i32) -> String {
            if value != 0 { "true" } else { "false" }.to_string()
        }

        fn from_null(&mut self) -> String {
            "null".to_string()
        }

        fn field(&mut self, input: String, name: String) -> Result<String, String> {
            extract_field(&input, &name)
        }

        fn as_string(&mut self, input: String) -> Result<String, String> {
            decode_string(&input)
        }
    }

    /// Walk a JSON object and return the raw JSON text of `name`'s value,
    /// preserving its enclosing syntax (strings stay quoted, objects stay
    /// braced, etc.) so the caller can re-parse or pass it on as a `Json`
    /// value.
    ///
    /// Errors when the input isn't a JSON object, the field isn't found,
    /// or the input is malformed in a way that makes navigation
    /// unambiguous. The error message names the byte offset for parity
    /// with `parse`.
    fn extract_field(input: &str, name: &str) -> Result<String, String> {
        let bytes = input.as_bytes();
        let mut p = Parser { src: bytes, pos: 0 };
        p.skip_ws();
        if p.peek() != Some(b'{') {
            return Err(format!(
                "expected object at byte {}, got {:?}",
                p.pos,
                p.peek().map(|c| c as char)
            ));
        }
        p.pos += 1; // consume '{'
        p.skip_ws();
        if p.peek() == Some(b'}') {
            return Err(format!("field `{}` not found", name));
        }
        loop {
            p.skip_ws();
            if p.peek() != Some(b'"') {
                return Err(format!("expected string key at byte {}", p.pos));
            }
            let key_start = p.pos;
            p.string()?;
            let key_end = p.pos;
            // The key in the source is the inner unescaped slice between
            // the quotes. We compare against `name` literally — no
            // unescaping. For ASCII keys (the common case) this is
            // correct; for keys containing escapes the caller can decode
            // their own key before calling.
            let key_slice = &input[key_start + 1..key_end - 1];
            p.skip_ws();
            if p.bump() != Some(b':') {
                return Err(format!(
                    "expected ':' after key at byte {}",
                    p.pos.saturating_sub(1)
                ));
            }
            p.skip_ws();
            let value_start = p.pos;
            p.value()?;
            let value_end = p.pos;
            if key_slice == name {
                return Ok(input[value_start..value_end].to_string());
            }
            p.skip_ws();
            match p.bump() {
                Some(b',') => continue,
                Some(b'}') => return Err(format!("field `{}` not found", name)),
                _ => {
                    return Err(format!(
                        "expected ',' or '}}' at byte {}",
                        p.pos.saturating_sub(1)
                    ));
                }
            }
        }
    }

    /// Decode a JSON string literal (e.g. `"hello\\nworld"`) into its raw
    /// contents (e.g. `hello\nworld`). Mirrors the inverse of
    /// `from_string`. Errors when the input isn't a JSON string value or
    /// contains a malformed escape.
    fn decode_string(input: &str) -> Result<String, String> {
        let bytes = input.as_bytes();
        let mut p = Parser { src: bytes, pos: 0 };
        p.skip_ws();
        if p.peek() != Some(b'"') {
            return Err(format!(
                "expected string at byte {}, got {:?}",
                p.pos,
                p.peek().map(|c| c as char)
            ));
        }
        let start = p.pos;
        p.string()?; // validates the string syntax and advances past closing `"`
        let end = p.pos;
        // Strip the surrounding quotes and decode escapes.
        let inner = &input[start + 1..end - 1];
        unescape_json_string(inner)
    }

    /// Decode the body of a JSON string literal (no surrounding quotes).
    /// Caller has already validated the escape syntax via `Parser::string`,
    /// so we can assume well-formedness and focus on the byte mapping.
    fn unescape_json_string(s: &str) -> Result<String, String> {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c != b'\\' {
                out.push(c as char);
                i += 1;
                continue;
            }
            i += 1;
            if i >= bytes.len() {
                return Err("truncated escape".to_string());
            }
            match bytes[i] {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\x08'),
                b'f' => out.push('\x0c'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    if i + 4 >= bytes.len() {
                        return Err("truncated \\u escape".to_string());
                    }
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 5])
                        .map_err(|_| "bad \\u escape".to_string())?;
                    let code =
                        u32::from_str_radix(hex, 16).map_err(|_| "bad \\u escape".to_string())?;
                    // Push as a single char when in the BMP; surrogate
                    // pairs aren't combined here — a \uD800..\uDFFF code
                    // unit is emitted as the Unicode replacement
                    // character to keep the result valid UTF-8. Strings
                    // built via `from_string` never produce surrogate
                    // escapes, so this only bites on hand-written JSON.
                    out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                    i += 5;
                    continue;
                }
                other => return Err(format!("bad escape \\{}", other as char)),
            }
            i += 1;
        }
        Ok(out)
    }

    /// Escape a Rust `&str` as a JSON string literal (including the
    /// surrounding double quotes). Mirrors the `serde_json::to_string`
    /// behaviour for plain strings.
    fn json_escape_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\x08' => out.push_str("\\b"),
                '\x0c' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// Hand-rolled recursive-descent JSON validator. Walks the input
    /// once without building a tree and returns `Ok(())` iff the entire
    /// input is a single well-formed JSON value (possibly surrounded by
    /// whitespace). The error string names the byte offset for fast
    /// diagnosis.
    fn validate_json(s: &str) -> Result<(), String> {
        let mut p = Parser {
            src: s.as_bytes(),
            pos: 0,
        };
        p.skip_ws();
        p.value()?;
        p.skip_ws();
        if p.pos != p.src.len() {
            return Err(format!("unexpected trailing characters at byte {}", p.pos));
        }
        Ok(())
    }

    struct Parser<'a> {
        src: &'a [u8],
        pos: usize,
    }

    impl<'a> Parser<'a> {
        fn skip_ws(&mut self) {
            while let Some(c) = self.peek() {
                if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }

        fn peek(&self) -> Option<u8> {
            self.src.get(self.pos).copied()
        }

        fn bump(&mut self) -> Option<u8> {
            let c = self.peek()?;
            self.pos += 1;
            Some(c)
        }

        fn value(&mut self) -> Result<(), String> {
            self.skip_ws();
            let c = self
                .peek()
                .ok_or_else(|| format!("unexpected end of input at byte {}", self.pos))?;
            match c {
                b'{' => self.object(),
                b'[' => self.array(),
                b'"' => self.string(),
                b't' | b'f' => self.boolean(),
                b'n' => self.null(),
                b'-' | b'0'..=b'9' => self.number(),
                other => Err(format!(
                    "unexpected character {:?} at byte {}",
                    other as char, self.pos
                )),
            }
        }

        fn object(&mut self) -> Result<(), String> {
            self.pos += 1; // consume '{'
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(());
            }
            loop {
                self.skip_ws();
                if self.peek() != Some(b'"') {
                    return Err(format!("expected string key at byte {}", self.pos));
                }
                self.string()?;
                self.skip_ws();
                if self.bump() != Some(b':') {
                    return Err(format!(
                        "expected ':' after key at byte {}",
                        self.pos.saturating_sub(1)
                    ));
                }
                self.value()?;
                self.skip_ws();
                match self.bump() {
                    Some(b',') => continue,
                    Some(b'}') => return Ok(()),
                    _ => {
                        return Err(format!(
                            "expected ',' or '}}' at byte {}",
                            self.pos.saturating_sub(1)
                        ));
                    }
                }
            }
        }

        fn array(&mut self) -> Result<(), String> {
            self.pos += 1; // consume '['
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(());
            }
            loop {
                self.value()?;
                self.skip_ws();
                match self.bump() {
                    Some(b',') => continue,
                    Some(b']') => return Ok(()),
                    _ => {
                        return Err(format!(
                            "expected ',' or ']' at byte {}",
                            self.pos.saturating_sub(1)
                        ));
                    }
                }
            }
        }

        fn string(&mut self) -> Result<(), String> {
            self.pos += 1; // consume opening '"'
            loop {
                let start = self.pos;
                let c = self
                    .bump()
                    .ok_or_else(|| format!("unterminated string starting at byte {}", start))?;
                match c {
                    b'"' => return Ok(()),
                    b'\\' => {
                        let esc = self
                            .bump()
                            .ok_or_else(|| format!("unterminated escape at byte {}", self.pos))?;
                        match esc {
                            b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                            b'u' => {
                                for _ in 0..4 {
                                    let h = self.bump().ok_or_else(|| {
                                        format!("truncated \\u escape at byte {}", self.pos)
                                    })?;
                                    if !h.is_ascii_hexdigit() {
                                        return Err(format!(
                                            "bad hex digit in \\u escape at byte {}",
                                            self.pos.saturating_sub(1)
                                        ));
                                    }
                                }
                            }
                            other => {
                                return Err(format!(
                                    "bad escape \\{:?} at byte {}",
                                    other as char,
                                    self.pos.saturating_sub(1)
                                ));
                            }
                        }
                    }
                    0x00..=0x1F => {
                        return Err(format!(
                            "unescaped control character at byte {}",
                            self.pos.saturating_sub(1)
                        ));
                    }
                    _ => {}
                }
            }
        }

        fn number(&mut self) -> Result<(), String> {
            if self.peek() == Some(b'-') {
                self.pos += 1;
            }
            match self.peek() {
                Some(b'0') => self.pos += 1,
                Some(b'1'..=b'9') => {
                    self.pos += 1;
                    while matches!(self.peek(), Some(b'0'..=b'9')) {
                        self.pos += 1;
                    }
                }
                _ => return Err(format!("expected digit at byte {}", self.pos)),
            }
            if self.peek() == Some(b'.') {
                self.pos += 1;
                if !matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(format!("expected digit after '.' at byte {}", self.pos));
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            if matches!(self.peek(), Some(b'e' | b'E')) {
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                if !matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(format!("expected digit in exponent at byte {}", self.pos));
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            Ok(())
        }

        fn boolean(&mut self) -> Result<(), String> {
            let rest = &self.src[self.pos..];
            if rest.starts_with(b"true") {
                self.pos += 4;
                Ok(())
            } else if rest.starts_with(b"false") {
                self.pos += 5;
                Ok(())
            } else {
                Err(format!("expected 'true' or 'false' at byte {}", self.pos))
            }
        }

        fn null(&mut self) -> Result<(), String> {
            let rest = &self.src[self.pos..];
            if rest.starts_with(b"null") {
                self.pos += 4;
                Ok(())
            } else {
                Err(format!("expected 'null' at byte {}", self.pos))
            }
        }
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        canon::builtins::json::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/url` — URL parsing. Validates the shape of a `Url`
/// string (must start with `http://` or `https://` and have a non-empty
/// host). Returns the same string back as the `Url` handle on success or a
/// diagnostic message on failure.
mod host_builtin_url {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface url {
                parse: func(input: string) -> result<string, string>;
            }
            world host-shim {
                import url;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::url::Host for State {
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
        canon::builtins::url::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}
