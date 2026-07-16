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
//! no `canon:host/console` bridge is registered. The few remaining
//! `canon:builtins/*` bridges are genuine host boundaries (HTTP,
//! filesystem, float formatting) or extern/async-ABI test fixtures —
//! string processing that used to live here (URL validation, JSON
//! escaping / field extraction / decoding, RFC-3339 formatting, case
//! mapping) is pure Canon in `canon/std` now.

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
    http_hooks: CanonHttpHooks,
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
struct CanonHttpHooks;

impl WasiHttpHooks for CanonHttpHooks {
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
            http_hooks: CanonHttpHooks,
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
    let runtime = build_tokio_runtime();

    let code = runtime.block_on(async move {
        let (engine, linker) = match build_engine_and_linker() {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("error: {err:?}");
                return 1;
            }
        };
        match run_cli_component(&engine, &linker, bytes, args).await {
            Ok(code) => code,
            Err(err) => {
                eprintln!("error: {err:?}");
                1
            }
        }
    });
    // Exit code 0 falls through so callers finish normally; any non-zero
    // status terminates the process, mirroring the guest's request.
    if code != 0 {
        std::process::exit(code);
    }
}

/// Run many CLI components on a single shared engine + linker + tokio
/// runtime, in order. Each component gets a fresh `Store` (its own WASI
/// context) but reuses the compiled host `Linker` and the `Engine`'s
/// code cache, so N components pay the fixed runtime/engine/linker setup
/// once instead of N times. This is what backs `canon test <dir>`:
/// running every `*_test.can` file in one process rather than spawning a
/// subprocess per file.
///
/// Each element is `(header, bytes)` — `header` is printed (and flushed)
/// before the component runs so its guest output appears underneath.
/// Returns the number of components that finished with a non-zero exit
/// code (a guest `exit-with-code(n)`, a `wasi:cli/run` error result, or
/// a trap).
pub fn run_components(components: &[(String, Vec<u8>)]) -> usize {
    use std::io::Write;

    let runtime = build_tokio_runtime();
    runtime.block_on(async move {
        let (engine, linker) = match build_engine_and_linker() {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("error: {err:?}");
                return components.len();
            }
        };

        let mut failures = 0;
        for (header, bytes) in components {
            println!("{header}");
            // The guest inherits fd 1 directly (`inherit_stdio`), so flush
            // the host-side header first to keep output ordered.
            let _ = std::io::stdout().flush();
            let code = match run_cli_component(&engine, &linker, bytes, &[]).await {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("error: {err:?}");
                    1
                }
            };
            if code != 0 {
                failures += 1;
            }
        }
        failures
    })
}

/// The multi-thread tokio runtime both run paths need — async-stackful
/// component instantiation is only available on the async API.
fn build_tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: could not start tokio runtime: {e}");
            std::process::exit(1);
        })
}

/// Build the shared `Engine` and its populated `Linker` together. Held
/// once per process and reused across every component in a batch.
fn build_engine_and_linker() -> wasmtime::Result<(Engine, Linker<State>)> {
    let engine = build_engine()?;
    let linker = build_linker(&engine)?;
    Ok((engine, linker))
}

