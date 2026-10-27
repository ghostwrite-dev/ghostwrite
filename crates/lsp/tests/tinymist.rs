//! Live: tinymist through the client, as the editor drives it. Needs
//! `tinymist` installed (`cargo test -p ghostwrite-lsp -- --ignored`).

use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use ghostwrite_lsp::{Document, Event, Preview, Registry, convert};
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
    let root = std::env::temp_dir().join(format!("ghostwrite-typst-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (main, other) = (root.join("main.typ"), root.join("other.typ"));
    std::fs::write(&other, "#let shout = 2\n").unwrap();
    let text = "#import \"other.typ\": shout\n#let greet = 1\n#greet #shout\n";
    std::fs::write(&main, text).unwrap();

    let (tx, mut events) = mpsc::unbounded();
    let mut servers = Registry::new(root.clone(), tx);
    let mut doc = Document::open(servers.start(&["tinymist"]).expect("tinymist installed"), "typst", &main, text);

    // gd within the file, and into another file.
    let at = text.rfind("#greet").unwrap() + 2;
    let (path, pos) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!((path.file_name().unwrap(), pos.line), (main.file_name().unwrap(), 1));
    let at = text.rfind("shout").unwrap() + 1;
    let (path, _) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!(path.file_name().unwrap(), "other.typ");

    // Completion after `#gr`, before saving anything.
    let typed = format!("{text}#gr");
    doc.changed(&typed);
    let items = convert::completions(answer(doc.ask("textDocument/completion", &typed, typed.len())), &typed, typed.len());
    assert!(items.iter().any(|c| c.text == "greet"), "{:?}", items.iter().map(|c| &c.label).collect::<Vec<_>>());

    // An error typed (not saved) is reported, and goes away once fixed.
    let broken = format!("{text}#nope\n");
    doc.changed(&broken);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { diagnostics, .. }) if diagnostics.iter().any(|d| d.message.contains("nope")) => Some(()),
        _ => None,
    });
    doc.changed(text);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { path, diagnostics }) if path.ends_with("main.typ") && diagnostics.is_empty() => Some(()),
        _ => None,
    });
}

#[test]
#[ignore]
fn tinymist_serves_a_preview_and_stops_it() {
    let root = std::env::temp_dir().join(format!("ghostwrite-preview-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let main = root.join("main.typ");
    let text = "= Title\n\nSome text.\n";
    std::fs::write(&main, text).unwrap();

    let mut servers = Registry::new(root.clone(), mpsc::unbounded().0);
    let doc = Document::open(servers.start(&["tinymist"]).expect("tinymist installed"), "typst", &main, text);
    let (tx, mut served) = mpsc::unbounded();
    let mut preview = Preview::start(&doc, move |url| tx.unbounded_send(url).unwrap()).expect("tinymist previews");
    let url = wait(|| served.try_recv().ok());
    let addr = url.strip_prefix("http://").expect("an http URL");
    let get = || {
        use std::io::{Read as _, Write as _};
        let mut conn = std::net::TcpStream::connect(addr).ok()?;
        write!(conn, "GET / HTTP/1.0\r\nHost: {addr}\r\n\r\n").ok()?;
        let mut page = String::new();
        conn.read_to_string(&mut page).ok()?;
        Some(page)
    };
    assert!(get().is_some_and(|page| page.starts_with("HTTP/1.1 200") || page.starts_with("HTTP/1.0 200")), "the page is served");
    preview.scroll(text, text.find("Some").unwrap());

    drop(preview);
    wait(|| get().is_none().then_some(()));
}
