//! Web target (`WEB-TARGET.md`): a program defining the Elm-triple
//! (`init` / `update` / `view`) compiles to a self-contained core
//! module plus the JS-host bundle. In the browser, `canon-web.js`
//! drives the ABI; this test drives the *same* ABI from wasmtime
//! (plain core module, no component wrapper) so the whole loop —
//! model init, message dispatch in `update`, HTML rendering in
//! `view` — is pinned without needing a browser in CI:
//!
//!   init()                       -> i64        opaque model
//!   update(model, msg_ptr, len)  -> i64
//!   view(model)                  -> (i32, i32) UTF-8 HTML
//!   alloc(size)                  -> i32
//!
//! The five stdout imports are stubbed no-ops, exactly like the JS
//! host's console stubs.

use std::path::PathBuf;
use std::process::Command;
use wasmtime::{Engine, Instance, Linker, Module, Store, TypedFunc};

const COUNTER_SRC: &str = r#"use canon/std/web/Html
use canon/std/web/Msg

Model = Int

init = () -> Model {
    Model(0)
}

update = (Model * String) -> Model {
    String.(
        * ("Decrement") -> Model { Model(Model.sub(1)) }
        * ("Increment") -> Model { Model(Model.add(1)) }
        * (String) -> Model { Model }
    )
}

view = (Model) -> Html {
    "Canon Counter"
        .h1()
        .concat(Msg("Decrement").button("-"))
        .concat(Model.String().span())
        .concat(Msg("Increment").button("+"))
        .div()
}
"#;

struct WebApp {
    store: Store<()>,
    instance: Instance,
    init: TypedFunc<(), i64>,
    update: TypedFunc<(i64, i32, i32), i64>,
    view: TypedFunc<i64, (i32, i32)>,
    alloc: TypedFunc<i32, i32>,
}

impl WebApp {
    fn load(wasm: &[u8]) -> WebApp {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).expect("web core module must load");
        let mut linker: Linker<()> = Linker::new(&engine);
        const STDOUT: &str = "wasi:cli/stdout@0.3.0-rc-2026-03-15";
        linker
            .func_wrap(STDOUT, "write-via-stream", |_: i32| -> i32 { 1 })
            .unwrap();
        linker
            .func_wrap(STDOUT, "[stream-new-0]write-via-stream", || -> i64 {
                (2i64 << 32) | 1
            })
            .unwrap();
        linker
            .func_wrap(
                STDOUT,
                "[stream-write-0]write-via-stream",
                |_: i32, _: i32, _: i32| -> i32 { 0 },
            )
            .unwrap();
        linker
            .func_wrap(
                STDOUT,
                "[stream-drop-writable-0]write-via-stream",
                |_: i32| {},
            )
            .unwrap();
        linker
            .func_wrap(
                STDOUT,
                "[future-drop-readable-1]write-via-stream",
                |_: i32| {},
            )
            .unwrap();
        let mut store = Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiation must succeed with only stdout stubs");
        let init = instance.get_typed_func(&mut store, "init").unwrap();
        let update = instance.get_typed_func(&mut store, "update").unwrap();
        let view = instance.get_typed_func(&mut store, "view").unwrap();
        let alloc = instance.get_typed_func(&mut store, "alloc").unwrap();
        WebApp {
            store,
            instance,
            init,
            update,
            view,
            alloc,
        }
    }

    fn init(&mut self) -> i64 {
        self.init.call(&mut self.store, ()).expect("init")
    }

    fn send(&mut self, model: i64, msg: &str) -> i64 {
        let ptr = self
            .alloc
            .call(&mut self.store, msg.len() as i32)
            .expect("alloc");
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .expect("memory export");
        memory
            .write(&mut self.store, ptr as usize, msg.as_bytes())
            .expect("msg write");
        self.update
            .call(&mut self.store, (model, ptr, msg.len() as i32))
            .expect("update")
    }

    fn render(&mut self, model: i64) -> String {
        let (ptr, len) = self.view.call(&mut self.store, model).expect("view");
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .expect("memory export");
        let mut buf = vec![0u8; len as usize];
        memory
            .read(&self.store, ptr as usize, &mut buf)
            .expect("html read");
        String::from_utf8(buf).expect("view must return UTF-8")
    }
}

/// `canon build` on a web program writes the three-file bundle; the
/// wasm drives the full Elm loop under wasmtime.
#[test]
fn web_counter_full_loop() {
    let workdir = std::env::temp_dir().join(format!("canon_web_target_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let src_path = workdir.join("counter.can");
    std::fs::write(&src_path, COUNTER_SRC).unwrap();

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let out = Command::new(&canon_bin)
        .arg("build")
        .arg(&src_path)
        .output()
        .expect("canon build must spawn");
    assert!(
        out.status.success(),
        "canon build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Bundle: <dir>/build/<stem>/{<stem>.wasm, canon-web.js, index.html}.
    let bundle = workdir.join("build").join("counter");
    for name in ["counter.wasm", "canon-web.js", "index.html"] {
        assert!(
            bundle.join(name).exists(),
            "expected bundle file `{name}` in {}",
            bundle.display()
        );
    }
    let index = std::fs::read_to_string(bundle.join("index.html")).unwrap();
    assert!(
        index.contains("canonWebStart(\"counter.wasm\""),
        "index.html must boot the app: {index}"
    );

    let wasm = std::fs::read(bundle.join("counter.wasm")).unwrap();
    let mut app = WebApp::load(&wasm);

    let model = app.init();
    let html = app.render(model);
    assert!(
        html.contains("<h1>Canon Counter</h1>"),
        "initial view must render the title, got: {html}"
    );
    assert!(
        html.contains("<span>0</span>"),
        "initial count must render 0, got: {html}"
    );
    assert!(
        html.contains("<button data-msg=\"Increment\">+</button>"),
        "buttons must carry data-msg attributes, got: {html}"
    );

    let model = app.send(model, "Increment");
    let model = app.send(model, "Increment");
    assert!(
        app.render(model).contains("<span>2</span>"),
        "two increments must render 2"
    );

    let model = app.send(model, "Decrement");
    let model = app.send(model, "Decrement");
    let model = app.send(model, "Decrement");
    assert!(
        app.render(model).contains("<span>-1</span>"),
        "negative counts must render with the sign (Int.String())"
    );

    // Unknown messages hit the catch-all arm and leave the model alone.
    let model = app.send(model, "Nonsense");
    assert!(
        app.render(model).contains("<span>-1</span>"),
        "unknown messages must be a no-op"
    );

    let _ = std::fs::remove_dir_all(&workdir);
}
