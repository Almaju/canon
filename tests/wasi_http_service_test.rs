//! End-to-end integration test for the `wasi:http/service` world
//! (WASI-HTTP-HANDLER.md slice 1b).
//!
//! A Canon program whose entry is a free `(Request) -> Response`
//! function compiles to a standard WebAssembly component exporting
//! `wasi:http/handler@0.3.0-rc-2026-03-15#handle` — the same export
//! `wasmtime serve` and any compliant WASI HTTP host instantiate. This
//! test drives the whole pipeline: compile the program, serve it via
//! `canon run --addr`, send a real HTTP request over TCP, and assert
//! the response status.
//!
//! The slice-1b contract is deliberately minimal: the handler ignores
//! the request and returns an empty 200. Request introspection and
//! response composition are slices 2–3; when they land, this test
//! grows assertions on echoed request data.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Distinct from the ports in `http_handler_test.rs` so the test
/// binaries can run in parallel.
const TEST_PORT: u16 = 38431;

#[test]
fn wasi_http_service_smoke() {
    let workdir = std::env::temp_dir().join(format!("canon_wasi_http_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("service.can");
    std::fs::write(
        &src_path,
        r#"use canon/std/http/Headers
use canon/std/http/Request
use canon/std/http/Response
use canon/std/http/Status

home = (Request) -> Response {
    Response(Headers(), Status(201))
}
"#,
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let addr = format!("127.0.0.1:{TEST_PORT}");
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .arg("--addr")
        .arg(&addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run --addr`");

    // Poll the port until it accepts connections. Cap at ~10s (the
    // first request also pays component compilation inside wasmtime).
    let start = Instant::now();
    let mut bound = false;
    while start.elapsed() < Duration::from_secs(10) {
        if TcpStream::connect(&addr).is_ok() {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !bound {
        let _ = child.kill();
        let out = child.wait_with_output().ok();
        let diag = out
            .map(|o| {
                format!(
                    "stdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                )
            })
            .unwrap_or_default();
        panic!("server never bound {addr}\n{diag}");
    }

    // Two sequential requests on separate connections: the second one
    // guards against per-request state corruption (each request leaks
    // one trailers-future writer by design; that must not affect
    // subsequent requests). The 201 pins the static-status extraction
    // from the handler body (`Status(201)` above), not the
    // `response.new` default.
    for attempt in 1..=2 {
        let response = send_request(&addr).unwrap_or_else(|e| {
            let _ = child.kill();
            panic!("request {attempt} failed: {e}");
        });
        assert!(
            response.starts_with("HTTP/1.1 201"),
            "request {attempt}: expected HTTP 201, got:\n{response}"
        );
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);
}

fn send_request(addr: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}
