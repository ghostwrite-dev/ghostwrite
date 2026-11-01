//! Where a story's items apply, and the edit an accepted step makes.
//!
//! An item is placed in a file (the one asked about, or another one in the
//! project) near some lines: where its quoted line numbers point, if they
//! do. A step names its file with `<file>`; one that doesn't is in the file
//! asked about, unless its lines are only in one other project file (Claude
//! read it, then forgot to say). Placing never touches the editor: file
//! contents come from a [`Files`] the editor provides, with open buffers as
//! they are on screen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::lines::{self, Change};
use crate::protocol::{Note, Step};
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
        let place = self.find(item, asked, files);
        match pointed_at(item) {
            Some(near) => Place { near, ..place },
            None => place,
        }
    }

    fn find(&self, item: &Item, asked: &Asked, files: &dyn Files) -> Place {
        let here = Place { path: None, near: asked.lines.clone() };
        match item {
            Item::Step(Step { file: Some(file), old, .. }) => {
                let path = self.path(file);
                if Some(path.as_path()) == asked.path {
                    return here;
                }
                // In another file an empty `old` appends.
                let near = match files.text(&path).filter(|_| old.is_empty()) {
                    Some(text) => {
                        let n = text.lines().count();
                        n..n
                    }
                    None => 0..1,
                };
                Place { path: Some(path), near }
            }
            Item::Note(Note { file: Some(file), .. }) => {
                let path = self.path(file);
                if Some(path.as_path()) == asked.path { here } else { Place { path: Some(path), near: 0..1 } }
            }
            // Not named: the file asked about, unless the lines are only in
            // one other project file (Claude read it, then forgot <file>).
            Item::Step(Step { file: None, old: quoted, .. }) | Item::Note(Note { file: None, at: quoted, .. }) if !quoted.is_empty() => {
                if lines::find(asked.text, quoted, asked.lines.clone(), false).is_some() {
                    return here;
                }
                let found = self.found_in.borrow_mut().entry(quoted.join("\n")).or_insert_with(|| only_file_with(quoted, files)).clone();
                match found {
                    Some(path) => Place { path: Some(path), near: 0..1 },
                    None => here,
                }
            }
            _ => here,
        }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.root.join(file.trim_start_matches("./"))
    }

    /// The files as they'd be with `accepted` applied one after another, as
    /// a request sent ahead assumes they are.
    pub fn assume(&self, asked: &Asked, files: &dyn Files, accepted: &[Item]) -> Assumed {
        let mut assumed = Assumed { asked: asked.text.to_string(), ..Assumed::default() };
        for item in accepted {
            // Removing and renaming aren't assumed: the next request sees the files as they are.
            let Item::Step(step) = item else { continue };
            if step.op.as_ref().is_some_and(|op| *op != crate::protocol::FileOp::Create) {
                continue;
            }
            let (place, text) = {
                let files = Overlay { files, changed: &assumed.others };
                let now = Asked { text: &assumed.asked, lines: asked.lines.clone(), ..*asked };
                let place = self.place(item, &now, &files);
                let text = match &place.path {
                    None => Some(assumed.asked.clone()),
                    Some(path) => files.text(path).or_else(|| step.creates().then(String::new)),
                };
                (place, text)
            };
            let Some(mut text) = text else { continue };
            let Some(edit) = edit(&text, step, place.near) else { continue };
            text.replace_range(edit.bytes, &edit.text);
            match place.path {
                None => assumed.asked = text,
                Some(path) => {
                    assumed.changed_at.push((path.clone(), edit.cursor_line..edit.cursor_line + 1));
                    assumed.others.insert(path, text);
                }
            }
        }
        assumed
    }
}

/// Where an item's quoted line numbers (or the editor, for a note it gave) point.
fn pointed_at(item: &Item) -> Option<Range<usize>> {
    let line = item.line()?;
    Some(line..line.saturating_add(item.quoted().len().max(1)))
}

/// The files with steps not yet accepted applied: the one asked about, and
/// the others they change, and where each step changed one, in order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Assumed {
    pub asked: String,
    pub others: HashMap<PathBuf, String>,
    pub changed_at: Vec<(PathBuf, Range<usize>)>,
}

/// Files as they are, but for the `changed` ones.
pub struct Overlay<'a> {
    pub files: &'a dyn Files,
    pub changed: &'a HashMap<PathBuf, String>,
}

impl Files for Overlay<'_> {
    fn text(&self, path: &Path) -> Option<String> {
        self.changed.get(path).cloned().or_else(|| self.files.text(path))
    }

    fn all(&self) -> Vec<PathBuf> {
        let mut all = self.files.all();
        all.extend(self.changed.keys().filter(|p| !all.contains(p)).cloned().collect::<Vec<_>>());
        all
    }
}

