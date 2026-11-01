//! Real plugins built for WebAssembly and run: what they can do, what they can't,
//! and a real grammar. Building needs the `wasm32-wasip2` target.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ghostwrite_core::Language;
use ghostwrite_plugin::manifest::Manifest;
use ghostwrite_plugin::{Action, Context, Event, Plugin as _, install};

/// The folder of the crate at `path`, relative to this one, with its `.wasm` built.
fn built(crate_path: &str, wasm: &str) -> Option<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(crate_path);
    // Kept with the rest of the build, so that it's reused, and cleaned with it.
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugins");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--release", "--target", "wasm32-wasip2", "--manifest-path"])
        .arg(source.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    if !status.success() {
        return None;
    }
    let out = ghostwrite_project::scratch_dir("plugin-folder");
    std::fs::copy(target.join("wasm32-wasip2/release").join(wasm), out.join(wasm)).expect("it was built");
    for file in ["plugin.json", "highlights.scm", "grammar.wasm"] {
        if source.join(file).exists() {
            std::fs::copy(source.join(file), out.join(file)).unwrap();
        }
    }
    Some(out)
}

fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime)));
}

fn example() -> Option<PathBuf> {
    start();
    static BUILT: OnceLock<Option<PathBuf>> = OnceLock::new();
    BUILT.get_or_init(|| built("../../examples/word-count", "word_count.wasm")).clone()
}

const COMMANDS: &[&str] = &["calls", "spin", "panic", "hoard", "read", "write", "list", "environment", "connect", "nothing"];

