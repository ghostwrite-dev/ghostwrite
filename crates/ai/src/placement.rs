//! Where a story's items apply, and the edit an accepted step makes.
//!
//! An item is placed in a file (the one asked about, or another one in the
//! project) near some lines. A step names its file with `<file>`; one that
//! doesn't is in the file asked about, unless its lines are only in one other
//! project file (Claude read it, then forgot to say). Placing never touches
//! the editor: file contents come from a [`Files`] the editor provides, with
//! open buffers as they are on screen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::protocol::{self, Change, Note, Step};
use crate::story::Item;

/// The project's files as the editor sees them.
pub trait Files {
    /// A file's text: its open buffer if it has one, else what's on disk.
    fn text(&self, path: &Path) -> Option<String>;
    /// Every file in the project.
    fn all(&self) -> Vec<PathBuf>;
}

/// The file asked about, and the lines pointed at in it.
pub struct Asked<'a> {
    pub path: Option<&'a Path>,
    pub text: &'a str,
    pub lines: Range<usize>,
}

/// Where an item applies: a file (`None`: the one asked about), and the
/// lines to look for it near.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: Option<PathBuf>,
    pub near: Range<usize>,
}

/// Places items, remembering where unnamed steps were found.
pub struct Placer {
    root: PathBuf,
    found_in: RefCell<HashMap<String, Option<PathBuf>>>,
}

impl Placer {
    pub fn new(root: PathBuf) -> Self {
        Self { root, found_in: RefCell::default() }
    }

    pub fn place(&self, item: &Item, asked: &Asked, files: &dyn Files) -> Place {
        let here = Place { path: None, near: asked.lines.clone() };
        match item {
            Item::Step(Step { file: Some(file), old, .. }) => {
                let path = self.root.join(file.trim_start_matches("./"));
                if Some(path.as_path()) == asked.path {
                    return here;
                }
                // In another file an empty `old` appends.
                let near = match files.text(&path).filter(|_| old.is_empty()) {
                    Some(text) => {
                        let n = line_count(&text);
                        n..n
                    }
                    None => 0..1,
                };
                Place { path: Some(path), near }
            }
            Item::Note(Note { file: Some(file), .. }) => self.named(file, asked).unwrap_or(here),
            // Not named: the file asked about, unless the lines are only in
            // one other project file (Claude read it, then forgot <file>).
            Item::Step(Step { file: None, old: lines, .. }) | Item::Note(Note { file: None, at: lines, .. }) if !lines.is_empty() => {
                if protocol::locate(asked.text, lines, asked.lines.clone()).is_some() {
                    return here;
                }
                let found = self.found_in.borrow_mut().entry(lines.join("\n")).or_insert_with(|| only_file_with(lines, files)).clone();
                match found {
                    Some(path) => Place { path: Some(path), near: 0..1 },
                    None => here,
                }
            }
            _ => here,
        }
    }

    /// A file named by an item: the one asked about, or another.
    fn named(&self, file: &str, asked: &Asked) -> Option<Place> {
        let path = self.root.join(file.trim_start_matches("./"));
        Some(if Some(path.as_path()) == asked.path { Place { path: None, near: asked.lines.clone() } } else { Place { path: Some(path), near: 0..1 } })
    }
}

