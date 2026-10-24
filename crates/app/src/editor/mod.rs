//! The editor: everything inside a project window is one modal editor.
//!
//! The model is the [`Project`]: the open files ([`Tabs`] of [`Document`]s
//! and their views), the file tree, the language servers. Each feature is a
//! type that works on it: a Claude [`story`], a [`tour`] of command output,
//! diagnostics or search matches, and the completion menu ([`language`]).
//! [`Editor`] is the gpui view around all of that: it routes keys, owns focus
//! and prompts, runs background work, and decides when to switch tabs.
//!
//! Every key goes through [`Editor::key_down`] as a modal [`Key`]: the
//! completion menu gets first say in insert mode, then the mode decides.
//! Normal, select and insert mode edit the active document; tree mode
//! drives the file tree; AI mode reviews a story and tour mode a tour, with
//! the review keys. Text typed in insert mode arrives through the
//! platform's input handler.
//!
//! - [`project`], [`tabs`], [`document`]: the model
//! - [`commands`]: `:` commands
//! - [`story`] (drawn by [`story_view`]), [`shell`], [`tour`], [`language`], [`tree`]: features
//! - [`view`] and [`element`]: drawing

mod commands;
mod document;
mod element;
mod language;
mod project;
mod shell;
mod story;
mod story_view;
mod tabs;
mod tour;
mod tree;
mod view;

#[cfg(test)]
mod tests;

use std::ops::Range as Bytes;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use ghostwrite_ai::Backend;
use ghostwrite_core::modal::{Outcome, Registers, Target};
use ghostwrite_core::{Buffer, Effect, Key, Modal, Mode, Range, Review};
use ghostwrite_project::store::{self, Session};
use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Keystroke, PathPromptOptions,
    Pixels, Point, PromptLevel, Subscription, UTF16Selection, UniformListScrollHandle, Window, px,
};

pub use document::{DocId, Document};
pub use project::Project;
pub use tabs::{Autoscroll, Tab, Tabs};

use crate::keys::key_of;
use crate::ui::text_field::{TextField, TextFieldEvent};

pub enum EditorEvent {
    /// `:project`: the projects page, or straight into the given folder.
    OpenProject(Option<PathBuf>),
}

/// How the editor starts.
pub struct Options {
    /// Start language servers for the files opened.
    pub language_servers: bool,
    /// How `space i` reaches Claude, if it can.
    pub backend: Option<Backend>,
}

impl Default for Options {
    fn default() -> Self {
        Self { language_servers: true, backend: Backend::from_env() }
    }
}

/// What a prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptKind {
    Command,
    /// Asking Claude, or saying something about the item under review.
    Ask,
    /// Naming a new file or folder in this folder.
    TreeAdd(PathBuf),
    TreeRename(PathBuf),
}

/// The one open prompt: it has the keyboard until it closes.
struct Prompt {
    kind: PromptKind,
    field: Entity<TextField>,
    _subscription: Subscription,
}

/// What the code view measured when it last drew.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measured {
    /// Lines that fit on screen (for paging).
    pub rows: usize,
    /// The cursor, in window coordinates (for the completion menu and input methods).
    pub cursor: Option<Bounds<Pixels>>,
}

pub struct Editor {
    project: Project,
    focus: FocusHandle,
    modal: Modal,
    /// The yank register, shared by all documents.
    yanked: String,
    show_tree: bool,
    tree_width: Pixels,
    tree_scroll: UniformListScrollHandle,
    /// Dragging the tree's edge.
    resizing_tree: bool,
    prompt: Option<Prompt>,
    /// Earlier `:` commands, oldest first, and where ↑/↓ is in them.
    history: Vec<String>,
    history_at: Option<usize>,
    cheatsheet: bool,
    backend: Option<Arc<Backend>>,
    story: Option<story::StoryState>,
    tour: Option<tour::TourState>,
    /// The last `:sh` command, running or done.
    shell: Option<shell::Shell>,
    last_shell: Option<String>,
    completion: Option<language::CompletionMenu>,
    /// Text being composed with an input method, in the active document.
    marked: Option<Bytes<usize>>,
    measured: Measured,
    /// Watches the project for changes made by other programs.
    _watcher: Option<ghostwrite_project::watch::Watcher>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The yank register and the clipboard, for modal commands.
struct Regs<'a> {
    yanked: &'a mut String,
    cx: &'a mut App,
}

