//! How long editing a big file takes: an edit, parsing again, coloring again, and
//! what the UI thread does for each keystroke. Measured, not checked:
//! `cargo test --release -p ghostwrite-wasm --test speed -- --ignored --nocapture`.
//! Needs `scripts/build-plugins.sh` to have run.

use std::path::Path;
use std::time::{Duration, Instant};

use ghostwrite_core::syntax::{Colors, spans_in};
use ghostwrite_core::{Buffer, Language, Range, Text as _};
use ghostwrite_plugin::manifest::Manifest;

fn install() -> bool {
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins");
    if !bundled.join("rust/plugin.json").is_file() {
        return false;
    }
    ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
    let (found, _) = Manifest::in_folder(&bundled);
    ghostwrite_plugin::install(|| found.into_iter().map(|plugin| Box::new(plugin) as Box<dyn ghostwrite_plugin::Plugin>).collect());
    true
}

/// The least of `times` runs of `work`: what it takes when nothing else is in the way.
fn least<T>(times: usize, mut work: impl FnMut() -> T) -> (Duration, T) {
    let mut best = (Duration::MAX, None);
    for _ in 0..times {
        let start = Instant::now();
        let out = work();
        let took = start.elapsed();
        if took < best.0 {
            best = (took, Some(out));
        }
    }
    (best.0, best.1.unwrap())
}

#[test]
#[ignore]
fn how_long_a_keystroke_takes_in_a_big_file() {
    if !install() {
        return eprintln!("scripts/build-plugins.sh hasn't run: skipped");
    }
    let one = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/src/editor/element/mod.rs")).unwrap();
    let text = one.repeat(12);
    let language = Language::for_path(Path::new("a.rs")).unwrap();
    let grammar = language.grammar.unwrap();
    Colors::new(language, 0, ghostwrite_core::Rope::from_str("fn main() {}"));
    println!("{} lines, {} bytes", text.lines().count(), text.len());

    let (took, (buffer, colors)) = least(5, || {
        let buffer = Buffer::new(&text);
        let colors = Colors::new(language, buffer.version(), buffer.rope().clone());
        (buffer, colors)
    });
    println!("open (buffer, parse, color all of it): {took:?}, {} spans", colors.spans.len());

    let mut buffer = buffer;
    let middle = buffer.floor_boundary(buffer.len() / 2);
    let mut scratch = Buffer::new(&text);
    let (took, _) = least(100, || {
        scratch.edit(middle..middle, "x");
        scratch.edit(middle..middle + 1, "");
    });
    println!("insert a char in the middle (buffer only): {:?}", took / 2);
    buffer.edit(middle..middle, "x");
    let edits = buffer.changes_since(colors.version).unwrap();

    let (took, _) = least(10, || {
        let mut old = colors.tree().unwrap().clone();
        edits.iter().for_each(|edit| old.edit(edit));
        grammar.parse_rope(buffer.rope(), Some(&old), &[])
    });
    println!("parse again, from the last tree: {took:?}");
    let (took, updated) = least(10, || colors.update(buffer.version(), buffer.rope().clone(), &edits));
    println!("parse again and color what changed: {took:?}");

    // What the UI thread does per keystroke: hand a snapshot off, and draw a screenful moved past the edits.
    let (took, _) = least(100, || (buffer.rope().clone(), buffer.changes_since(colors.version)));
    println!("UI: snapshot and edits for the job: {took:?}");
    let screen = buffer.line_start(buffer.line_of(middle).saturating_sub(30))..buffer.line_end(buffer.line_of(middle) + 30);
    let (took, shown) = least(100, || spans_in(&colors.spans, &edits, screen.clone()));
    println!("UI: a screenful's colors, moved past the edit: {took:?} ({} spans)", shown.len());
    assert!(!updated.spans.is_empty());

    // A file of about 5 MB.
    let big = text.repeat(5 * 1024 * 1024 / text.len() + 1);
    let (took, mut buffer) = least(3, || Buffer::new(&big));
    println!("5 MB ({} lines): open the buffer {took:?}", buffer.line_count());
    let (took, colors) = least(1, || Colors::new(language, buffer.version(), buffer.rope().clone()));
    println!("5 MB: color all of it (off the UI thread) {took:?}, {} spans", colors.spans.len());
    let middle = buffer.floor_boundary(buffer.len() / 2);
    let start = Instant::now();
    buffer.begin(Range::point(middle));
    for (i, c) in "typed".chars().enumerate() {
        buffer.edit(middle + i..middle + i, &c.to_string());
    }
    buffer.commit(Range::point(middle + 5));
    println!("5 MB: type 5 chars in the middle {:?} each", start.elapsed() / 5);
    let edits = buffer.changes_since(colors.version).unwrap();
    let (took, _) = least(3, || colors.update(buffer.version(), buffer.rope().clone(), &edits));
    println!("5 MB: parse again and color what changed {took:?}");
    let screen = buffer.line_start(buffer.line_of(middle).saturating_sub(30))..buffer.line_end(buffer.line_of(middle) + 30);
    let (took, _) = least(100, || spans_in(&colors.spans, &edits, screen.clone()));
    println!("5 MB, UI: a screenful's colors {took:?}");
    let start = Instant::now();
    buffer.undo();
    println!("5 MB: undo it {:?}", start.elapsed());
    assert_eq!(buffer.len(), big.len());
}
