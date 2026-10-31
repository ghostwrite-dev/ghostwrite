//! The editor: everything inside a project window is one modal editor.
//!
//! The model is the [`Project`]: the open files ([`Tabs`] of [`Document`]s
//! and their views), the file tree, the language servers. Each feature is a
//! type that works on it: a [`story`] (asked of the model, or one of the
//! editor's [`stops`]: command output, diagnostics, search matches), and the
//! completion menu ([`language`]).
//! [`Editor`] is the gpui view around all of that: it routes keys, owns focus
//! and prompts, runs background work, and decides when to switch tabs.
//!
//! Every key goes through [`Editor::key_down`] as a modal [`Key`]: the
//! completion menu gets first say in insert mode, then the mode decides.
//! Normal, select and insert mode edit the active document; tree mode
//! drives the file tree; AI mode reviews a story, with the review keys;
//! terminal mode sends them to the terminal's shell. Text typed in insert
//! mode arrives through the platform's input handler.
//!
//! - [`project`], [`tabs`], [`document`]: the model
//! - [`commands`]: `:` commands, in the [`prompt`]
//! - [`input`]: text from the platform
//! - [`story`] (drawn by [`story_view`]), [`stops`], [`shell`], [`terminal`], [`language`], [`tree`]: features
//! - [`typeset`]: notes about a Typst file, typeset; [`preview`]: its pages, compiled as it's typed, beside it;
//!   [`goals`]: a Lean file's goals at the cursor, beside it; [`semantic`]: the server's colors over the grammar's
//! - [`view`] and [`element`]: drawing; [`pdf_view`] draws a PDF's pages instead of its text; [`preview_view`] a Typst file's

mod commands;
mod document;
mod element;
mod find;
mod goals;
mod input;
mod jumps;
mod language;
mod pdf_view;
mod picker;
mod preview;
mod preview_view;
mod project;
mod prompt;
mod repeat;
mod semantic;
mod server_actions;
mod shell;
mod stops;
mod story;
mod story_view;
mod tabs;
mod terminal;
mod tree;
mod typeset;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range as Bytes;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use ghostwrite_ai::Backend;
use ghostwrite_core::modal::{Outcome, Registers, Target};
use ghostwrite_core::{Buffer, Effect, Key, Modal, Mode, Range, Review};
use ghostwrite_project::store::{self, Session};
use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Keystroke, PathPromptOptions, Pixels, PromptLevel,
    Subscription, Window,
};

pub use document::{DocId, Document};
pub use project::Project;
pub use tabs::{Autoscroll, Tab, Tabs};

use crate::keys::key_of;
use prompt::{Prompt, PromptKind};

pub enum EditorEvent {
    /// `:project`: the projects page, or straight into the given folder.
    OpenProject(Option<PathBuf>),
    /// `:q`, `:wq`: close the window, asking about unsaved changes unless `force`d.
    CloseWindow { force: bool },
}

/// How the editor starts.
pub struct Options {
    /// Start language servers for the files opened.
    pub language_servers: bool,
    /// Show Typst files compiled, beside them.
    pub previews: bool,
    /// What `space a` asks, the one in use first (the API with a key, else Claude Code).
    pub backends: Vec<Backend>,
}

impl Default for Options {
    fn default() -> Self {
        Self { language_servers: true, previews: true, backends: Backend::available() }
    }
}

/// What the code view measured when it last drew.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measured {
    /// Lines that fit on screen (for paging).
    pub rows: usize,
    /// The cursor, in window coordinates (for the completion menu and input methods).
    pub cursor: Option<Bounds<Pixels>>,
    /// The first line on screen (`gt`).
    pub top_line: Option<usize>,
}

