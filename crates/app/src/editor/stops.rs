//! Stories the editor gives: `:output` goes through the last `:sh` command's
//! errors (or its pages of output), `space d` the file's diagnostics, `:grep`
//! and `space /` the project's matches. Each stop is a note, reviewed like any
//! story's: Tab reads on, and Enter asks the model about it, which takes over
//! from there.

use std::path::Path;

use ghostwrite_ai::Item;
use ghostwrite_ai::protocol::Note;
use ghostwrite_project::files::{MAX_FOUND, grep, search_pattern};
use ghostwrite_project::shell::exit_label;
use ghostwrite_project::stops::{Stop, command_stops};
use gpui::{AppContext as _, Context, Window};

use super::Editor;

/// A note at `line` of `path`, whose text there is `text`.
fn note_at(root: &Path, path: &Path, line: usize, text: &str, say: String) -> Item {
    let file = path.strip_prefix(root).unwrap_or(path).display().to_string();
    Item::Note(Note { file: Some(file), at: vec![text.to_string()], text: say, line: Some(line) })
}

/// Line `line` of the file at `path` (open or on disk).
fn line_of(editor: &Editor, path: &Path, line: usize) -> String {
    let text = editor.project.tabs.text_of(path).map(str::to_string).or_else(|| std::fs::read_to_string(path).ok()).unwrap_or_default();
    text.lines().nth(line).unwrap_or_default().to_string()
}

impl Editor {
    /// `:output`: go through the last command's errors, or its output.
    pub(super) fn output_story(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let shell = self.shell.as_ref().ok_or("no command has run yet (:sh ‹command›)")?;
        let (command, output, exit) = shell.finished()?;
        let exit = exit_label(exit);
        let root = self.project.root.clone();
        let items: Vec<Item> = command_stops(&output, &root)
            .into_iter()
            .map(|stop| match stop {
                Stop::Error(loc) => note_at(&root, &loc.path, loc.line, &line_of(self, &loc.path, loc.line), loc.message.join("\n")),
                Stop::Page(page) => Item::Note(Note { file: None, at: vec![], text: format!("$ {command}\n{}", page.join("\n")), line: None }),
            })
            .collect();
        if items.is_empty() {
            return Err(format!("`{command}` had no output"));
        }
        self.open_given(format!("go through what `{command}` printed ({exit})"), items, cx);
        Ok(())
    }

    /// `space d`: go through this file's diagnostics.
    pub(super) fn diagnostics_story(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.tab().map(|t| &t.doc) else { return };
        let Some(path) = doc.path.clone() else { return };
        let root = self.project.root.clone();
        let items: Vec<Item> = doc
            .diagnostics
            .iter()
            .map(|d| {
                let line = doc.buffer.line_of(d.range.start);
                note_at(&root, &path, line, doc.buffer.line(line).trim_end_matches('\n'), format!("{}: {}", d.severity.label(), d.message))
            })
            .collect();
        if items.is_empty() {
            return self.project.say("no diagnostics in this file");
        }
        let name = self.project.display(&path);
        self.open_given(format!("go through the diagnostics in {name}"), items, cx);
    }

    /// `:grep` and `space /`: search the project (on a background thread),
    /// then go through the matches.
    pub(super) fn start_search(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let pattern = search_pattern(typed)?;
        let open = self.project.tabs.iter().filter_map(|t| Some((t.doc.path.clone()?, t.doc.text().to_string()))).collect();
        let root = self.project.root.clone();
        let search = cx.background_spawn(async move { grep(&root, &pattern, &open) });
        let typed = typed.to_string();
        self.project.say(format!("searching for {typed}…"));
        cx.spawn_in(window, async move |this, cx| {
            let found = search.await;
            this.update(cx, |this, cx| {
                if found.is_empty() {
                    this.project.say(format!("no matches for {typed}"));
                } else {
                    this.project.status = None;
                    let n = found.len();
                    let what = format!("{n}{} match{} for {typed}", if n < MAX_FOUND { "" } else { "+" }, if n == 1 { "" } else { "es" });
                    let root = this.project.root.clone();
                    let items = found.iter().map(|f| note_at(&root, &f.path, f.line, &f.text, what.clone())).collect();
                    this.open_given(format!("go through the {what}"), items, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }
}