/// Instantiate `bytes` on the shared engine/linker and drive
/// `wasi:cli/run.run`, returning a process-style exit code: `0` on a
/// clean run, `n` when the guest requested `exit-with-code(n)`, and `1`
/// when `run` returned its error result. Instantiation and non-exit
/// traps propagate as `Err`.
///
/// Instantiating via the linker directly and driving `run` ourselves
/// mirrors what the bindgen-generated `Command` does internally (it
/// keeps its inner `Instance` private).
async fn run_cli_component(
    engine: &Engine,
    linker: &Linker<State>,
    bytes: &[u8],
    args: &[&str],
) -> wasmtime::Result<i32> {
    let mut store = Store::new(engine, build_state(args));
    let component = Component::new(engine, bytes)
        .map_err(|e| wasmtime::Error::msg(format!("invalid wasm component: {e:?}")))?;

    let instance = linker.instantiate_async(&mut store, &component).await?;

    // Look up `wasi:cli/run.run` and call it as an async-stackful
    // function returning `result<_, _>`.
    let run_iface_idx = instance
        .get_export_index(&mut store, None, WASI_CLI_RUN)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing {WASI_CLI_RUN} export")))?;
    let run_fn_idx = instance
        .get_export_index(&mut store, Some(&run_iface_idx), "run")
        .ok_or_else(|| wasmtime::Error::msg("missing wasi:cli/run.run export"))?;
    let run_func: wasmtime::component::TypedFunc<(), (Result<(), ()>,)> =
        instance.get_typed_func(&mut store, run_fn_idx)?;

    // A guest `wasi:cli/exit#exit(-with-code)` call surfaces as an
    // `I32Exit` trap during the call — that's a normal termination
    // request, not an error, so map it to its code instead of
    // propagating. `and_then` collapses the run-concurrent Result and the
    // inner call Result into one layer so either can carry the trap.
    let outcome = store
        .run_concurrent(async move |store| run_func.call_concurrent(store, ()).await)
        .await
        .and_then(|inner| inner);
    match outcome {
        Ok((Ok(()),)) => Ok(0),
        Ok((Err(()),)) => Ok(1),
        Err(err) => match err.downcast::<wasmtime_wasi::I32Exit>() {
            Ok(exit) => Ok(exit.0),
            Err(err) => Err(err),
        },
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
    host_builtin_string::add_to_linker(&mut linker)?;
    host_builtin_filesystem::add_to_linker(&mut linker)?;
    host_builtin_http::add_to_linker(&mut linker)?;
    host_builtin_json::add_to_linker(&mut linker)?;

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
/// `web` is the fullstack mode: when a web bundle is given, GET requests
/// for the paths it owns (`/`, `/index.html`, `/canon-web.js`,
/// `/<stem>.wasm`) are answered from memory and every other request
/// dispatches to the guest — frontend and backend on one origin.
///
/// This blocks the calling thread until the listener is shut down
/// (currently never — there's no graceful-shutdown wiring yet; killing
/// the process is the supported way to stop). Errors during accept /
/// dispatch are logged to stderr but don't terminate the server; a
/// fatal bind error is fatal.
pub fn serve_component(
    bytes: &[u8],
    addr: std::net::SocketAddr,
    web: Option<crate::webhost::WebAssets>,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: could not start tokio runtime: {e}");
            std::process::exit(1);
        });

    runtime.block_on(async move {
        if let Err(err) = serve_component_async(bytes, addr, web).await {
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    });
}

async fn serve_component_async(
    bytes: &[u8],
    addr: std::net::SocketAddr,
    web: Option<crate::webhost::WebAssets>,
) -> wasmtime::Result<()> {
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
    let web = web.map(Arc::new);

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
        let web = web.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_connection(engine, service_pre, socket, web).await {
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
    web: Option<std::sync::Arc<crate::webhost::WebAssets>>,
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
        let web = web.clone();
        async move {
            // Fullstack mode: the web bundle owns its four paths; the
            // guest never sees them. Everything else is the backend's.
            if let Some(assets) = &web {
                if req.method() == http::Method::GET {
                    if let Some((mime, body)) = assets.get(req.uri().path()) {
                        return Ok(asset_response(mime, body));
                    }
                }
            }
            dispatch_request(service, store, req).await
        }
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
    // when `Stream<T>` response bodies land (streaming, not yet implemented),
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

/// A static-asset response for fullstack mode — the web bundle's answer
/// shaped like the guest's, so hyper serves both through one service.
fn asset_response(
    mime: &'static str,
    body: &[u8],
) -> http::Response<UnsyncBoxBody<Bytes, ErrorCode>> {
    use http_body_util::{BodyExt, Full};
    let body = Full::new(Bytes::copy_from_slice(body))
        .map_err(|never| match never {})
        .boxed_unsync();
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, mime)
        .body(body)
        .expect("static response builder shape is valid")
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
/// `min` survives only as the extern-binding test fixture (the `deps/`
/// resolution tests and `tests/runtime/extern.can` import it) — real
/// integer `Minimum` / `Maximum` are pure Canon in `canon/std/int.can`.
mod host_builtins {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface math {
                min: func(a: s64, b: s64) -> s64;
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
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        canon::builtins::math::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}

/// `canon:builtins/string` — async host echoes. String *transforms*
/// (case mapping, escaping) are pure Canon in `canon/std` now; the two
/// functions left here exist only to exercise the guest-side async
/// canonical-ABI call sequence from tests.
mod host_builtin_string {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface strings {
                echo: async func(input: string) -> string;
                slow-echo: async func(input: string) -> string;
            }
            world host-shim {
                import strings;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::strings::Host for State {}

    // Async functions go on the `HostWithStore` trait, with an
    // `Accessor<T, Self>` first arg instead of `&mut self`. These two
    // exist purely to exercise the guest-side async canonical-ABI call
    // sequence from tests.
    impl canon::builtins::strings::HostWithStore for HasSelf<State> {
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

/// `canon:builtins/json` — the one JSON builder Canon can't express:
/// float formatting. Everything else (validation, escaping, field
/// extraction, string decoding, int/bool rendering) is pure Canon in
/// `canon/std/json.can` now. Shortest-round-trip decimal rendering of an
/// f64 is genuinely numeric machinery (Grisu/Ryū territory), so it stays
/// a host bridge.
mod host_builtin_json {
    use super::State;
    use wasmtime::component::{HasSelf, Linker};

    wasmtime::component::bindgen!({
        inline: "
            package canon:builtins@0.1.0;
            interface json {
                /// Render a 64-bit float as a JSON number. NaN and ±Inf
                /// (which JSON cannot represent) are emitted as `null`.
                from-float: func(value: f64) -> string;
            }
            world host-shim {
                import json;
            }
        ",
        require_store_data_send: true,
    });

    impl canon::builtins::json::Host for State {
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
    }

    pub fn add_to_linker(linker: &mut Linker<State>) -> wasmtime::Result<()> {
        canon::builtins::json::add_to_linker::<_, HasSelf<State>>(linker, |state| state)
    }
}
