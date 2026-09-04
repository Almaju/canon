//! `Url -> Fetched?` reaches the network through `wasi:http/client`:
//! the component imports the standard interface, and the embedded
//! runtime's default sender carries the request.
//!
//! A `Request => Response` program served by `canon run --addr` is the
//! peer, so the round trip is real TCP without leaving the machine: a
//! GET whose body echoes the path, a POST that answers 201, and a path
//! that answers 404 — the status the `HttpError` carries.

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Distinct from every port the other test binaries bind.
const PORT: u16 = 38441;

fn canon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_canon"))
}

fn workdir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("http-client-test");
    std::fs::create_dir_all(&dir).expect("create workdir");
    dir
}

/// Serve `source` on `PORT`; returns once the port accepts connections.
fn serve(path: &Path, source: &str) -> Child {
    std::fs::write(path, source).expect("write server");
    let addr = format!("127.0.0.1:{PORT}");
    let mut child = canon()
        .args(["run", path.to_str().expect("utf-8 path"), "--addr", &addr])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `canon run --addr`");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if TcpStream::connect(&addr).is_ok() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let out = child.wait_with_output().expect("server output");
    panic!(
        "server never bound {addr}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const SERVER: &str = r#"Request => Response {
    Request.method() -> (
        * "POST" {
            Request.path() -> (
                * None { Body("no path") -> Response(Headers() * Status(400)) }
                * Some<String> { Body(`posted {String}`) -> Response(Headers() * Status(201)) }
            )
        }
        * String {
            Request.path() -> (
                * None { Body("no path") -> Response(Headers() * Status(400)) }
                * Some<String> {
                    String -> (
                        * "/missing" { Body("nothing here") -> Response(Headers() * Status(404)) }
                        * String { Body(`got {String}`) -> Response(Headers() * Status(200)) }
                    )
                }
            )
        }
    )
}
"#;

#[test]
fn fetched_talks_to_a_wasi_http_service() {
    let dir = workdir();
    let mut server = serve(&dir.join("server.can"), SERVER);
    let client = dir.join("client");
    std::fs::create_dir_all(&client).expect("create client dir");
    let program = format!(
        "Unit => Program {{
    Url(\"http://127.0.0.1:{PORT}/hello?x=1\")? -> Fetched? -> Print
    Body(\"data\")
        -> Fetched(
            Method(\"POST\")
            * RequestHeaders(\"content-type: text/plain\")
            * Url(\"http://127.0.0.1:{PORT}/in\")?
        )?
        -> Print
    Url(\"http://127.0.0.1:{PORT}/missing\")? -> Fetched -> (
        * Err<HttpError> {{ HttpError -> Print }}
        * Ok<Fetched> {{ Fetched -> Print }}
    )
}}
"
    );
    let main = client.join("main.can");
    std::fs::write(&main, program).expect("write client");
    let fix = canon()
        .args(["check", "--fix", main.to_str().expect("utf-8 path")])
        .output()
        .expect("canon check --fix");
    assert!(
        fix.status.success(),
        "client program does not check:\n{}{}",
        String::from_utf8_lossy(&fix.stdout),
        String::from_utf8_lossy(&fix.stderr)
    );
    let out = canon()
        .args(["run", main.to_str().expect("utf-8 path")])
        .output()
        .expect("canon run");
    let _ = server.kill();
    let _ = server.wait();
    assert!(
        out.status.success(),
        "canon run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "got /hello?x=1\nposted /in\nHTTP 404: nothing here\n"
    );
    let _ = std::io::stdout().flush();
}