impl Registers for Regs<'_> {
    fn yanked(&self) -> String {
        self.yanked.clone()
    }

    fn yank(&mut self, text: String) {
        *self.yanked = text;
    }

    fn clipboard(&self) -> Option<String> {
        self.cx.read_from_clipboard().and_then(|c| c.text())
    }

    fn set_clipboard(&mut self, text: String) {
        self.cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

impl Editor {
    pub fn new(root: PathBuf, options: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (events, mut diagnostics) = futures::channel::mpsc::unbounded();
        let servers = if options.language_servers {
            ghostwrite_lsp::Registry::new(root.clone(), events)
        } else {
            ghostwrite_lsp::Registry::disabled(root.clone())
        };
        // Diagnostics arrive from the servers' reader threads.
        cx.spawn(async move |this, cx| {
            while let Some(ghostwrite_lsp::Event::Diagnostics { path, diagnostics }) = diagnostics.next().await {
                let applied = this.update(cx, |this, cx| {
                    if this.project.apply_diagnostics(&path, diagnostics) {
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
        // Other programs' changes arrive from the watcher's thread.
        let (changes, mut changed) = futures::channel::mpsc::unbounded();
        let watcher = ghostwrite_project::watch::watch(&root, move |paths| {
            changes.unbounded_send(paths).ok();
        })
        .ok();
        cx.spawn(async move |this, cx| {
            while let Some(paths) = changed.next().await {
                let applied = this.update(cx, |this, cx| this.disk_changed(&paths, cx));
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
        let subscriptions = vec![
            // Remember the open files for next time.
            cx.on_app_quit(|this: &mut Self, _| {
                this.save_session();
                async {}
            }),
            // Files may have changed while we were away.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.project.tree.refresh();
                    cx.notify();
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            project: Project::new(root, servers),
            focus,
            modal: Modal::default(),
            yanked: String::new(),
            show_tree: true,
            tree_width: px(store::tree_width().unwrap_or(240.)),
            tree_scroll: UniformListScrollHandle::new(),
            resizing_tree: false,
            prompt: None,
            history: store::command_history(),
            history_at: None,
            cheatsheet: false,
            backend: options.backend.map(Arc::new),
            story: None,
            tour: None,
            shell: None,
            last_shell: None,
            completion: None,
            marked: None,
            measured: Measured::default(),
            _watcher: watcher,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.project.is_dirty()
    }

    pub fn save_session(&self) {
        store::save_session(&self.project.root, &self.project.session());
    }

    pub fn restore(&mut self, session: &Session, cx: &mut Context<Self>) {
        self.project.restore(session);
        self.tab_changed(cx);
    }

    fn tab(&self) -> Option<&Tab> {
        self.project.tab()
    }

    /// Another program changed `paths`.
    fn disk_changed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // An insert session can't outlive its text being replaced.
        if self.tab().is_some_and(|t| !t.doc.dirty && t.doc.path.as_ref().is_some_and(|p| paths.contains(p))) {
            self.leave_insert();
        }
        if self.project.disk_changed(paths) {
            self.completion = None;
            cx.notify();
        }
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.project.tab_mut()
    }

    /// The tab being edited, or the message for commands that need one.
    fn current(&self) -> Result<&Tab, String> {
        self.tab().ok_or_else(|| "no file open".to_string())
    }

    // Switching tabs: the editor's part is closing an insert session first
    // (as one undo step) and dropping what was about the old tab.

    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix != self.project.tabs.active_index() {
            self.leave_insert();
        }
        self.project.activate(ix);
        self.tab_changed(cx);
    }

    fn tab_changed(&mut self, cx: &mut Context<Self>) {
        self.completion = None;
        self.marked = None;
        cx.notify();
    }

    /// Open each path in its own tab, switching to it if it's already open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if let Some(ix) = self.project.open(&path) {
                self.activate(ix, cx);
            }
        }
    }

    fn new_buffer(&mut self, cx: &mut Context<Self>) {
        let ix = self.project.add(None, String::new());
        self.activate(ix, cx);
    }

    fn close_active(&mut self, cx: &mut Context<Self>) {
        self.leave_insert();
        self.project.close(self.project.tabs.active_index());
        self.tab_changed(cx);
    }

    fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        let (n, at) = (self.project.tabs.len(), self.project.tabs.active_index());
        if n > 0 {
            self.activate(if forward { (at + 1) % n } else { (at + n - 1) % n }, cx);
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        match tab.doc.path.clone() {
            Some(path) => self.project.write(self.project.tabs.active_index(), path),
            None => self.save_as(window, cx),
        }
        cx.notify();
    }

    fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        let dir = tab.doc.path.as_deref().and_then(std::path::Path::parent).map_or(self.project.root.clone(), std::path::Path::to_path_buf);
        let answer = cx.prompt_for_new_path(&dir, Some(&tab.doc.name()));
        let id = tab.doc.id;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = answer.await else { return };
            this.update(cx, |this, cx| {
                // The tab may have moved or closed while the dialog was up.
                if let Some(ix) = this.project.tabs.index_of_id(id) {
                    this.project.write(ix, path);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            this.update(cx, |this, cx| this.open_paths(paths, cx)).ok();
        })
        .detach();
    }

    /// Close the active tab, asking first if it has unsaved changes.
    fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        if !tab.doc.dirty {
            return self.close_active(cx);
        }
        let message = format!("Close {} without saving?", tab.doc.name());
        let answer = window.prompt(PromptLevel::Warning, &message, Some("Your changes will be lost."), &["Close", "Cancel"], cx);
        let id = tab.doc.id;
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update(cx, |this, cx| {
                if let Some(ix) = this.project.tabs.index_of_id(id) {
                    this.activate(ix, cx);
                    this.close_active(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // Keys.

    /// Close an insert session in the active document, as one undo step.
    fn leave_insert(&mut self) {
        if self.modal.mode != Mode::Insert {
            return;
        }
        self.marked = None;
        self.completion = None;
        match self.project.tabs.active_mut() {
            Some(tab) => self.modal.to_normal(&mut tab.target(1)),
            None => self.modal.mode = Mode::Normal,
        }
    }

    /// Every key in the editor comes here first (a prompt takes its own keys).
    fn key_down(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cheatsheet && !keystroke.modifiers.platform {
            self.cheatsheet = false;
            cx.notify();
            return true;
        }
        let Some(key) = key_of(keystroke) else { return false };
        let handled = (self.modal.mode == Mode::Insert && self.menu_key(key, cx)) || self.mode_key(key, window, cx);
        if handled {
            cx.notify();
        }
        handled
    }

    /// A review key, for what the mode is reviewing.
    fn review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        match self.modal.mode {
            Mode::Ai => self.story_review(key, window, cx),
            Mode::Tour => self.tour_review(key, window, cx),
            _ => {}
        }
    }

    /// A key for the current mode.
    fn mode_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let outcome = if self.modal.mode == Mode::Tree { self.modal.tree_key(key) } else { self.buffer_key(key, cx) };
        match outcome {
            Outcome::Ignored => return false,
            Outcome::Handled => {}
            Outcome::Effect(effect) => self.effect(effect, window, cx),
            Outcome::Tree(command) => self.tree_command(command, window, cx),
            Outcome::Review(review) => self.review(review, window, cx),
        }
        true
    }

    /// A key for the active document (or, with none open, for the commands that work anyway).
    fn buffer_key(&mut self, key: Key, cx: &mut Context<Self>) -> Outcome {
        let rows = self.measured.rows.max(2);
        let mut regs = Regs { yanked: &mut self.yanked, cx };
        let outcome = match self.project.tabs.active_mut() {
            Some(tab) => {
                let before = (tab.doc.buffer.version(), tab.view.selection);
                let outcome = self.modal.key(key, &mut regs, &mut tab.target(rows));
                if (tab.doc.buffer.version(), tab.view.selection) != before {
                    tab.view.autoscroll = Some(Autoscroll::Fit);
                    tab.sync();
                }
                outcome
            }
            None => {
                // Effects (`:`, `space e`, `?`) work with nothing open; edits go nowhere.
                let (mut buffer, mut selection) = (Buffer::default(), Range::default());
                let indent = Default::default();
                let mut target = Target { buffer: &mut buffer, selection: &mut selection, indent: &indent, page_rows: rows };
                let outcome = self.modal.key(key, &mut regs, &mut target);
                if self.modal.mode == Mode::Insert {
                    self.modal.to_normal(&mut target);
                }
                outcome
            }
        };
        if self.modal.mode != Mode::Insert {
            self.completion = None;
        }
        outcome
    }

    /// Carry out what a key asked for beyond the buffer.
    fn effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::NextBuffer => self.cycle_tab(true, cx),
            Effect::PrevBuffer => self.cycle_tab(false, cx),
            Effect::FocusTree => self.enter_tree(cx),
            Effect::CommandLine => self.open_prompt(PromptKind::Command, "", window, cx),
            Effect::Search => self.open_prompt(PromptKind::Command, "grep ", window, cx),
            Effect::Assist => self.open_story("", window, cx),
            Effect::Cheatsheet => self.cheatsheet = true,
            Effect::GotoDefinition => self.goto_definition(window, cx),
            Effect::Hover => self.show_hover(window, cx),
            Effect::Diagnostics => self.diagnostics_tour(cx),
        }
    }

    /// Typed text, in insert mode (anywhere else it's ignored).
    fn insert_text(&mut self, text: &str, replacing: Option<Bytes<usize>>, cx: &mut Context<Self>) {
        if self.modal.mode != Mode::Insert {
            return;
        }
        let Some(tab) = self.project.tabs.active_mut() else { return };
        if let Some(range) = replacing {
            tab.view.selection = Range::new(range.start, range.end).clamp(tab.doc.buffer.len());
        }
        self.modal.insert(text, &mut tab.target(1));
        tab.view.autoscroll = Some(Autoscroll::Fit);
        tab.sync();
        self.after_typing(text, cx);
        cx.notify();
    }

    // Prompts.

    fn open_prompt(&mut self, kind: PromptKind, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match kind {
            PromptKind::Ask => "Ask Claude, or tell it what to change…",
            PromptKind::Command => "",
            PromptKind::TreeAdd(_) => "name, or name/ for a folder",
            PromptKind::TreeRename(_) => "new name",
        };
        let field = cx.new(|cx| TextField::new(text, placeholder, cx));
        let subscription = cx.subscribe_in(&field, window, |this, field, event, window, cx| {
            let text = field.read(cx).text().to_string();
            this.prompt_event(*event, text, window, cx);
        });
        field.read(cx).focus(window);
        if kind == PromptKind::Command {
            self.history_at = None;
            self.project.status = None;
        }
        self.prompt = Some(Prompt { kind, field, _subscription: subscription });
        cx.notify();
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn asking(&self) -> bool {
        self.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Ask)
    }

    /// The prompt's field while asking Claude.
    fn ask_field(&self) -> Option<Entity<TextField>> {
        self.prompt.as_ref().filter(|p| p.kind == PromptKind::Ask).map(|p| p.field.clone())
    }

    fn prompt_event(&mut self, event: TextFieldEvent, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.prompt.as_ref().map(|p| p.kind.clone()) else { return };
        match (event, kind) {
            (TextFieldEvent::Submit, PromptKind::Command) => {
                self.close_prompt(window, cx);
                self.remember_command(&text);
                if let Err(e) = self.run_command(&text, window, cx) {
                    self.project.status = (!e.is_empty()).then(|| e.into());
                }
            }
            (TextFieldEvent::Submit, PromptKind::Ask) => self.ask(text, window, cx),
            (TextFieldEvent::Submit, PromptKind::TreeAdd(dir)) => {
                self.close_prompt(window, cx);
                self.tree_add(&dir, &text, cx);
            }
            (TextFieldEvent::Submit, PromptKind::TreeRename(from)) => {
                self.close_prompt(window, cx);
                self.tree_rename(&from, &text);
            }
            (TextFieldEvent::Cancel, PromptKind::Ask) => self.cancel_ask(window, cx),
            (TextFieldEvent::Cancel, _) => self.close_prompt(window, cx),
            (TextFieldEvent::Up | TextFieldEvent::Down, PromptKind::Command) => self.history_step(event == TextFieldEvent::Up, cx),
            (TextFieldEvent::Complete, PromptKind::Command) => self.complete_command(&text, cx),
            _ => {}
        }
        cx.notify();
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range: Bytes<usize>, actual: &mut Option<Bytes<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let text = self.tab()?.doc.text();
        let range = from_utf16(text, &range);
        actual.replace(to_utf16(text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let tab = self.tab()?;
        let sel = tab.view.selection;
        Some(UTF16Selection { range: to_utf16(tab.doc.text(), &sel.bytes()), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Bytes<usize>> {
        Some(to_utf16(self.tab()?.doc.text(), self.marked.as_ref()?))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.text(), &range?)));
        self.insert_text(text, replacing, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: Option<Bytes<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.text(), &range?)));
        let start = replacing.as_ref().map(|r| r.start).or_else(|| self.tab().map(|t| t.view.selection.from()));
        self.insert_text(text, replacing, cx);
        if self.modal.mode == Mode::Insert {
            self.marked = start.filter(|_| !text.is_empty()).map(|s| s..s + text.len());
        }
    }

    fn bounds_for_range(&mut self, _: Bytes<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        self.measured.cursor
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

fn to_utf16(text: &str, range: &Bytes<usize>) -> Bytes<usize> {
    let units = |at: usize| text[..at.min(text.len())].chars().map(char::len_utf16).sum::<usize>();
    units(range.start)..units(range.end)
}

fn from_utf16(text: &str, range: &Bytes<usize>) -> Bytes<usize> {
    let byte = |units: usize| {
        let mut count = 0;
        for (i, c) in text.char_indices() {
            if count >= units {
                return i;
            }
            count += c.len_utf16();
        }
        text.len()
    };
    byte(range.start)..byte(range.end)
}
