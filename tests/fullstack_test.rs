//! End-to-end integration test for fullstack packages.
//!
//! A directory with `src/web.can` (the Elm triple) and `src/server.can`
//! (`Request => Response`) in place of `src/main.can` is a fullstack
//! package: `canon run <dir> --addr` compiles both entries and serves
//! them from one process on one address — the web bundle owns `/`,
//! `/index.html`, `/canon-web.js`, and `/<name>.wasm`; every other
//! request dispatches to the server component. This test drives the
//! whole pipeline over real TCP and pins that routing split.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Distinct from the ports in the other server-test binaries so the
/// test binaries can run in parallel.
const TEST_PORT: u16 = 38434;

#[test]
fn fullstack_serves_bundle_and_backend_on_one_address() {
    let workdir = std::env::temp_dir().join(format!("canon_fullstack_{}", std::process::id()));
    let src = workdir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("web.can"),
        r#"Init = Model

Model = String

Update = Model

Model => Html {
    <p>{Model}</p>
}

Unit => Init {
    Model("hello") -> Init
}

Model * String => Update {
    String
        -> Model
        -> Update
}
"#,
    )
    .unwrap();
    std::fs::write(
        src.join("server.can"),
        r#"Request => Response {
    Body("api: ok") -> Response(Headers() * Status(200))
}
"#,
    )
    .unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let addr = format!("127.0.0.1:{TEST_PORT}");
    let mut child = Command::new(&canon_bin)
        .arg("run")
        .arg(&workdir)
        .arg("--addr")
        .arg(&addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canon run --addr` on a fullstack package");

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

    // The bundle's four paths come from memory: the shell, the host JS,
    // and the compiled web app (a core module, magic `\0asm`). The
    // package directory's name is the app's stem.
    let stem = workdir.file_name().unwrap().to_string_lossy().into_owned();
    for (path, marker) in [
        ("/".to_string(), "canonWebStart"),
        ("/index.html".to_string(), "canonWebStart"),
        ("/canon-web.js".to_string(), "canon-web.js"),
        (format!("/{stem}.wasm"), "\0asm"),
    ] {
        let response = get(&addr, &path).unwrap_or_else(|e| {
            let _ = child.kill();
            panic!("GET {path} failed: {e}");
        });
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "GET {path}: expected HTTP 200, got:\n{response}"
        );
        assert!(
            response.contains(marker),
            "GET {path}: expected body containing {marker:?}, got:\n{response}"
        );
    }

    // Every other path dispatches to the server component.
    let response = get(&addr, "/api").unwrap_or_else(|e| {
        let _ = child.kill();
        panic!("GET /api failed: {e}");
    });
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "GET /api: expected HTTP 200 from the guest, got:\n{response}"
    );
    assert!(
        response.ends_with("api: ok"),
        "GET /api: expected the guest body, got:\n{response}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&workdir);
}

fn get(addr: &str, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
