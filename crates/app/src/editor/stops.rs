//! Stories the editor gives: `space d` goes through the file's diagnostics,
//! `:grep` and `space /` the project's matches. Each stop is a note, reviewed like any
//! story's: Tab reads on, and Enter asks the model about it, which takes over
//! from there.

use std::path::Path;

use ghostwrite_ai::Item;
use ghostwrite_ai::protocol::{Note, Step};
use ghostwrite_project::files::{MAX_FOUND, grep, search_pattern};
use gpui::{AppContext as _, Context, Window};

use super::Editor;

/// A note at `line` of `path`, whose text there is `text`.
fn note_at(root: &Path, path: &Path, line: usize, text: &str, say: String) -> Item {
    let file = ghostwrite_project::platform::with_slashes(path.strip_prefix(root).unwrap_or(path));
    // No text (a line too long to keep): the note is at the line by its number.
    let at = if text.is_empty() { vec![] } else { vec![text.to_string()] };
    Item::Note(Note { file: Some(file), at, text: say, line: Some(line) })
}

/// Each matching line once, with the matches in it replaced: a step for each that changes.
fn replacing(root: &Path, pattern: &regex::Regex, with: &str, found: &[ghostwrite_project::files::Found]) -> Vec<Item> {
    let mut items: Vec<Item> = vec![];
    let mut last = None;
    for f in found {
        if f.text.is_empty() || last == Some((&f.path, f.line)) {
            continue;
        }
        last = Some((&f.path, f.line));
        let new = pattern.replace_all(&f.text, with).into_owned();
        if new != f.text {
            let file = ghostwrite_project::platform::with_slashes(f.path.strip_prefix(root).unwrap_or(&f.path));
            let say = format!("{} → {}", pattern.as_str().trim_start_matches("(?i)"), with);
            items.push(Item::Step(Step { file: Some(file), old: vec![f.text.clone()], line: Some(f.line), new: vec![new], say, op: None, complete: true }));
        }
    }
    items
}

impl Editor {
    /// `:replace old new`: search the project, and go through the changes as
    /// steps, accepted with Tab; `:replace!` takes them all. Neither saves.
    pub(super) fn start_replace(&mut self, typed: &str, with: String, all: bool, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let pattern = search_pattern(typed)?;
        let open = self.project.tabs.iter().filter_map(|t| Some((t.doc.path.clone()?, t.doc.text().to_string()))).collect();
        let root = self.project.root.clone();
        let search = {
            let pattern = pattern.clone();
            cx.background_spawn(async move { grep(&root, &pattern, &open) })
        };
        let typed = typed.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let found = search.await;
            this.update(cx, |this, cx| {
                let root = this.project.root.clone();
                let items = replacing(&root, &pattern, &with, &found);
                if items.is_empty() {
                    return this.project.say(format!("nothing to replace for {typed}"));
                }
                let n = items.len();
                this.project.say(format!("{n} line{} to change", if n == 1 { "" } else { "s" }));
                this.open_given(format!("replace {typed} with {with}"), items, cx);
                if all {
                    this.accept_all_given(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
