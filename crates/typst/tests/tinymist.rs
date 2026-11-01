//! Live: tinymist through the client, as the editor drives it. Needs
//! `tinymist` installed (`cargo test -p ghostwrite-lsp -- --ignored`).

use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use ghostwrite_lsp::{Document, Event, Registry, convert};
use serde_json::Value;

fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn answer(mut rx: oneshot::Receiver<Result<Value, String>>) -> Value {
    wait(|| rx.try_recv().unwrap()).unwrap()
}

#[test]
#[ignore]
fn tinymist_jumps_completes_and_rechecks_while_typing() {
    ghostwrite_plugin::install(|| vec![Box::new(ghostwrite_typst::Plugin)]);
    let root = std::env::temp_dir().join(format!("ghostwrite-typst-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (main, other) = (root.join("main.typ"), root.join("other.typ"));
    std::fs::write(&other, "#let shout = 2\n").unwrap();
    let text = "#import \"other.typ\": shout\n#let greet = 1\n#greet #shout\n";
    std::fs::write(&main, text).unwrap();

    let (tx, mut events) = mpsc::unbounded();
    let mut servers = Registry::new(root.clone(), tx);
    let mut doc = Document::open(servers.start(&["tinymist"], &main).expect("tinymist installed"), "typst", &main, text);

    // gd within the file, and into another file.
    let at = text.rfind("#greet").unwrap() + 2;
    let (path, pos) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!((path.file_name().unwrap(), pos.line), (main.file_name().unwrap(), 1));
    let at = text.rfind("shout").unwrap() + 1;
    let (path, _) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!(path.file_name().unwrap(), "other.typ");

    // Completion after `#gr`, before saving anything.
    let typed = format!("{text}#gr");
    doc.changed(&typed, None);
    let items = convert::completions(answer(doc.ask("textDocument/completion", &typed, typed.len())), &typed, typed.len());
    assert!(items.iter().any(|c| c.text == "greet"), "{:?}", items.iter().map(|c| &c.label).collect::<Vec<_>>());

    // An error typed (not saved) is reported, and goes away once fixed.
    let broken = format!("{text}#nope\n");
    doc.changed(&broken, None);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { diagnostics, .. }) if diagnostics.iter().any(|d| d.message.contains("nope")) => Some(()),
        _ => None,
    });
    doc.changed(text, None);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { path, diagnostics, .. }) if path.ends_with("main.typ") && diagnostics.is_empty() => Some(()),
        _ => None,
    });
}
