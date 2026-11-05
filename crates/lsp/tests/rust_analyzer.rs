//! Live: rust-analyzer's colors, asked for whole and then as what
//! changed (it may answer with all of them anyway, as it has here). Needs `rust-analyzer` installed (`cargo test -p ghostwrite-lsp -- --ignored`).

use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use ghostwrite_lsp::{Document, Registry, convert};
use serde_json::Value;

fn answer(mut rx: oneshot::Receiver<Result<Value, String>>) -> Value {
    let start = Instant::now();
    loop {
        if let Some(v) = rx.try_recv().unwrap() {
            return v.unwrap();
        }
        assert!(start.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore]
fn colors_asked_for_as_changes_match_the_whole() {
    let root = std::env::temp_dir().join(format!("ghostwrite-ra-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let main = root.join("src/main.rs");
    let text = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    std::fs::write(&main, text).unwrap();
    let mut servers = Registry::new(root.clone(), mpsc::unbounded().0);
    let mut doc = Document::open(servers.start(&["rust-analyzer"], &main).expect("rust-analyzer installed"), "rust", &main, text);

    // Until it's loaded the project, its colors change by themselves.
    let mut last = vec![];
    let (id, data) = loop {
        // (Nothing at all at first.)
        let (id, data) = convert::token_data(&answer(doc.semantic_tokens(None)), None).unwrap_or_default();
        if data == last && !data.is_empty() {
            break (id, data);
        }
        last = data;
        std::thread::sleep(Duration::from_millis(500));
    };
    let legend = doc.semantic_legend().expect("rust-analyzer has semantic tokens");
    let tokens = convert::semantic_tokens(&data, text, &legend);
    assert!(tokens.iter().any(|(r, s)| &text[r.clone()] == "x" && *s == "variable"), "{tokens:?}");

    // Edit, ask for what changed, twice: the same as asking for all of it.
    let (mut id, mut data) = (id, data);
    for edited in [text.replace("let x", "let mut x"), text.replace("let x = 1", "let mut x = 1.0")] {
        doc.changed(&ghostwrite_core::Rope::from_str(&edited), None);
        let changed = answer(doc.semantic_tokens(id.as_deref()));
        (id, data) = convert::token_data(&changed, Some(&data)).expect("tokens");
        let all = convert::token_data(&answer(doc.semantic_tokens(None)), None).expect("tokens").1;
        assert_eq!(data, all);
    }
    std::fs::remove_dir_all(root).ok();
}
