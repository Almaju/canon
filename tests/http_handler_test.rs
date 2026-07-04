//! End-to-end integration test for the dynamic HTTP handler ABI.
//!
//! Spawns a child `canon run` process that binds an HTTP server with
//! a `handleRequest = (String) -> String` user function defined, then
//! opens a TCP connection to it, sends an HTTP request, and asserts
//! that the response body matches what the handler returned.
//!
//! This is the smoke test for `DYNAMIC-HANDLERS.md` slice 1: it
//! proves the callback ABI works end-to-end (compiler synthesises
//! the wrapper, component exports it, host runtime looks it up,
//! calls it per request).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Ports used by the tests. Chosen high to avoid clashing with dev
/// services. Each test uses a different port so they can be run in
/// parallel without binding the same port.
const TEST_PORT: u16 = 38421;
const TEST_PORT_SSE: u16 = 38422;

#[test]
fn dynamic_handler_round_trip() {
    let workdir = std::env::temp_dir().join(format!("canon_http_test_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let ow_path = workdir.join("handler.can");
    std::fs::write(
        &ow_path,
        format!(
            r#"handleRequest = (String) -> String {{
    "echoed via dynamic handler"
}}

main = () -> Result<Unit, IoError> {{
    HttpServer(Port({port}))
        .post(HttpStatus(200), RoutePath("/"), "unused")
        .serve()
}}
"#,
            port = TEST_PORT
        ),
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&ow_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run`");

    // Poll the port until it accepts connections. Cap at ~5s.
    let start = Instant::now();
    let addr = format!("127.0.0.1:{}", TEST_PORT);
    let mut bound = false;
    while start.elapsed() < Duration::from_secs(5) {
        if TcpStream::connect(&addr).is_ok() {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !bound {
        let _ = child.kill();
        let out = child.wait_with_output().ok();
        let (stdout_s, stderr_s) = out
            .map(|o| {
                (
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                )
            })
            .unwrap_or_default();
        panic!(
            "server did not bind to {} within 5s\n  stdout: {}\n  stderr: {}",
            addr, stdout_s, stderr_s
        );
    }

    // Send a minimal HTTP request and read the full response.
    let result = (|| -> Result<String, std::io::Error> {
        let mut stream = TcpStream::connect(&addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
        )?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    })();

    // Tear down before asserting, so a failure doesn't leak a server.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);

    let response = result.expect("HTTP exchange failed");
    assert!(
        response.contains("echoed via dynamic handler"),
        "expected handler output in response; got:\n{}",
        response
    );
}

/// Companion smoke test for the SSE/Content-Type override pathway (Gap
/// 6 MVP from `DYNAMIC-HANDLERS.md`): when the guest handler's
/// returned string starts with `Content-Type: <mime>\r\n\r\n`, the
/// host honours that as the response's Content-Type. Proves that
/// `text/event-stream` flows end-to-end with a single payload.
#[test]
fn dynamic_handler_sse_content_type() {
    let workdir = std::env::temp_dir().join(format!("canon_http_sse_test_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let ow_path = workdir.join("handler.can");
    std::fs::write(
        &ow_path,
        format!(
            r#"handleRequest = (String) -> String {{
    "Content-Type: text/event-stream\r\n\r\ndata: hello\n\ndata: world\n\n"
}}

main = () -> Result<Unit, IoError> {{
    HttpServer(Port({port}))
        .post(HttpStatus(200), RoutePath("/"), "unused")
        .serve()
}}
"#,
            port = TEST_PORT_SSE
        ),
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&ow_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run`");

    let start = Instant::now();
    let addr = format!("127.0.0.1:{}", TEST_PORT_SSE);
    let mut bound = false;
    while start.elapsed() < Duration::from_secs(5) {
        if TcpStream::connect(&addr).is_ok() {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !bound {
        let _ = child.kill();
        let out = child.wait_with_output().ok();
        let (so, se) = out
            .map(|o| {
                (
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                )
            })
            .unwrap_or_default();
        panic!("sse server did not bind\n  stdout: {so}\n  stderr: {se}");
    }

    let result = (|| -> Result<String, std::io::Error> {
        let mut stream = TcpStream::connect(&addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(
            b"GET /events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n",
        )?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    })();

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);

    let response = result.expect("HTTP exchange failed");
    assert!(
        response.contains("Content-Type: text/event-stream"),
        "expected event-stream content type in response; got:\n{}",
        response
    );
    assert!(
        response.contains("data: hello"),
        "expected SSE event payload in response; got:\n{}",
        response
    );
}