pub struct Editor {
    project: Project,
    focus: FocusHandle,
    modal: Modal,
    /// The yank register, shared by all documents.
    yanked: String,
    /// Named registers (`"a y`).
    registers: HashMap<char, String>,
    tree_pane: tree::TreePane,
    prompt: Option<Prompt>,
    /// Earlier `:` commands, oldest first, and where ↑/↓ is in them.
    history: Vec<String>,
    history_at: Option<usize>,
    cheatsheet: bool,
    /// What `space a` asks, the one in use first.
    backends: Vec<Arc<Backend>>,
    story: Option<story::StoryState>,
    /// The last `:sh` command, running or done.
    shell: Option<shell::Shell>,
    last_shell: Option<String>,
    /// The user's shell, under the code.
    terminal: terminal::TerminalPane,
    /// The code should take the keys back at the next draw (a field that had them went away).
    pub(super) refocus: bool,
    /// Completion questions asked: only the latest answer is shown.
    completions_asked: u64,
    /// The active file's rows, wrapped: for a version of the text and a width.
    wrapped: Option<element::Wrapped>,
    /// The signature of the call being typed.
    signature: Option<server_actions::Signature>,
    /// `.` and macros.
    recorder: repeat::Recorder,
    /// `space f`, `space b`.
    picker: Option<picker::Picker>,
    /// Where `ctrl-o` goes back to.
    jumps: jumps::Jumps,
    /// Searching the file (`/`, `n`).
    find: find::Find,
    /// The next redraw while Claude is thinking.
    ticking: Option<gpui::Task<()>>,
    /// A click in the code is being dragged: it selects.
    pub(super) text_drag: bool,
    preview_pane: preview::PreviewPane,
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

/// The registers and the clipboard, for modal commands: the yank
/// register, `+` (and `*`) the clipboard, `_` nothing, and named ones.
struct Regs<'a> {
    yanked: &'a mut String,
    named: &'a mut HashMap<char, String>,
    cx: &'a mut App,
}

