//! The project's file tree. Folders list lazily as they expand, honouring
//! `.gitignore`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

actions!(
    file_tree,
    [
        SelectNext, SelectPrev, SelectFirst, SelectLast, Expand, Collapse, Confirm, Dismiss, NewEntry, RenameEntry, DeleteEntry,
        CancelPrompt, GhostAccept, GhostSkip
    ]
);

pub fn bind_keys(cx: &mut App) {
    // Not while asking about a proposal: then keys go to the prompt.
    let ctx = Some("FileTree && !asking");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("k", SelectPrev, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("g g", SelectFirst, ctx),
        KeyBinding::new("home", SelectFirst, ctx),
        KeyBinding::new("shift-g", SelectLast, ctx),
        KeyBinding::new("end", SelectLast, ctx),
        KeyBinding::new("l", Expand, ctx),
        KeyBinding::new("right", Expand, ctx),
        KeyBinding::new("h", Collapse, ctx),
        KeyBinding::new("left", Collapse, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("escape", Dismiss, ctx),
        KeyBinding::new("a", NewEntry, ctx),
        KeyBinding::new("r", RenameEntry, ctx),
        KeyBinding::new("d", DeleteEntry, ctx),
        KeyBinding::new("escape", CancelPrompt, Some("TreePrompt > Input")),
        KeyBinding::new("escape", CancelPrompt, Some("TreeAsk > Input")),
        // A proposed file is decided here, with the same keys as any story step.
        KeyBinding::new("tab", GhostAccept, ctx),
        KeyBinding::new("backspace", GhostSkip, ctx),
    ]);
}

/// Create `name` inside `dir`: a folder when it ends in `/`, else an empty
/// file. Missing folders on the way are made; an existing entry is an error.
pub fn create_entry(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.split('/').any(|part| part == "..") {
        return Err("give a name without `..`".into());
    }
    let path = dir.join(name.trim_end_matches('/'));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let made = if name.ends_with('/') {
        std::fs::create_dir_all(&path)
    } else {
        path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop)
        })
    };
    made.map(|()| path).map_err(|e| format!("couldn't create {name}: {e}"))
}

/// Rename `from` to `name` within its folder (or below it, if `name` has `/`).
pub fn rename_entry(from: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_end_matches('/');
    if name.is_empty() || name.split('/').any(|part| part == "..") {
        return Err("give a name without `..`".into());
    }
    let to = from.parent().unwrap_or(from).join(name);
    if to == from {
        return Ok(to);
    }
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    to.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(from, &to))
        .map(|()| to)
        .map_err(|e| format!("couldn't rename: {e}"))
}

/// Move `path` to the Trash. Tests delete outright instead of filling the real Trash.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    #[cfg(not(test))]
    let moved = trash::delete(path).map_err(|e| e.to_string());
    #[cfg(test)]
    let moved = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| e.to_string());
    moved.map_err(|e| format!("couldn't move {} to the Trash: {e}", path.display()))
}

/// A line of the list: a real row, a proposed new file, or a proposal's keys.
enum Shown {
    Row(usize),
    Ghost(usize, String),
    Keys(usize, String),
    Ask(usize),
    Note(usize, String),
}

/// A story step about a file or folder, shown in the tree where it applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proposal {
    Create(PathBuf),
    Remove(PathBuf),
    /// This file or folder, to the new path (relative to the project).
    Rename(PathBuf, String),
}

enum Op {
    /// Add a file or folder inside this folder.
    New(PathBuf),
    Rename(PathBuf),
}

pub enum FileTreeEvent {
    /// Open a file; `focus` moves the keyboard to the editor.
    Open { path: PathBuf, focus: bool },
    /// Hand the keyboard back to the editor.
    Dismiss,
    /// A review key pressed on a proposed file.
    Review(crate::editor::ReviewKey),
    /// A file or folder moved; open tabs below `from` should follow.
    Renamed { from: PathBuf, to: PathBuf },
    /// A file or folder went to the Trash.
    Deleted(PathBuf),
    /// Something went wrong; say so in the status bar.
    Error(String),
}

