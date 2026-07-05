//! End-to-end integration test for the `wasi:http/service` world.
//!
//! A Canon program whose entry is a free `(Request) => Response`
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
/// Separate ports per test — the tests in this binary may run
/// concurrently.
const HEADERS_TEST_PORT: u16 = 38432;
const METHOD_TEST_PORT: u16 = 38433;

#[test]
fn wasi_http_service_smoke() {
    let workdir = std::env::temp_dir().join(format!("canon_wasi_http_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("service.can");
    std::fs::write(
        &src_path,
        r#"home = (Request) => Response {
    Response(Body("created: " -> Joined("ok")) * Headers() * Status(201))
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
    // guards against per-request state corruption. The 201 pins the
    // *runtime* status from the compiled handler body, and the body
    // assertion pins the string-body path (contents stream written
    // after `task.return`, then closed — a hung/chunked response fails
    // the read).
    for attempt in 1..=2 {
        let response = send_request(&addr).unwrap_or_else(|e| {
            let _ = child.kill();
            panic!("request {attempt} failed: {e}");
        });
        assert!(
            response.starts_with("HTTP/1.1 201"),
            "request {attempt}: expected HTTP 201, got:\n{response}"
        );
        assert!(
            response.ends_with("created: ok"),
            "request {attempt}: expected the concat-built body, got:\n{response}"
        );
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);
}

/// Response headers: `Headers().set(name, value)` chains compile to
/// real `[method]fields.append` calls, so the values reach the wire.
/// Pins the slice-2 headers wiring (previously `.set` silently
/// degraded to empty headers).
#[test]
fn wasi_http_service_response_headers() {
    let workdir = std::env::temp_dir().join(format!("canon_wasi_http_hdr_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("service.can");
    std::fs::write(
        &src_path,
        r#"home = (Request) => Response {
    Response(Body("<h1>hi</h1>") * Headers().set("content-type" * "text/html").set("x-canon" * "1") * Status(200))
}
"#,
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let addr = format!("127.0.0.1:{HEADERS_TEST_PORT}");
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .arg("--addr")
        .arg(&addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run --addr`");

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

    let response = send_request(&addr).unwrap_or_else(|e| {
        let _ = child.kill();
        panic!("request failed: {e}");
    });
    let _ = child.kill();
    let _ = child.wait();
    let head = response
        .split("\r\n\r\n")
        .next()
        .unwrap_or(&response)
        .to_ascii_lowercase();
    assert!(
        head.contains("content-type: text/html"),
        "expected content-type header from Headers().set, got:\n{response}"
    );
    assert!(
        head.contains("x-canon: 1"),
        "expected second chained header, got:\n{response}"
    );
    assert!(
        response.ends_with("<h1>hi</h1>"),
        "expected the html body, got:\n{response}"
    );
    let _ = std::fs::remove_dir_all(&workdir);
}

/// Request introspection: `Request.method()` surfaces the WIT `method`
/// variant as a plain `String`, so REST verbs route via literal
/// dispatch. Pins both the static-discriminant mapping (GET/POST) and
/// the catch-all arm receiving the method name (PUT via the interned
/// static string).
#[test]
fn wasi_http_service_method_dispatch() {
    let workdir = std::env::temp_dir().join(format!("canon_wasi_http_mth_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("service.can");
    std::fs::write(
        &src_path,
        r#"Request => Response {
    Request.method().(
        * "GET" => Response { Response(Body("got GET") * Headers() * Status(200)) }
        * "POST" => Response { Response(Body("got POST") * Headers() * Status(201)) }
        * String => Response { Response(Body("no " -> Joined(String)) * Headers() * Status(405)) }
    )
}
"#,
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let addr = format!("127.0.0.1:{METHOD_TEST_PORT}");
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&src_path)
        .arg("--addr")
        .arg(&addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run --addr`");

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
        panic!("server never bound {addr}");
    }

    for (verb, expected_status, expected_body) in [
        ("GET", "200", "got GET"),
        ("POST", "201", "got POST"),
        ("PUT", "405", "no PUT"),
    ] {
        let response = send_verb(&addr, verb).unwrap_or_else(|e| {
            let _ = child.kill();
            panic!("{verb} request failed: {e}");
        });
        assert!(
            response.starts_with(&format!("HTTP/1.1 {expected_status}")),
            "{verb}: expected {expected_status}, got:\n{response}"
        );
        assert!(
            response.ends_with(expected_body),
            "{verb}: expected body `{expected_body}`, got:\n{response}"
        );
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);
}

fn send_request(addr: &str) -> std::io::Result<String> {
    send_verb(addr, "GET")
}

fn send_verb(addr: &str, verb: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(
        format!("{verb} / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}