/// The plugin that misbehaves.
fn misbehaving() -> Option<Manifest> {
    start();
    static BUILT: OnceLock<Option<PathBuf>> = OnceLock::new();
    let folder = BUILT.get_or_init(|| built("tests/fixture", "misbehaving.wasm")).clone()?;
    let commands: Vec<String> = COMMANDS.iter().map(|name| format!(r#"{{ "name": "{name}" }}"#)).collect();
    let json = format!(r#"{{ "name": "misbehaving", "api": 1, "code": "misbehaving.wasm", "commands": [{}] }}"#, commands.join(","));
    Some(Manifest::parse_in(&json, Some(&folder)).unwrap())
}

fn context(text: &str, selection: std::ops::Range<usize>) -> Context {
    Context { root: "/project".into(), path: Some("/project/a.txt".into()), language: None, text: text.into(), cursor: selection.end, selection }
}

macro_rules! needs_the_target {
    ($made:expr) => {
        match $made {
            Some(made) => made,
            None => return eprintln!("no wasm32-wasip2 target: skipped"),
        }
    };
}

#[test]
fn a_plugin_is_installed_from_its_folder_and_answers() {
    let source = needs_the_target!(example());
    let folder = ghostwrite_project::scratch_dir("plugins");
    let plugin = install::install(&source.display().to_string(), &folder).unwrap();
    assert_eq!(plugin.commands().iter().map(|command| command.name).collect::<Vec<_>>(), ["word-count", "shout"]);
    let described = plugin.describe();
    assert!(described.contains(":word-count :shout") && described.contains("runs when a file is saved"), "{described}");

    assert_eq!(plugin.run("word-count", &[], &context("one two  three\n", 0..0)), Ok(vec![Action::Say("3 words".into())]));
    assert_eq!(
        plugin.run("shout", &[], &context("say hello now", 4..9)),
        Ok(vec![Action::Edit { range: 4..9, text: "HELLO".into() }]),
        "an edit is what it asks for, and the editor does it"
    );
    assert_eq!(plugin.events(), [Event::Saved]);
    assert_eq!(plugin.on_event(Event::Saved, &context("a b", 0..0)), Ok(vec![Action::Say("2 words".into())]));
    assert_eq!(plugin.run("nothing", &[], &context("", 0..0)), Err("no command nothing".into()));
}

#[test]
fn what_cant_be_run_isnt_installed() {
    let source = needs_the_target!(example());
    let (broken, folder) = (ghostwrite_project::scratch_dir("broken-plugin"), ghostwrite_project::scratch_dir("plugins"));
    std::fs::copy(source.join("plugin.json"), broken.join("plugin.json")).unwrap();
    std::fs::write(broken.join("word_count.wasm"), "not webassembly").unwrap();
    let why = install::install(&broken.display().to_string(), &folder).err().unwrap_or_default();
    assert!(why.contains("its code can't be run"), "{why}");
    assert!(!folder.join("word-count").exists());
}

#[test]
fn a_plugin_reaches_nothing_it_isnt_handed() {
    let plugin = needs_the_target!(misbehaving());
    let secret = ghostwrite_project::scratch_dir("secret").join("secret");
    std::fs::write(&secret, "private").unwrap();
    let ask = |name: &str, arg: &Path| plugin.run(name, &[arg.display().to_string()], &context("", 0..0));
    assert!(ask("read", &secret).is_err(), "no files");
    assert!(ask("list", Path::new("/")).is_err(), "no folders");
    assert!(ask("write", &secret.with_extension("new")).is_err(), "no writing");
    assert!(!secret.with_extension("new").exists());
    assert!(plugin.run("connect", &["127.0.0.1:9".to_string()], &context("", 0..0)).is_err(), "no network");
    assert_eq!(plugin.run("environment", &[], &context("", 0..0)), Ok(vec![Action::Say("0".into())]), "no environment");
}

#[test]
fn a_plugin_that_runs_away_is_stopped_and_starts_afresh() {
    let plugin = needs_the_target!(misbehaving());
    let empty = context("", 0..0);
    let calls = |plugin: &Manifest| plugin.run("calls", &[], &empty);
    let before = calls(&plugin).unwrap();
    assert_ne!(calls(&plugin).unwrap(), before, "it keeps what it knows from one call to the next");

    let started = Instant::now();
    let why = plugin.run("spin", &[], &empty).unwrap_err();
    assert!(why.contains("took too long"), "{why}");
    assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    assert_eq!(calls(&plugin), Ok(vec![Action::Say("1".into())]), "and it's back, from the start");

    let why = plugin.run("panic", &[], &empty).unwrap_err();
    assert!(why.contains("it failed"), "{why}");
    assert_eq!(calls(&plugin), Ok(vec![Action::Say("1".into())]));

    assert!(plugin.run("hoard", &[], &empty).is_err(), "it can't have all the memory");
    assert_eq!(calls(&plugin), Ok(vec![Action::Say("1".into())]));
}

#[test]
fn a_grammar_highlights_its_files() {
    start();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    let folder = ghostwrite_project::scratch_dir("grammar-plugin");
    for file in ["grammar.wasm", "highlights.scm"] {
        std::fs::copy(fixture.join(file), folder.join(file)).unwrap();
    }
    let json = r#"{ "name": "grammar-test", "api": 1, "extensions": ["grammartest"],
        "grammar": { "name": "json", "wasm": "grammar.wasm", "query": "highlights.scm" } }"#;
    std::fs::write(folder.join("plugin.json"), json).unwrap();
    let plugin = Manifest::in_own_folder(&folder).unwrap();
    plugin.check().expect("it loads");
    let language: &'static Language = plugin.languages()[0];
    let text = "{\"a\": 1, \"b\": true}";
    let spans = language.highlight(text);
    let shown: Vec<&str> = spans.iter().map(|span| &text[span.range.clone()]).collect();
    assert!(shown.contains(&"1") && shown.contains(&"true"), "{shown:?}");
    assert_eq!(language.highlight(text), spans, "and again, on the same parser");

    std::fs::write(folder.join("highlights.scm"), "(no_such_thing) @string").unwrap();
    assert!(Manifest::in_own_folder(&folder).unwrap().check().is_err(), "a query that doesn't fit is said at once");
    std::fs::write(folder.join("highlights.scm"), "(number) @number").unwrap();
    std::fs::write(folder.join("grammar.wasm"), "not webassembly").unwrap();
    assert!(Manifest::in_own_folder(&folder).unwrap().check().is_err(), "so is what isn't a grammar");
}