/// The one project file containing `quoted`, if exactly one does.
fn only_file_with(quoted: &[String], files: &dyn Files) -> Option<PathBuf> {
    let mut hits = files.all().into_iter().filter(|path| files.text(path).is_some_and(|t| lines::find(&t, quoted, 0..1, false).is_some()));
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

/// Bytes of whole `lines` of `text`, line breaks included.
fn lines_bytes(text: &str, lines: Range<usize>) -> Range<usize> {
    let starts = lines::line_starts(text);
    let at = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    at(lines.start)..at(lines.end).max(at(lines.start))
}

/// What a step does to `text`, placed near the lines `near`.
fn change(text: &str, step: &Step, near: Range<usize>) -> Option<Change> {
    let found = lines::find(text, &step.old, near, step.line.is_some())?;
    Some(lines::change(text, &found, &step.old, &step.new))
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
    let change = change(text, step, near)?;
    let anchor = change.anchor_line();
    let first = if change.removed.is_empty() { anchor } else { change.removed.start };
    Some(StepView { change, first, anchor })
}

/// What accepting a step does to its file's text: exactly what its view
/// shows, the lines struck out replaced by the lines added.
#[derive(Debug, PartialEq, Eq)]
pub struct StepEdit {
    pub bytes: Range<usize>,
    pub text: String,
    /// The line the cursor goes to the end of, after the edit (like accepting a completion).
    pub cursor_line: usize,
}

pub fn edit(text: &str, step: &Step, near: Range<usize>) -> Option<StepEdit> {
    let change = change(text, step, near)?;
    let bytes = lines_bytes(text, change.removed.clone());
    let replaced = &text[bytes.clone()];
    let mut new = change.added.join("\n");
    if !change.added.is_empty() && (replaced.ends_with('\n') || replaced.is_empty()) {
        new.push('\n');
    }
    // Adding after a last line without a line break: start a new line.
    let unterminated = bytes.start == text.len() && !text.is_empty() && !text.ends_with('\n');
    if unterminated && !new.is_empty() {
        new.insert(0, '\n');
    }
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
            line: None,
            new: new.iter().map(|s| s.to_string()).collect(),
            say: String::new(),
            op: None,
            complete: true,
        })
    }

    fn at_line(item: Item, line: usize) -> Item {
        let Item::Step(step) = item else { panic!() };
        Item::Step(Step { line: Some(line), ..step })
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
        assert_eq!(place(&at_line(step(Some("lib.rs"), &["x = 1"], &["x = 2"]), 7)).near, 7..8, "where its numbers point");
    }

    #[test]
    fn notes_find_their_files() {
        let root = PathBuf::from("/p");
        let project = Project(vec![(root.join("main.rs"), "fn main() {}\n".into()), (root.join("lib.rs"), "pub fn run() {}\n".into())]);
        let asked = Asked { path: Some(&root.join("main.rs")), text: "fn main() {}\n", lines: 0..1 };
        let placer = Placer::new(root.clone());
        let note = |file: Option<&str>, at: &str| Item::Note(Note { file: file.map(str::to_string), at: vec![at.into()], text: "x".into(), line: None });
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
        assert_eq!(apply("a\nb\nc\n", &step(None, &["b"], &[]), 0..1), "a\nc\n", "deleting");
        // Inserting after an anchor line shows only the new line.
        let insert = step(None, &["fn a() {"], &["fn a() {", "    x();"]);
        let Item::Step(s) = &insert else { panic!() };
        assert_eq!(view("fn a() {\n}\n", s, 0..1).unwrap(), StepView { change: Change { removed: 1..1, added: vec!["    x();".into()] }, first: 0, anchor: 0 });
    }

    #[test]
    fn an_edit_is_what_its_view_shows() {
        // The quote's spacing is off: the line it repeats stays as the file has it,
        // and the new line is indented as the file is.
        let text = "fn a() {\n\tlet  x = 1;\n}\n";
        let item = step(None, &["    let x = 1;"], &["    let x = 1;", "    let y = 2;"]);
        assert_eq!(apply(text, &item, 0..1), "fn a() {\n\tlet  x = 1;\n\tlet y = 2;\n}\n");
    }

    #[test]
    fn a_number_picks_the_copy_meant() {
        let text = "if a {\n    done();\n}\nif b {\n    done();\n}\n";
        let root = PathBuf::from("/p");
        let asked = Asked { path: Some(&root.join("a.rs")), text, lines: 0..1 };
        let placer = Placer::new(root.clone());
        let apply_placed = |item: Item| apply(text, &item, placer.place(&item, &asked, &Project(vec![])).near);
        let finished = "if a {\n    done();\n}\nif b {\n    finish();\n}\n";
        assert_eq!(apply_placed(at_line(step(None, &["    done();"], &["    finish();"]), 4)), finished, "not the copy nearer the cursor");
        assert_eq!(apply_placed(at_line(step(None, &["    don();"], &["    finish();"]), 4)), finished, "a slip in the copy, where the number points");
    }

    #[test]
    fn steps_ahead_are_assumed_in_every_file_they_change() {
        let root = PathBuf::from("/p");
        let project = Project(vec![(root.join("main.rs"), "fn main() {\n    run();\n}\n".into()), (root.join("lib.rs"), "pub fn run() {}\n".into())]);
        let asked = Asked { path: Some(&root.join("main.rs")), text: "fn main() {\n    run();\n}\n", lines: 1..2 };
        let accepted = [
            step(Some("lib.rs"), &["pub fn run() {}"], &["pub fn run(n: u32) {}"]),
            step(Some("main.rs"), &["    run();"], &["    run(1);"]),
            step(Some("lib.rs"), &["pub fn run(n: u32) {}"], &["pub fn run(n: u32) {", "}"]),
        ];
        let mut create = step(Some("util.rs"), &[], &["pub fn two() {}"]);
        if let Item::Step(s) = &mut create {
            s.op = Some(crate::protocol::FileOp::Create);
        }
        let mut accepted = accepted.to_vec();
        accepted.push(create);
        let assumed = Placer::new(root.clone()).assume(&asked, &project, &accepted);
        assert_eq!(assumed.asked, "fn main() {\n    run(1);\n}\n");
        assert_eq!(assumed.others.get(&root.join("lib.rs")).map(String::as_str), Some("pub fn run(n: u32) {\n}\n"), "each builds on the one before");
        assert_eq!(assumed.changed_at.iter().rfind(|(p, _)| p == &root.join("lib.rs")).map(|(_, l)| l.clone()), Some(1..2), "and was last changed there");
        assert_eq!(assumed.others.get(&root.join("util.rs")).map(String::as_str), Some("pub fn two() {}\n"), "a file made ahead");
    }
}