pub(crate) struct Row {
    path: PathBuf,
    pub(crate) name: SharedString,
    depth: usize,
    is_dir: bool,
}

pub struct FileTree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    pub(crate) rows: Vec<Row>,
    active: Option<PathBuf>,
    /// The row the keyboard is on.
    cursor: usize,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The name prompt for adding or renaming, while it's open.
    prompt: Option<(Op, Entity<InputState>, Subscription)>,
    /// A file or folder Claude proposes creating (a faint row where it would
    /// go) or removing (its row, struck through).
    ghost: Option<PathBuf>,
    doomed: Option<PathBuf>,
    renaming: Option<(PathBuf, String)>,
    /// The prompt for asking about or redirecting the proposal, under it.
    asking: Option<Entity<InputState>>,
    /// Claude's answer about the proposal, under it.
    note: Option<String>,
    /// The list's width last frame, for wrapping the answer.
    width: std::rc::Rc<std::cell::Cell<Pixels>>,
}

impl EventEmitter<FileTreeEvent> for FileTree {}

/// Folders first, then files, each by case-insensitive name.
fn list_dir(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut entries: Vec<(PathBuf, bool)> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != ".DS_Store")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            (e.into_path(), is_dir)
        })
        .collect();
    entries.sort_by_cached_key(|(p, is_dir)| (!is_dir, p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
    entries
}

impl FileTree {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            root,
            expanded: HashSet::new(),
            rows: vec![],
            active: None,
            cursor: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            prompt: None,
            ghost: None,
            doomed: None,
            renaming: None,
            asking: None,
            note: None,
            width: std::rc::Rc::new(std::cell::Cell::new(px(240.))),
        };
        this.rebuild();
        this
    }

    pub fn rebuild(&mut self) {
        fn walk(dir: &Path, depth: usize, expanded: &HashSet<PathBuf>, rows: &mut Vec<Row>) {
            for (path, is_dir) in list_dir(dir) {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into();
                let open = is_dir && expanded.contains(&path);
                rows.push(Row { path: path.clone(), name, depth, is_dir });
                if open {
                    walk(&path, depth + 1, expanded, rows);
                }
            }
        }
        let at = self.rows.get(self.cursor).map(|r| r.path.clone());
        let mut rows = vec![];
        walk(&self.root, 0, &self.expanded, &mut rows);
        self.rows = rows;
        self.cursor = at.and_then(|p| self.index_of(&p)).unwrap_or(self.cursor).min(self.rows.len().saturating_sub(1));
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|r| r.path == path)
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Take the keyboard, starting on the file being edited.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active.as_deref().and_then(|p| self.index_of(p)) {
            self.cursor = ix;
        }
        self.scroll.scroll_to_item(self.cursor, ScrollStrategy::Center);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn move_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = ix.min(self.rows.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.cursor, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.cursor + 1, cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.cursor.saturating_sub(1), cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(usize::MAX, cx);
    }

    /// Open a folder (or step into an open one); preview a file, keeping the keyboard here.
    fn expand(&mut self, _: &Expand, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.cursor) else { return };
        match (row.is_dir, self.expanded.contains(&row.path)) {
            (true, true) => self.move_cursor(self.cursor + 1, cx),
            (true, false) => self.click(self.cursor, cx),
            (false, _) => cx.emit(FileTreeEvent::Open { path: row.path.clone(), focus: false }),
        }
    }

    /// Close the folder, or move up to the folder containing this row.
    fn collapse(&mut self, _: &Collapse, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.cursor) else { return };
        if row.is_dir && self.expanded.remove(&row.path) {
            self.rebuild();
            cx.notify();
        } else if let Some(parent) = row.path.parent().and_then(|p| self.index_of(p)) {
            self.move_cursor(parent, cx);
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if self.ghost.is_some() || self.doomed.is_some() || self.renaming.is_some() {
            return self.ghost_key(crate::editor::ReviewKey::Redirect, cx);
        }
        let Some(row) = self.rows.get(self.cursor) else { return };
        if row.is_dir {
            self.click(self.cursor, cx);
        } else {
            cx.emit(FileTreeEvent::Open { path: row.path.clone(), focus: true });
        }
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(if self.ghost.is_some() || self.doomed.is_some() || self.renaming.is_some() { FileTreeEvent::Review(crate::editor::ReviewKey::Stop) } else { FileTreeEvent::Dismiss });
    }

    fn ghost_key(&mut self, key: crate::editor::ReviewKey, cx: &mut Context<Self>) {
        if self.ghost.is_some() || self.doomed.is_some() || self.renaming.is_some() {
            cx.emit(FileTreeEvent::Review(key));
        }
    }

    /// The folder new entries go in: the one under the cursor, or the
    /// cursor's file's folder.
    fn target_dir(&self) -> PathBuf {
        match self.rows.get(self.cursor) {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row.path.parent().map_or(self.root.clone(), Path::to_path_buf),
            None => self.root.clone(),
        }
    }

    fn open_prompt(&mut self, op: Op, initial: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().to_string();
                this.apply_prompt(&name, window, cx);
            }
            InputEvent::Blur => {
                this.prompt = None;
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.prompt = Some((op, input, subscription));
        cx.notify();
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_prompt(&mut self, _: &CancelPrompt, window: &mut Window, cx: &mut Context<Self>) {
        self.close_prompt(window, cx);
    }

    fn new_entry(&mut self, _: &NewEntry, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.target_dir();
        self.open_prompt(Op::New(dir), String::new(), window, cx);
    }

    fn rename_entry(&mut self, _: &RenameEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.cursor) else { return };
        let (path, name) = (row.path.clone(), row.name.to_string());
        self.open_prompt(Op::Rename(path), name, window, cx);
    }

    fn apply_prompt(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((op, _, _)) = self.prompt.take() else { return };
        self.close_prompt(window, cx);
        let done = match op {
            Op::New(dir) => create_entry(&dir, name).map(|path| {
                self.expanded.insert(dir);
                self.reveal(&path);
                if path.is_file() {
                    cx.emit(FileTreeEvent::Open { path, focus: true });
                }
            }),
            Op::Rename(from) => rename_entry(&from, name).map(|to| {
                if self.expanded.remove(&from) {
                    self.expanded.insert(to.clone());
                }
                self.reveal(&to);
                cx.emit(FileTreeEvent::Renamed { from, to });
            }),
        };
        if let Err(e) = done {
            cx.emit(FileTreeEvent::Error(e));
        }
        cx.notify();
    }

    /// Re-list, opening the folders above `path` and putting the cursor on it.
    fn reveal(&mut self, path: &Path) {
        for dir in path.ancestors().skip(1).take_while(|d| d.starts_with(&self.root) && *d != self.root) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rebuild();
        if let Some(ix) = self.index_of(path) {
            self.cursor = ix;
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    /// Move the entry under the cursor to the Trash, after asking.
    fn delete_entry(&mut self, _: &DeleteEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.cursor) else { return };
        let path = row.path.clone();
        let what = if row.is_dir { "folder" } else { "file" };
        let message = format!("Move {} to the Trash?", row.name);
        let detail = format!("The {what} can be restored from the Trash.");
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &["Move to Trash", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                match move_to_trash(&path) {
                    Ok(()) => {
                        this.rebuild();
                        cx.emit(FileTreeEvent::Deleted(path));
                    }
                    Err(e) => cx.emit(FileTreeEvent::Error(e)),
                }
                this.focus.focus(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Highlight the file being edited, opening the folders above it.
    pub fn set_active(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if path == self.active {
            return;
        }
        if let Some(p) = &path
            && p.starts_with(&self.root)
        {
            let mut grew = false;
            for dir in p.ancestors().skip(1).take_while(|d| *d != self.root) {
                grew |= self.expanded.insert(dir.to_path_buf());
            }
            if grew || !self.rows.iter().any(|r| &r.path == p) {
                self.rebuild();
            }
            if let Some(ix) = self.index_of(p) {
                self.cursor = ix;
                self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            }
        }
        self.active = path;
        cx.notify();
    }

    /// Show (or clear) a proposed file or folder: its existing folders open,
    /// and a faint row sits where it would be created.
    pub fn propose(&mut self, proposal: Option<Proposal>, cx: &mut Context<Self>) {
        let (ghost, doomed, renaming) = match proposal {
            Some(Proposal::Create(p)) => (Some(p), None, None),
            Some(Proposal::Remove(p)) => (None, Some(p), None),
            Some(Proposal::Rename(p, to)) => (None, None, Some((p, to))),
            None => (None, None, None),
        };
        if let Some(p) = doomed.as_ref().or(renaming.as_ref().map(|(p, _)| p)) {
            self.reveal(p);
        }
        self.doomed = doomed;
        self.renaming = renaming;
        self.asking = None;
        self.note = None;
        self.set_ghost(ghost, cx);
    }

    #[cfg(test)]
    pub(crate) fn note_for_test(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Show (or clear) an answer about the proposal.
    pub fn set_note(&mut self, note: Option<String>, cx: &mut Context<Self>) {
        self.note = note;
        cx.notify();
    }

    /// What's typed in the ask prompt, while it's open.
    #[cfg(test)]
    pub(crate) fn asking_text(&self, cx: &App) -> Option<String> {
        self.asking.as_ref().map(|i| i.read(cx).value().to_string())
    }

    /// Show (or close) the ask prompt under the proposal.
    pub fn set_asking(&mut self, input: Option<Entity<InputState>>, cx: &mut Context<Self>) {
        self.asking = input;
        cx.notify();
    }

    pub fn set_ghost(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(p) = &path {
            for dir in p.ancestors().skip(1).take_while(|d| d.starts_with(&self.root) && *d != self.root) {
                if dir.is_dir() {
                    self.expanded.insert(dir.to_path_buf());
                }
            }
            self.rebuild();
        }
        self.ghost = path;
        cx.notify();
    }

    /// Where the ghost row goes: under its nearest existing folder (the
    /// root, or a row), at that folder's child depth; and what it reads.
    pub(crate) fn ghost_row(&self) -> Option<(usize, usize, String)> {
        let ghost = self.ghost.as_ref()?;
        let parent = ghost.ancestors().skip(1).find(|d| d.is_dir())?;
        let rel = ghost.strip_prefix(parent).ok()?.display().to_string();
        let name = if ghost.to_string_lossy().ends_with('/') { format!("{rel}/") } else { rel };
        if parent == self.root {
            return Some((0, 0, name));
        }
        let ix = self.index_of(parent)?;
        Some((ix + 1, self.rows[ix].depth + 1, name))
    }

    pub(crate) fn click(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        self.cursor = ix;
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
            }
            self.rebuild();
            cx.notify();
        } else {
            cx.emit(FileTreeEvent::Open { path: row.path.clone(), focus: true });
        }
    }

    /// What each line of the list shows: the real rows, with a proposed new
    /// file among them and a line of keys under whatever is proposed.
    fn display_rows(&self) -> Vec<Shown> {
        let ghost = self.ghost_row();
        let proposed = self.doomed.as_ref().or(self.renaming.as_ref().map(|(p, _)| p));
        let mut out = vec![];
        // Under the proposal: its keys; an answer about it, then its keys; or the ask prompt.
        let cols = |depth: usize| ((f32::from(self.width.get()) - (8. + 14. * depth as f32 + 16.) - 8.) / 7.).max(20.) as usize;
        let keys = |depth: usize, what: &str| match (&self.asking, &self.note) {
            (Some(_), _) => vec![Shown::Ask(depth)],
            (None, Some(note)) => {
                let mut lines: Vec<Shown> = crate::assist::wrap(note, cols(depth)).into_iter().map(|l| Shown::Note(depth, l)).collect();
                lines.push(Shown::Keys(depth, "⇥ next · ⏎ ask".into()));
                lines
            }
            (None, None) => vec![Shown::Keys(depth, format!("⇥ {what} · ⌫ {} · ⏎ ask or redirect", if what == "create" { "skip" } else { "keep" }))],
        };
        for (ix, row) in self.rows.iter().enumerate() {
            if let Some((at, depth, name)) = &ghost
                && *at == ix
            {
                out.push(Shown::Ghost(*depth, name.clone()));
                out.extend(keys(*depth, "create"));
            }
            out.push(Shown::Row(ix));
            if proposed == Some(&row.path) {
                out.extend(keys(row.depth, if self.doomed.is_some() { "remove" } else { "rename" }));
            }
        }
        if let Some((at, depth, name)) = ghost
            && at >= self.rows.len()
        {
            out.push(Shown::Ghost(depth, name));
            out.extend(keys(depth, "create"));
        }
        out
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let focused = self.focus.is_focused(window);
        let theme = cx.theme();
        let shown_rows = self.display_rows();
        range
            .filter_map(|shown| {
                let line = shown_rows.get(shown)?;
                // The keys for a proposal, on their own line under it.
                if let Shown::Keys(depth, keys) = line {
                    return Some(
                        div()
                            .id(("keys", shown))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .pl(px(8. + 14. * *depth as f32 + 16.))
                            .w_full()
                            .text_xs()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(theme.muted_foreground)
                            .child(keys.clone()),
                    );
                }
                // Claude's answer about it.
                if let Shown::Note(depth, text) = line {
                    return Some(
                        div()
                            .id(("note", shown))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .pl(px(8. + 14. * *depth as f32 + 16.))
                            .w_full()
                            .text_sm()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(text.clone()),
                    );
                }
                // Asking about it: the prompt, in place of the keys.
                if let (Shown::Ask(depth), Some(input)) = (line, &self.asking) {
                    return Some(
                        div()
                            .id(("ask", shown))
                            .key_context("TreeAsk")
                            .on_action(cx.listener(|_, _: &CancelPrompt, _, cx| cx.emit(FileTreeEvent::Review(crate::editor::ReviewKey::Stop))))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8. + 14. * *depth as f32 + 16.))
                            .pr_2()
                            .w_full()
                            .child(div().flex_none().text_xs().text_color(theme.warning).child("✦"))
                            .child(Input::new(input).appearance(false).px_0().py_0().h(px(22.)).flex_1().text_sm())
                            .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child("⏎ send · esc")),
                    );
                }
                if let Shown::Ghost(depth, name) = line {
                    return Some(
                        div()
                            .id(("ghost", shown))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8. + 14. * *depth as f32))
                            .pr_2()
                            .w_full()
                            .text_sm()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .bg(if focused { theme.accent } else { theme.warning.opacity(0.08) })
                            .text_color(if focused { theme.accent_foreground } else { theme.muted_foreground })
                            .child(div().w(px(12.)).flex_none().text_color(theme.warning).child("+"))
                            .child(div().italic().child(name.clone())),
                    );
                }
                let Shown::Row(ix) = *line else { return None };
                let row = self.rows.get(ix)?;
                let active = self.active.as_ref() == Some(&row.path);
                let doomed = self.doomed.as_ref() == Some(&row.path);
                let renamed_to = self.renaming.as_ref().filter(|(p, _)| *p == row.path).map(|(_, to)| to.clone());
                let under_cursor = focused && ix == self.cursor;
                let arrow = match (row.is_dir, self.expanded.contains(&row.path)) {
                    (true, true) => "▾",
                    (true, false) => "▸",
                    _ => "",
                };
                Some(
                    div()
                        .id(shown)
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(8. + 14. * row.depth as f32))
                        .pr_2()
                        .text_sm()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .w_full()
                        // A full-width row background: strong for the keyboard's
                        // row, subtle for the open file while the tree has the keyboard.
                        .map(|d| match (under_cursor, active) {
                            (true, _) => d.bg(theme.accent).text_color(theme.accent_foreground),
                            (false, true) if focused => d.bg(theme.muted),
                            (false, true) => d.bg(theme.accent).text_color(theme.accent_foreground),
                            (false, false) => d.hover(|s| s.bg(theme.muted)),
                        })
                        .child(div().w(px(12.)).flex_none().text_color(theme.muted_foreground).child(arrow))
                        .when(!doomed && renamed_to.is_none(), |d| d.child(row.name.clone()))
                        // Proposed rename: the old name struck, the new one beside it.
                        .when_some(renamed_to, |d, to| {
                            d.bg(theme.warning.opacity(if focused { 0.2 } else { 0.08 }))
                                .child(div().line_through().text_color(theme.muted_foreground).child(row.name.clone()))
                                .child(div().text_color(theme.warning).child(format!("→ {to}")))
                        })
                        // Proposed for removal: struck through, with the keys.
                        .when(doomed, |d| {
                            d.bg(theme.danger.opacity(if focused { 0.22 } else { 0.1 }))
                                .child(div().line_through().text_color(theme.danger).child(row.name.clone()))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.click(ix, cx)))
                        // Right-click acts on this row: put the keyboard here first.
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, window, cx| {
                                this.cursor = ix;
                                this.focus.focus(window, cx);
                                cx.notify();
                            }),
                        ),
                )
            })
            .collect()
    }
}

impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.root.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_default();
        let prompt = self.prompt.as_ref().map(|(op, input, _)| {
            let label = match op {
                Op::New(dir) => {
                    let rel = dir.strip_prefix(&self.root).unwrap_or(dir).display().to_string();
                    if rel.is_empty() { "New file or folder/".to_string() } else { format!("New in {rel}/") }
                }
                Op::Rename(_) => "Rename to".to_string(),
            };
            div()
                .key_context("TreePrompt")
                .on_action(cx.listener(Self::cancel_prompt))
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .gap_1()
                .border_b_1()
                .border_color(theme.border)
                .child(div().text_xs().text_color(theme.muted_foreground).child(label))
                .child(Input::new(input))
        });
        // The list owns the tree's single-letter keys; the prompt sits outside
        // it so typing a name never triggers them.
        let list = div()
            .track_focus(&self.focus)
            .key_context(if self.asking.is_some() { "FileTree asking" } else { "FileTree" })
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::expand))
            .on_action(cx.listener(Self::collapse))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::new_entry))
            .on_action(cx.listener(Self::rename_entry))
            .on_action(cx.listener(Self::delete_entry))
            .on_action(cx.listener(|this, _: &GhostAccept, _, cx| this.ghost_key(crate::editor::ReviewKey::Accept, cx)))
            .on_action(cx.listener(|this, _: &GhostSkip, _, cx| this.ghost_key(crate::editor::ReviewKey::Skip, cx)))
            .flex_1()
            .min_h_0()
            .relative()
            .child({
                let width = self.width.clone();
                canvas(move |bounds, _, _| width.set(bounds.size.width), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
            })
            .child(
                uniform_list("files", self.display_rows().len(), cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)))
                    .track_scroll(&self.scroll)
                    .size_full(),
            );
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(name),
            )
            .children(prompt)
            .child(list);
        div()
            .id("file-tree")
            .size_full()
            .child(root)
            .context_menu(|menu, _, _| {
                menu.menu("New File or Folder…", Box::new(NewEntry))
                    .menu("Rename…", Box::new(RenameEntry))
                    .separator()
                    .menu("Move to Trash", Box::new(DeleteEntry))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::list_dir;

    #[test]
    fn lists_folders_first_and_skips_ignored() {
        let dir = std::env::temp_dir().join(format!("ghostwrite-tree-{}", std::process::id()));
        for d in ["src", "target", ".git"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["b.rs", "A.md", ".gitignore"] {
            std::fs::write(dir.join(f), "target/\n").unwrap();
        }
        let names: Vec<String> =
            list_dir(&dir).into_iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["src", ".gitignore", "A.md", "b.rs"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn creates_and_renames_entries() {
        use super::{create_entry, rename_entry};
        let dir = std::env::temp_dir().join(format!("ghostwrite-ops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = create_entry(&dir, "src/lib.rs").unwrap();
        assert!(file.is_file());
        assert!(create_entry(&dir, "docs/").unwrap().is_dir());
        assert!(create_entry(&dir, "src/lib.rs").is_err());
        assert!(create_entry(&dir, "../escape").is_err());
        let moved = rename_entry(&file, "main.rs").unwrap();
        assert_eq!(moved, dir.join("src/main.rs"));
        assert!(moved.is_file() && !file.exists());
        assert!(rename_entry(&moved, "../../x").is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
