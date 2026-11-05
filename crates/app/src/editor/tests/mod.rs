//! The editor driven by keystrokes, headless, by feature. Here is the
//! harness: an editor on a scratch project, with a scripted model.

mod editing;
mod files;
mod languages;
mod plugins;
mod story;
mod terminal;

use std::path::PathBuf;

use ghostwrite_ai::Backend;
use ghostwrite_core::Mode;
use ghostwrite_project::tree::Proposal;
use ghostwrite_project::{fs, scratch_dir, store};
use gpui::{Entity, TestAppContext, VisualTestContext, WindowAppearance};

use super::{Editor, Options};

struct Setup<'a> {
    editor: Entity<Editor>,
    cx: &'a mut VisualTestContext,
    dir: PathBuf,
}

/// A project folder with `doc.txt` holding `text`, open in an editor.
fn open<'a>(text: &str, replies: &[&str], cx: &'a mut TestAppContext) -> Setup<'a> {
    open_with(text, replies, false, cx)
}

/// [`open`], with the language servers running or not.
fn open_with<'a>(text: &str, replies: &[&str], language_servers: bool, cx: &'a mut TestAppContext) -> Setup<'a> {
    store::redirect_for_tests();
    fs::delete_instead_of_trash();
    let dir = scratch_dir("editor");
    let path = dir.join("doc.txt");
    std::fs::write(&path, text).unwrap();
    cx.update(|cx| {
        crate::theme::install(WindowAppearance::Light, cx);
        crate::ui::text_field::bind_keys(cx);
    });
    let backend = (!replies.is_empty()).then(|| Backend::Scripted(replies.iter().map(|r| r.to_string()).collect()));
    let root = dir.clone();
    let (editor, cx) = cx.add_window_view(|window, cx| {
        let mut editor = Editor::new(root, Options { language_servers, asides: false, backend }, window, cx);
        editor.open_paths(vec![path], cx);
        editor
    });
    cx.run_until_parked();
    Setup { editor, cx, dir }
}

impl Setup<'_> {
    fn keys(&mut self, keys: &str) -> &mut Self {
        self.cx.simulate_keystrokes(keys);
        self.cx.run_until_parked();
        self
    }

    fn input(&mut self, text: &str) -> &mut Self {
        self.cx.simulate_input(text);
        self.cx.run_until_parked();
        self
    }

    /// `:` and a command, then Enter.
    fn command(&mut self, line: &str) -> &mut Self {
        self.keys(":").input(line).keys("enter")
    }

    fn read<R>(&mut self, f: impl FnOnce(&Editor) -> R) -> R {
        self.editor.read_with(self.cx, |e, _| f(e))
    }

    fn text(&mut self) -> String {
        self.read(|e| e.tab().map(|t| t.doc.buffer.to_string()).unwrap_or_default())
    }

    fn status(&mut self) -> Option<String> {
        self.read(|e| e.project.status.as_ref().map(|s| s.to_string()))
    }

    fn active(&mut self) -> Option<String> {
        let dir = self.dir.clone();
        self.read(|e| e.project.active_path().map(|p| p.strip_prefix(&dir).unwrap().display().to_string()))
    }

    fn cursor(&mut self) -> usize {
        self.read(|e| e.tab().map_or(0, |t| t.view.selection.head))
    }

    fn reviewing(&mut self) -> bool {
        self.read(|e| e.story.is_some())
    }

    /// Which reply the item under review is.
    fn current(&mut self) -> Option<usize> {
        self.read(|e| e.story.as_ref()?.story.as_ref()?.current()).map(|id| id.0)
    }

    /// `space i`, a request, Enter, and the scripted reply arrives.
    fn ask(&mut self, what: &str) -> &mut Self {
        self.keys("space a").input(what).keys("enter")
    }

    fn file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap_or_default()
    }
}

// Replies the scripted model gives.

const ADD: &str = "def add(a, b):\n    pass\n";

const DONE: &str = "<done>\n</done>\n";

/// A scripted edit in `file`: `at` becomes `new` (lines joined with `\n`).
fn edit(file: &str, at: &str, new: &str) -> String {
    format!("<file>\n{file}\n</file>\n<at>\n{at}\n</at>\n<say>\nBecause.\n</say>\n<new>\n{new}\n</new>\n")
}

/// A scripted note in `file`, about `at` (empty: the file as a whole).
fn note(file: &str, at: &str, say: &str) -> String {
    let at = if at.is_empty() { String::new() } else { format!("{at}\n") };
    format!("<file>\n{file}\n</file>\n<at>\n{at}</at>\n<say>\n{say}\n</say>\n")
}

fn fix() -> String {
    edit("doc.txt", "    pass", "    return a + b")
}

fn hints() -> String {
    edit("doc.txt", "def add(a, b):", "def add(a: int, b: int) -> int:")
}
