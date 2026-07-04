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

const COUNTER_SRC: &str = r#"Model = Int

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

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The shipped `examples/todolist-web` app: build the real package,
/// drive the full add/toggle/delete/clear loop, and prove the
/// localStorage story — replaying the message log on a fresh instance
/// reproduces the exact same view (that is *how* the host persists,
/// since the model is a fold over messages).
#[test]
fn web_todolist_example_loop_and_replay() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("todolist-web");

    let canon_bin = PathBuf::from(env!("CARGO_BIN_EXE_canon"));
    let out = Command::new(&canon_bin)
        .arg("build")
        .arg(&example)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("canon build must spawn");
    assert!(
        out.status.success(),
        "canon build examples/todolist-web failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // `examples/` is a workspace, so members build to its shared
    // `examples/build/<stem>.{wasm}` plus the three-file web bundle.
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("build");
    let wasm_path = build.join("todolist-web.wasm");
    assert!(
        wasm_path.exists(),
        "expected {} after canon build",
        wasm_path.display()
    );

    // The generated shell must enable localStorage persistence, keyed
    // by the app's stem.
    let index = std::fs::read_to_string(build.join("index.html")).unwrap();
    assert!(
        index.contains("canonWebStart(\"todolist-web.wasm\"")
            && index.contains("\"canon:todolist-web\""),
        "index.html must boot the app with a persistence key: {index}"
    );

    let wasm = std::fs::read(&wasm_path).unwrap();

    // Messages the browser host would send, driving the whole feature
    // set. The same sequence is replayed below.
    let log = ["Add:buy milk", "Toggle:3", "Delete:1", "Clear"];

    let mut app = WebApp::load(&wasm);
    let mut model = app.init();
    let html = app.render(model);
    assert!(
        html.contains("<h1>Canon Todos</h1>"),
        "initial view must render the title: {html}"
    );
    assert_eq!(count(&html, "<li>"), 2, "two seed items: {html}");
    assert!(
        html.contains("data-msg-form=\"Add:\""),
        "add form must be present: {html}"
    );

    model = app.send(model, log[0]);
    let html = app.render(model);
    assert_eq!(count(&html, "<li>"), 3, "add appends an item: {html}");
    assert!(html.contains("buy milk"), "new item text present: {html}");

    model = app.send(model, log[1]);
    assert!(
        app.render(model).contains("<s>buy milk</s>"),
        "toggling the third item strikes it through"
    );

    model = app.send(model, log[2]);
    assert_eq!(
        count(&app.render(model), "<li>"),
        2,
        "delete removes an item"
    );

    model = app.send(model, log[3]);
    let live = app.render(model);
    assert!(
        !live.contains("<s>buy milk</s>"),
        "clear drops the completed item: {live}"
    );
    assert_eq!(
        count(&live, "<li>"),
        1,
        "one incomplete item remains: {live}"
    );

    // Persistence: a fresh instance that replays the saved message log
    // must land on the identical view — no model serialization needed.
    let mut replay = WebApp::load(&wasm);
    let mut rmodel = replay.init();
    for msg in log {
        rmodel = replay.send(rmodel, msg);
    }
    assert_eq!(
        replay.render(rmodel),
        live,
        "replaying the message log must reproduce the persisted view"
    );
}
