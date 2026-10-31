//! Live: Lean through the client, as the editor drives it: its goals at
//! the cursor, `#eval`'s output, its progress, and its colors. Needs `lake` (elan)
//! installed (`cargo test -p ghostwrite-lsp -- --ignored`).

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
        assert!(start.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn answer(mut rx: oneshot::Receiver<Result<Value, String>>) -> Value {
    wait(|| rx.try_recv().unwrap()).unwrap()
}

#[test]
#[ignore]
fn lean_shows_goals_and_output_and_progress() {
    let root = std::env::temp_dir().join(format!("ghostwrite-lean-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let main = root.join("Main.lean");
    let text = "theorem t (n : Nat) : n + 0 = n := by\n  rfl\n\n#eval 6 * 7\n";
    std::fs::write(&main, text).unwrap();

    let (tx, mut events) = mpsc::unbounded();
    let mut servers = Registry::new(root.clone(), tx);
    let mut doc = Document::open(servers.start(&["lean"], &main).expect("lake installed"), "lean", &main, text);

    let goal = answer(doc.ask("$/lean/plainGoal", text, text.find("rfl").unwrap()));
    let goals: Vec<&str> = goal["goals"].as_array().expect("goals").iter().filter_map(Value::as_str).collect();
    assert_eq!(goals, ["n : Nat\n⊢ n + 0 = n"]);

    let (mut output, mut done) = (false, false);
    wait(|| {
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Diagnostics { diagnostics, .. } => output |= diagnostics.iter().any(|d| d.message == "42"),
                Event::Progress { checking: None, .. } => done = true,
                _ => {}
            }
        }
        (output && done).then_some(())
    });

    // Its colors: what each name is, by Lean's own parser.
    let legend = doc.semantic_legend().expect("Lean has semantic tokens");
    let (id, data) = convert::token_data(&answer(doc.semantic_tokens(None)), None).expect("tokens");
    let tokens = convert::semantic_tokens(&data, text, &legend);
    let at = |word: &str| tokens.iter().find(|(r, _)| &text[r.clone()] == word).map(|(_, s)| *s);
    assert_eq!(at("theorem"), Some("keyword"));
    assert_eq!(at("n"), Some("variable"));

    // After an edit, the same as all of it (Lean sends all of it; rust-analyzer only what changed).
    // Sent as an edit (Lean takes them): its goals are of the edited text.
    let mut buffer = ghostwrite_core::Buffer::new(text);
    let at = text.find("n : Nat").unwrap();
    buffer.edit(at..at, "m ");
    let edited = buffer.text().to_string();
    doc.changed(&edited, buffer.take_edits());
    let goal = answer(doc.ask("$/lean/plainGoal", &edited, edited.find("rfl").unwrap()));
    assert_eq!(goal["goals"][0], "m n : Nat\n⊢ n + 0 = n", "Lean has the edit");
    let all = convert::token_data(&answer(doc.semantic_tokens(None)), None).expect("tokens").1;
    let changed = convert::token_data(&answer(doc.semantic_tokens(id.as_deref())), Some(&data)).expect("tokens").1;
    assert_eq!(changed, all);
    std::fs::remove_dir_all(root).ok();
}
