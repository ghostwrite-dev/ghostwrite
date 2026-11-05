//! Lean, as the plugin that comes with the editor (`plugins/lean`): its colors, its
//! abbreviations, its goals beside a file, and, live, Lean itself through the client,
//! as the editor drives it. Needs `scripts/build-plugins.sh` to have run, and the live
//! test `lake` (elan) installed (`cargo test -p ghostwrite-wasm --test lean -- --ignored`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use ghostwrite_core::Language;
use ghostwrite_core::syntax::SCOPES;
use ghostwrite_lsp::{Document, Event, Registry, convert};
use ghostwrite_plugin::manifest::Manifest;
use ghostwrite_plugin::{Block, Plugin as _};
use serde_json::{Value, json};

fn plugin_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins/lean")
}

/// Its language, the plugin installed; `None` if it hasn't been built.
fn lean() -> Option<&'static Language> {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    let installed = *INSTALLED.get_or_init(|| {
        ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
        if !plugin_folder().join("lean.wasm").is_file() {
            return false;
        }
        let plugin = Manifest::in_own_folder(&plugin_folder()).unwrap();
        plugin.check().unwrap();
        ghostwrite_plugin::install(|| vec![Box::new(plugin)]);
        true
    });
    installed.then(|| Language::for_path(Path::new("Basic.lean")).expect("its files"))
}

macro_rules! needs_the_plugin {
    () => {
        match lean() {
            Some(lean) => lean,
            None => return eprintln!("scripts/build-plugins.sh hasn't run: skipped"),
        }
    };
}

#[test]
fn lexes_lean() {
    let lean = needs_the_plugin!();
    let text = "/-- doc -/\ntheorem add_zero' (n : Nat) : n + 0 = n := by\n  simp -- done\n#eval s!\"{1}\" ++ 'x'.toString\n@[simp] def f := ∀ x, x = 0x1F";
    let spans = lean.highlight(text);
    let at = |word: &str| {
        let at = text.find(word).unwrap();
        spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope])
    };
    assert_eq!(at("doc"), Some("comment.doc"));
    assert_eq!(at("theorem"), Some("keyword"));
    assert_eq!(at("add_zero'"), Some("function"));
    assert_eq!(at("Nat"), Some("type"));
    assert_eq!(at("simp"), Some("function.builtin"));
    assert_eq!(at("done"), Some("comment"));
    assert_eq!(at("#eval"), Some("keyword"));
    assert_eq!(at("{1}"), Some("string"));
    assert_eq!(at("'x'"), Some("string.special"));
    assert_eq!(at("@[simp]"), Some("attribute"));
    assert_eq!(at("∀"), Some("keyword"));
    assert_eq!(at("0x1F"), Some("number"));
    assert_eq!(at(":="), Some("operator"));
    assert_eq!(at("n :"), None, "a variable's just text");
}

#[test]
fn it_knows_its_files_and_symbols() {
    let lean = needs_the_plugin!();
    let abbreviations = lean.abbreviations.unwrap();
    assert_eq!(abbreviations.eager("alpha"), Some("α"));
    assert_eq!(abbreviations.finished("N"), Some("ℕ"));
    assert_eq!(lean.highlight("def x := 1\n").first().map(|s| s.range.clone()), Some(0..3), "def");
    assert_eq!((lean.comment, lean.unpaired, lean.servers), (Some("--"), &['`'][..], &["lean"][..]));
    let server = Manifest::in_own_folder(&plugin_folder()).unwrap().servers()[0];
    assert_eq!((server.name, server.command), ("lean", &["lake", "serve"][..]), "so the project's toolchain and packages are used");
    assert_eq!(server.roots, ["lakefile.lean", "lakefile.toml", "lean-toolchain"]);
    assert_eq!(convert::token_scope("leanSorryLike", false), Some("constant.builtin"), "`sorry`, `admit`");
    assert!(ghostwrite_plugin::keys(Some(lean)).iter().any(|key| key.keys == "space v" && key.command == "preview"));
    assert!(ghostwrite_plugin::panel(lean).is_some() && !ghostwrite_plugin::renders(lean));
}

#[test]
fn it_shows_the_goals_or_the_type_expected() {
    let lean = needs_the_plugin!();
    let mut goals = ghostwrite_plugin::panel(lean).unwrap();
    assert_eq!(goals.title(), "Lean");
    assert!(matches!(goals.blocks().as_slice(), [Block::Muted(_)]));
    goals.answered(vec![Some(json!({ "goals": ["n : Nat\n⊢ n + 0 = n"] })), None]);
    assert_eq!(goals.blocks(), [Block::Heading("1 goal".into()), Block::Code { text: "n : Nat\n⊢ n + 0 = n".into(), stands_out: Some('⊢') }]);
    goals.answered(vec![Some(json!({ "goals": [] })), None]);
    assert_eq!(goals.blocks(), [Block::Good("No goals".into())]);
    goals.answered(vec![None, Some(json!({ "goal": "Nat" }))]);
    assert_eq!(goals.blocks()[0], Block::Heading("Expected type".into()));
}

#[test]
fn once_its_done_checking_the_goals_are_asked_for_again() {
    let lean = needs_the_plugin!();
    let mut goals = ghostwrite_plugin::panel(lean).unwrap();
    let progress = |line: Option<u32>| json!({ "processing": line.map_or(vec![], |l| vec![json!({ "range": { "start": { "line": l } } })]) });
    assert!(!goals.notification("$/lean/fileProgress", &progress(Some(3))), "still going");
    assert_eq!(goals.working_on(), Some(3));
    assert!(goals.notification("$/lean/fileProgress", &progress(None)), "done");
    assert_eq!(goals.working_on(), None);
    assert!(!goals.notification("$/lean/fileProgress", &progress(None)), "and only once");
    assert!(!goals.notification("something else", &json!({})));
}

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

/// Live: its goals at the cursor, `#eval`'s output, its progress, and its colors.
#[test]
#[ignore]
fn lean_shows_goals_and_output_and_progress() {
    let _ = needs_the_plugin!();
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
                // Done checking: nothing's in progress.
                Event::Notification { method, params, .. } if method == "$/lean/fileProgress" => {
                    done = params["processing"].as_array().is_some_and(Vec::is_empty)
                }
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
    let edited = buffer.to_string();
    let edits = buffer.take_edits();
    doc.changed(buffer.rope(), edits);
    let goal = answer(doc.ask("$/lean/plainGoal", &edited, edited.find("rfl").unwrap()));
    assert_eq!(goal["goals"][0], "m n : Nat\n⊢ n + 0 = n", "Lean has the edit");
    let all = convert::token_data(&answer(doc.semantic_tokens(None)), None).expect("tokens").1;
    let changed = convert::token_data(&answer(doc.semantic_tokens(id.as_deref())), Some(&data)).expect("tokens").1;
    assert_eq!(changed, all);
    std::fs::remove_dir_all(root).ok();
}