impl Registers for Regs<'_> {
    fn yanked(&self, register: Option<char>) -> String {
        match register {
            None => self.yanked.clone(),
            Some('+' | '*') => self.clipboard().unwrap_or_default(),
            Some('_') => String::new(),
            Some(name) => self.named.get(&name).cloned().unwrap_or_default(),
        }
    }

    fn yank(&mut self, register: Option<char>, text: String) {
        match register {
            None => *self.yanked = text,
            Some('+' | '*') => self.set_clipboard(text),
            Some('_') => {}
            Some(name) => {
                self.named.insert(name, text);
            }
        }
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
        let (events, mut from_servers) = futures::channel::mpsc::unbounded();
        let servers =
            if options.language_servers { ghostwrite_lsp::Registry::new(root.clone(), events) } else { ghostwrite_lsp::Registry::disabled(root.clone()) };
        // Diagnostics, and files to show, arrive from the servers' reader threads.
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = from_servers.next().await {
                let applied = this.update_in(cx, |this, window, cx| match event {
                    ghostwrite_lsp::Event::Diagnostics { path, version, diagnostics } => {
                        if this.project.apply_diagnostics(&path, version, diagnostics) {
                            cx.notify();
                        }
                    }
                    ghostwrite_lsp::Event::ApplyEdit { edit } => this.apply_workspace_edit(&edit, cx),
                    ghostwrite_lsp::Event::Exited { server, why } => {
                        this.project.server_exited(server, &why);
                        cx.notify();
                    }
                    ghostwrite_lsp::Event::Show { path, selection } => this.show_document(&path, selection, window, cx),
                    ghostwrite_lsp::Event::Recolor { server } => {
                        for tab in this.project.tabs.iter_mut().filter(|t| t.doc.lsp.as_ref().is_some_and(|l| l.server() == server)) {
                            tab.doc.semantic.stale();
                        }
                        cx.notify();
                    }
                    ghostwrite_lsp::Event::Progress { path, checking } => {
                        if this.project.apply_progress(&path, checking) {
                            cx.notify();
                        }
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
            project: Project::new(root, servers, options.previews),
            focus,
            modal: Modal::default(),
            yanked: String::new(),
            registers: HashMap::new(),
            tree_pane: tree::TreePane::new(),
            prompt: None,
            history: store::command_history(),
            history_at: None,
            cheatsheet: false,
            backends: options.backends.into_iter().map(Arc::new).collect(),
            story: None,
            shell: None,
            last_shell: None,
            terminal: terminal::TerminalPane::default(),
            preview_pane: preview::PreviewPane::new(),
            text_drag: false,
            ticking: None,
            find: Default::default(),
            jumps: Default::default(),
            picker: None,
            recorder: Default::default(),
            signature: None,
            wrapped: None,
            completions_asked: 0,
            refocus: false,
            completion: None,
            marked: None,
            measured: Measured::default(),
            _watcher: watcher,
            _subscriptions: subscriptions,
        }
    }

    /// Write every file with unsaved changes; whether they all were.
    pub fn write_all(&mut self) -> bool {
        self.project.write_all();
        !self.project.is_dirty()
    }

    /// How many files have unsaved changes.
    pub fn dirty_count(&self) -> usize {
        self.project.tabs.iter().filter(|t| t.doc.dirty).count()
    }

    pub fn is_dirty(&self) -> bool {
        self.project.is_dirty()
    }

    /// Out of select mode: going to another pane ends a selection, so coming
    /// back (or a click there) doesn't carry it on.
    fn stop_selecting(&mut self) {
        if self.modal.mode == Mode::Select {
            self.modal.mode = Mode::Normal;
        }
    }

    /// The project's folder.
    pub fn root(&self) -> &std::path::Path {
        &self.project.root
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

    /// Another program changed `paths`: read them again in the background.
    fn disk_changed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let reread = self.project.to_reread(paths);
        let read = cx.background_spawn(async move { reread.read() });
        cx.spawn(async move |this, cx| {
            let disk = read.await;
            this.update(cx, |this, cx| this.reread(disk, cx)).ok();
        })
        .detach();
    }

    fn reread(&mut self, disk: project::Disk, cx: &mut Context<Self>) {
        // An insert session can't outlive its text being replaced.
        let replaced = self.tab().is_some_and(|t| !t.doc.dirty && t.doc.path.as_deref().and_then(|p| disk.text(p)).is_some_and(|text| text != t.doc.text()));
        if replaced {
            self.leave_insert();
        }
        if self.project.reread(disk) {
            self.completion = None;
            cx.notify();
        }
    }

    /// A pane's edge is being dragged: the mouse isn't selecting.
    fn resizing(&self) -> bool {
        self.tree_pane.resizing || self.terminal.resizing.is_some() || self.preview_pane.resizing
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
        let from = self.before_jump();
        if ix != self.project.tabs.active_index() {
            self.leave_insert();
        }
        self.project.activate(ix);
        // Another file is a jump, however it was got to (`ctrl-o` comes back).
        self.note_jump(from);
        self.tab_changed(cx);
    }

    fn tab_changed(&mut self, cx: &mut Context<Self>) {
        self.completion = None;
        self.marked = None;
        // So a crash loses no more than the last change of tabs.
        self.save_session();
        cx.notify();
    }

    /// Open each path in its own tab, switching to it if it's already open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            // A PDF is read aside (a long one takes seconds), then opened.
            let pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            if pdf && self.project.tabs.index_of_path(&path).is_none() {
                self.open_pdf(ghostwrite_project::clean(&path), cx);
                continue;
            }
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

    /// ⌘S. If another program changed the file since it was read, ask
    /// whose version to keep.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        let Some(path) = tab.doc.path.clone() else { return self.save_as(window, cx) };
        if !tab.doc.changed_on_disk() {
            if self.formats_on_save() {
                let id = tab.doc.id;
                return self.format(
                    move |this, cx| {
                        if let Some(ix) = this.project.tabs.index_of_id(id) {
                            this.project.write(ix, path, false);
                        }
                        cx.notify();
                    },
                    cx,
                );
            }
            self.project.write(self.project.tabs.active_index(), path, false);
            return cx.notify();
        }
        let message = format!("{} changed on disk since it was opened", tab.doc.name());
        let detail = "Saving writes your version over the one on disk. Reloading takes the disk's (undo brings yours back).";
        let answer = window.prompt(PromptLevel::Warning, &message, Some(detail), &["Save Mine", "Reload Theirs", "Cancel"], cx);
        let id = tab.doc.id;
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                let Some(ix) = this.project.tabs.index_of_id(id) else { return };
                match answer {
                    Ok(0) => {
                        this.project.write(ix, path, true);
                    }
                    Ok(1) => match std::fs::read_to_string(&path) {
                        Ok(text) => this.project.tabs.get_mut(ix).into_iter().for_each(|tab| tab.reload(text.clone())),
                        Err(e) => this.project.say(format!("couldn't reload: {e}")),
                    },
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                // The dialog asked before writing over anything.
                if let Some(ix) = this.project.tabs.index_of_id(id) {
                    this.project.write(ix, path, true);
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

    /// Open the PDF at `path` once it's read, off the UI thread.
    fn open_pdf(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.project.say(format!("reading {}…", self.project.display(&path)));
        let reading = cx.background_spawn({
            let path = path.clone();
            async move { ghostwrite_pdf::Pdf::open(&path) }
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            this.update(cx, |this, cx| {
                match read {
                    Ok(pdf) => {
                        let ix = this.project.tabs.index_of_path(&path).unwrap_or_else(|| this.project.add_pdf(path, pdf));
                        this.project.status = None;
                        this.activate(ix, cx);
                    }
                    Err(e) => this.project.say(format!("couldn't open {}: {e}", path.display())),
                }
                cx.notify();
            })
            .ok();
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
        self.signature = None;
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
        if self.modal.mode == Mode::Terminal {
            let handled = self.terminal_key(keystroke);
            if handled {
                cx.notify();
            }
            return handled;
        }
        let Some(key) = key_of(keystroke) else { return false };
        // A message lasts until the next key (which may say something new),
        // and so do the marked matches, unless it's about searching.
        self.project.status = None;
        if !matches!(key, ghostwrite_core::Key::Char('n' | 'N' | '*')) {
            self.find.shown = false;
        }
        let from = self.before_jump();
        self.record_key(key);
        let handled = (self.modal.mode == Mode::Insert && self.menu_key(key, cx)) || self.mode_key(key, window, cx);
        self.recorded_key(key);
        if !matches!(key, ghostwrite_core::Key::Ctrl('o' | 'i')) {
            self.note_jump(from);
        }
        if self.modal.take_refused() {
            self.project.say("no editing while a story's up: tab accepts, ⏎ redirects, esc esc ends it");
        }
        if handled {
            cx.notify();
        }
        handled
    }

    /// A review key, for the story under review.
    fn review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.mode == Mode::Ai {
            self.story_review(key, window, cx);
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
        let mut regs = Regs { yanked: &mut self.yanked, named: &mut self.registers, cx };
        let outcome = match self.project.tabs.active_mut() {
            Some(tab) => {
                let before = (tab.doc.buffer.version(), tab.view.selection);
                let outcome = self.modal.key(key, &mut regs, &mut tab.target(rows));
                // A PDF's text can't change: no inserting, and an edit is undone as it syncs.
                let pdf = tab.doc.pdf.is_some();
                let refused = pdf && (self.modal.mode == Mode::Insert || tab.doc.buffer.version() != before.0);
                if pdf && self.modal.mode == Mode::Insert {
                    self.modal.to_normal(&mut tab.target(rows));
                }
                if (tab.doc.buffer.version(), tab.view.selection) != before {
                    tab.view.autoscroll = Some(Autoscroll::Fit);
                    tab.sync();
                }
                if refused {
                    self.project.say("a PDF is read-only");
                }
                outcome
            }
            None => {
                // Effects (`:`, `space e`, `?`) work with nothing open; edits go nowhere.
                let (mut buffer, mut selection) = (Buffer::default(), Range::default());
                let indent = Default::default();
                let mut target = Target { buffer: &mut buffer, selection: &mut selection, indent: &indent, page_rows: rows, language: None };
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
            Effect::Find { backward } => self.start_find(backward, window, cx),
            Effect::FindNext { backward } => self.find_next(backward, cx),
            Effect::PickFile => self.open_picker(picker::PickerKind::Files, window, cx),
            Effect::References => self.references(cx),
            Effect::Rename => self.start_rename(window, cx),
            Effect::CodeActions => self.code_actions(window, cx),
            Effect::Symbols => self.symbols(window, cx),
            Effect::PickBuffer => self.open_picker(picker::PickerKind::Buffers, window, cx),
            Effect::Repeat => self.repeat(window, cx),
            Effect::RecordMacro => self.record_macro(),
            Effect::PlayMacro => self.play_macro(window, cx),
            Effect::View(place) => self.view_cursor(place, cx),
            Effect::ScreenLine(place) => self.screen_line(place, cx),
            Effect::Diagnostic { forward } => self.next_diagnostic(forward, cx),
            Effect::JumpBack => self.jump(true, cx),
            Effect::JumpForward => self.jump(false, cx),
            Effect::FindSelection => {
                self.find_selection(cx);
                self.find_next(false, cx);
            }
            Effect::GotoDefinition => self.goto_definition(window, cx),
            Effect::Hover => self.show_hover(window, cx),
            Effect::NextProblem => self.next_problem(cx),
            Effect::Diagnostics => self.diagnostics_story(cx),
            Effect::Terminal => self.enter_terminal(cx),
        }
    }
}