/// The one project file containing `old`, if exactly one does.
fn only_file_with(old: &[String], files: &dyn Files) -> Option<PathBuf> {
    let mut hits = files.all().into_iter().filter(|path| files.text(path).is_some_and(|t| protocol::locate(&t, old, 0..1).is_some()));
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

/// Lines holding text (a trailing line break doesn't start one).
fn line_count(text: &str) -> usize {
    text.matches('\n').count() + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

/// Bytes of whole `lines` of `text`, line breaks included.
fn lines_bytes(text: &str, lines: Range<usize>) -> Range<usize> {
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let at = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    at(lines.start)..at(lines.end).max(at(lines.start))
}

/// How a step shows: what changes, the first line to show, and the line
/// its new lines fold out under.
#[derive(Debug, PartialEq, Eq)]
pub struct StepView {
    pub change: Change,
    pub first: usize,
    pub anchor: usize,
}

pub fn view(text: &str, step: &Step, near: Range<usize>) -> Option<StepView> {
    let change = protocol::change(protocol::locate(text, &step.old, near)?, &step.old, &step.new);
    let anchor = change.anchor_line();
    let first = if change.removed.is_empty() { anchor } else { change.removed.start };
    Some(StepView { change, first, anchor })
}

/// What accepting a step does to its file's text.
#[derive(Debug, PartialEq, Eq)]
pub struct StepEdit {
    pub bytes: Range<usize>,
    pub text: String,
    /// The line the cursor goes to the end of, after the edit (like accepting a completion).
    pub cursor_line: usize,
}

pub fn edit(text: &str, step: &Step, near: Range<usize>) -> Option<StepEdit> {
    let at = protocol::locate(text, &step.old, near)?;
    let bytes = lines_bytes(text, at.clone());
    let replaced = &text[bytes.clone()];
    let mut new = step.new.join("\n");
    if !step.new.is_empty() && (replaced.ends_with('\n') || replaced.is_empty()) {
        new.push('\n');
    }
    // Appending after a last line without a line break: start a new line.
    let unterminated = bytes.start == text.len() && !text.is_empty() && !text.ends_with('\n');
    if unterminated && !new.is_empty() {
        new.insert(0, '\n');
    }
    let change = protocol::change(at, &step.old, &step.new);
    let cursor_line = if change.added.is_empty() { change.removed.start.saturating_sub(1) } else { change.removed.start + change.added.len() - 1 };
    Some(StepEdit { bytes, text: new, cursor_line })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project(Vec<(PathBuf, String)>);

    impl Files for Project {
        fn text(&self, path: &Path) -> Option<String> {
            self.0.iter().find(|(p, _)| p == path).map(|(_, t)| t.clone())
        }
        fn all(&self) -> Vec<PathBuf> {
            self.0.iter().map(|(p, _)| p.clone()).collect()
        }
    }

    fn step(file: Option<&str>, old: &[&str], new: &[&str]) -> Item {
        Item::Step(Step {
            file: file.map(str::to_string),
            old: old.iter().map(|s| s.to_string()).collect(),
            new: new.iter().map(|s| s.to_string()).collect(),
            complete: true,
        })
    }

    fn apply(text: &str, item: &Item, near: Range<usize>) -> String {
        let Item::Step(s) = item else { panic!() };
        let e = edit(text, s, near).unwrap();
        format!("{}{}{}", &text[..e.bytes.start], e.text, &text[e.bytes.end..])
    }

    #[test]
    fn steps_find_their_files() {
        let root = PathBuf::from("/p");
        let project = Project(vec![
            (root.join("main.rs"), "fn main() {}\n".into()),
            (root.join("util.rs"), "pub fn one() {}".into()),
            (root.join("lib.rs"), "x = 1\n".into()),
        ]);
        let asked = Asked { path: Some(&root.join("main.rs")), text: "fn main() {}\n", lines: 0..1 };
        let placer = Placer::new(root.clone());
        let place = |item: &Item| placer.place(item, &asked, &project);
        assert_eq!(place(&step(None, &["fn main() {}"], &["x"])), Place { path: None, near: 0..1 }, "here");
        assert_eq!(place(&step(Some("main.rs"), &["fn main() {}"], &["x"])), Place { path: None, near: 0..1 }, "naming the file asked about");
        assert_eq!(place(&step(Some("util.rs"), &[], &["pub fn two() {}"])), Place { path: Some(root.join("util.rs")), near: 1..1 }, "appends");
        assert_eq!(place(&step(None, &["x = 1"], &["x = 2"])), Place { path: Some(root.join("lib.rs")), near: 0..1 }, "found where its lines are");
        assert_eq!(place(&step(Some("new.rs"), &[], &["a"])), Place { path: Some(root.join("new.rs")), near: 0..1 }, "a file that isn't there yet");
    }

    #[test]
    fn notes_find_their_files() {
        let root = PathBuf::from("/p");
        let project = Project(vec![(root.join("main.rs"), "fn main() {}\n".into()), (root.join("lib.rs"), "pub fn run() {}\n".into())]);
        let asked = Asked { path: Some(&root.join("main.rs")), text: "fn main() {}\n", lines: 0..1 };
        let placer = Placer::new(root.clone());
        let note = |file: Option<&str>, at: &str| Item::Note(Note { file: file.map(str::to_string), at: vec![at.into()], text: "x".into() });
        assert_eq!(placer.place(&note(Some("lib.rs"), "pub fn run() {}"), &asked, &project).path, Some(root.join("lib.rs")));
        assert_eq!(placer.place(&note(None, "pub fn run() {}"), &asked, &project).path, Some(root.join("lib.rs")), "found by its lines");
        assert_eq!(placer.place(&note(None, "fn main() {}"), &asked, &project).path, None);
    }

    #[test]
    fn edits_replace_whole_lines() {
        let item = step(None, &["    pass"], &["    return a + b"]);
        assert_eq!(apply("def f():\n    pass\n", &item, 0..1), "def f():\n    return a + b\n");
        let Item::Step(s) = &item else { panic!() };
        assert_eq!(edit("def f():\n    pass\n", s, 0..1).unwrap().cursor_line, 1);
        // Appending to a file whose last line has no line break.
        assert_eq!(apply("pub fn one() {}", &step(None, &[], &["pub fn two() {}"]), 1..1), "pub fn one() {}\npub fn two() {}\n");
        assert_eq!(apply("", &step(None, &[], &["first"]), 0..0), "first\n");
        // Inserting after an anchor line shows only the new line.
        let insert = step(None, &["fn a() {"], &["fn a() {", "    x();"]);
        let Item::Step(s) = &insert else { panic!() };
        assert_eq!(view("fn a() {\n}\n", s, 0..1).unwrap(), StepView { change: Change { removed: 1..1, added: vec!["    x();".into()] }, first: 0, anchor: 0 });
    }
}
