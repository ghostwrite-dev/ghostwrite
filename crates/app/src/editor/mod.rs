//! The editor: everything inside a project window is one modal editor.
//!
//! The model is the [`Project`]: the open files ([`Tabs`](tabs::Tabs) of [`Document`](document::Document)s
//! and their views), the file tree, the language servers. Each feature is a
//! type that works on it: a [`story`] (asked of the model, or one of the
//! editor's [`stops`]: command output, diagnostics, search matches), and the
//! completion menu ([`language`]).
//! [`Editor`] is the gpui view around all of that: it routes keys, owns focus
//! and prompts, runs background work, and decides when to switch tabs.
//!
//! Every key goes through [`Editor::key_down`] as a modal [`Key`], to the
//! [`views`] that has the keys: the code, Files, the terminal, a plugin's view.
//! The modes are the same in each. In the code, the completion menu gets first
//! say in insert mode, then the mode decides: normal, select and insert mode
//! edit the active document, and AI mode reviews a story, with the review keys.
//! In Files the keys drive the tree; on the terminal, insert mode sends them to
//! its shell, and the others move through what it shows. Text typed in insert
//! mode arrives through the platform's input handler.
//!
//! - [`project`], [`tabs`], [`document`]: the model, from [`ghostwrite_state`]
//! - [`commands`]: `:` commands, in the [`prompt`]
//! - [`input`]: text from the platform; [`pointer`]: the mouse in the code
//! - [`views`]: which view has the keys, and going from one to the next
//! - [`story`], [`stops`], [`terminal`], [`tree`], [`find`], [`picker`], [`jumps`], [`repeat`]: features
//! - [`language`], [`server_actions`], [`semantic`]: what language servers do
//! - [`rendered`]: a document's pages, whatever makes them
//! - [`aside`]: what's beside a file, from a plugin
//! - [`plugin_host`]: plugins' commands and events, and `:plugin`
//! - drawing: [`view`] (the window) and [`element`] (the code), and for
//!   each feature that draws, its `_view`: [`story_view`],
//!   [`terminal_view`], [`page_view`]
//!   draws a document's pages instead of its text

mod aside;
pub(crate) mod attention;
mod commands;
mod cursors;
mod debug;
mod element;
mod find;
mod input;
mod jumps;
mod language;
mod page_view;
mod picker;
mod plugin_host;
mod pointer;
mod prompt;
mod reading;
mod recovery;
mod rendered;
mod repeat;
mod semantic;
mod server_actions;
mod split;
mod stops;
mod story;
mod story_view;
mod terminal;
mod terminal_view;
mod tree;
mod update;
mod view;
mod views;

#[cfg(test)]
mod tests;

use std::ops::Range as Bytes;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use ghostwrite_ai::Backend;
use ghostwrite_core::modal::{self, Outcome, Target};
use ghostwrite_core::{Buffer, Command, Effect, Key, Modal, Mode, Range, Review, ViewKind};
use ghostwrite_project::store::{self, Session};
use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Keystroke, PathPromptOptions, Pixels, PromptLevel,
    Subscription, Window,
};

use ghostwrite_state::registers::Registers;
pub use ghostwrite_state::{Autoscroll, DocId, Project, Tab};
use ghostwrite_state::{document, project, tabs};

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
    /// Show what's beside files.
    pub asides: bool,
    /// The model `space a` asks, if one can be reached.
    pub backend: Option<Backend>,
}

impl Default for Options {
    fn default() -> Self {
        Self { language_servers: true, asides: true, backend: Backend::available() }
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
    // The model, and how keys edit it.
    project: Project,
    modal: Modal,
    registers: Registers,
    recorder: repeat::Recorder,
    jumps: jumps::Jumps,

    // Typing: the `:` line and other prompts, the platform's input, and
    // what language servers say while typing.
    prompt: Option<Prompt>,
    history: commands::History,
    /// Text being composed with an input method, in the active document.
    marked: Option<Bytes<usize>>,
    completion: Option<language::CompletionMenu>,
    completions_asked: language::Latest,
    /// The signature of the call being typed.
    signature: Option<server_actions::Signature>,
    signature_asked: language::Latest,

    // Features.
    /// What `space a` asks, if a model can be reached.
    backend: Option<Arc<Backend>>,
    story: Option<story::Live>,
    find: find::Find,
    picker: Option<picker::Picker>,
    cheatsheet: bool,
    /// Where the cheatsheet is scrolled to.
    cheatsheet_scroll: gpui::ScrollHandle,

    // The panes around the code.
    tree_pane: tree::TreePane,
    terminal: terminal::TerminalPane,
    aside_width: crate::ui::pane::Resizable,
    /// Where what's beside the file is scrolled to.
    aside_scroll: gpui::ScrollHandle,

    // On screen.
    focus: FocusHandle,
    /// The code should take the keys back at the next draw (a field that had them went away).
    refocus: bool,
    pointer: pointer::Pointer,
    /// The active file's rows, wrapped: for a version of the text and a width.
    wrapped: Option<element::Wrapped>,
    /// The same for what's in the other pane.
    wrapped_other: Option<element::Wrapped>,
    /// What's lit up, for the eye to go to.
    attention: attention::Attention,
    /// Where `:plugin` keeps plugins.
    plugins_at: Option<PathBuf>,
    /// A plugin fetched and checked by `:plugin install`, and where from, until `:plugin install!` installs it.
    staged_plugin: Option<(String, ghostwrite_plugin::install::Staged)>,
    /// Where the catalog of plugins to install is, and where it's kept: what's suggested
    /// for a file no plugin handles.
    catalog: Option<(String, PathBuf)>,
    _watching_links: Option<gpui::Task<()>>,
    /// Breakpoints, and what's being debugged.
    debug: debug::DebugPane,
    /// A second pane beside the one with the keys.
    split: Option<split::Split>,
    measured: Measured,
    /// The next redraw while the model is thinking.
    ticking: Option<gpui::Task<()>>,
    /// What the last story changed: each document, its steps, and its version when the story ended.
    last_story: ghostwrite_state::story::Undo,
    /// Copies of unsaved changes, for after a crash.
    recovery: recovery::Recovery,
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
    registers: &'a mut Registers,
    cx: &'a mut App,
}

impl modal::Registers for Regs<'_> {
    fn yanked(&self, register: Option<char>) -> String {
        match register {
            None => self.registers.yanked.clone(),
            Some('+' | '*') => self.clipboard().unwrap_or_default(),
            Some('_') => String::new(),
            Some(name) => self.registers.named.get(&name).cloned().unwrap_or_default(),
        }
    }

    fn yank(&mut self, register: Option<char>, text: String) {
        match register {
            None => self.registers.yanked = text,
            Some('+' | '*') => self.set_clipboard(text),
            Some('_') => {}
            Some(name) => {
                self.registers.named.insert(name, text);
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
        crate::plugins::install();
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
                    ghostwrite_lsp::Event::Notification { method, params, .. } => this.server_notification(&method, &params, cx),
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
                    this.forget_git_base();
                    cx.notify();
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut editor = Self {
            project: Project::new(root, servers, options.asides),
            modal: Modal::default(),
            registers: Registers::default(),
            recorder: Default::default(),
            jumps: Default::default(),
            prompt: None,
            history: commands::History::load(),
            marked: None,
            completion: None,
            completions_asked: Default::default(),
            signature: None,
            signature_asked: Default::default(),
            backend: options.backend.map(Arc::new),
            story: None,
            find: Default::default(),
            picker: None,
            cheatsheet: false,
            cheatsheet_scroll: gpui::ScrollHandle::new(),
            tree_pane: tree::TreePane::new(),
            terminal: terminal::TerminalPane::default(),
            aside_width: aside::width(),
            aside_scroll: gpui::ScrollHandle::new(),
            focus,
            refocus: false,
            pointer: Default::default(),
            wrapped: None,
            wrapped_other: None,
            split: None,
            attention: Default::default(),
            // A test says where, not to touch what's really installed.
            plugins_at: if cfg!(test) { None } else { crate::plugins::folder() },
            staged_plugin: None,
            catalog: if cfg!(test) { None } else { crate::plugins::catalog() },
            _watching_links: None,
            debug: Default::default(),
            measured: Measured::default(),
            ticking: None,
            last_story: vec![],
            recovery: Default::default(),
            _watcher: watcher,
            _subscriptions: subscriptions,
        };
        // A test has the default keys, not those of whoever runs it.
        if !cfg!(test) {
            let config = crate::config::get();
            editor.use_keys(&config.keymap, &config.problems);
        }
        editor.start_recovery(cx);
        editor.check_for_update(false, cx);
        editor.watch_linked_plugins(cx);
        editor
    }

    /// Keys looked up in `keymap` from now on (yours, from the config), and
    /// what's wrong with those that couldn't be bound said.
    fn use_keys(&mut self, keymap: &'static ghostwrite_core::Keymap, problems: &[String]) {
        self.modal.set_keymap(keymap);
        if !problems.is_empty() {
            self.project.say(format!("config.json \"keys\": {}", problems.join("; ")));
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

    /// A commit may have moved what the files are compared with.
    fn forget_git_base(&mut self) {
        for tab in self.project.tabs.iter_mut() {
            tab.doc.git.asked = false;
        }
    }

    /// Another program changed `paths`: read them again in the background.
    fn disk_changed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if paths.iter().any(|path| path.components().any(|part| part.as_os_str() == ".git")) {
            self.forget_git_base();
        }
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
        let replaced =
            self.tab().is_some_and(|t| !t.doc.dirty && t.doc.path.as_deref().and_then(|p| disk.text(p)).is_some_and(|text| *t.doc.buffer.rope() != *text));
        if replaced {
            self.leave_insert();
        }
        if self.project.reread(disk) {
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
            self.wander();
        }
        self.split_follows(ix);
        let from = self.before_jump();
        if ix != self.project.tabs.active_index() {
            self.leave_insert();
        }
        self.project.activate(ix);
        // Another file is a jump, however it was got to (`ctrl-o` comes back).
        self.note_jump(from);
        self.suggest_plugin(cx);
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
            if let Some(ix) = self.project.open(&path) {
                self.activate(ix, cx);
            }
        }
    }

    pub(super) fn new_buffer(&mut self, cx: &mut Context<Self>) {
        let ix = self.project.add(None, String::new());
        self.activate(ix, cx);
    }

    fn close_active(&mut self, cx: &mut Context<Self>) {
        self.leave_insert();
        self.project.close(self.project.tabs.active_index());
        self.tab_changed(cx);
    }

    /// Move the tab at `from` to where `to` is.
    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from != to {
            self.project.tabs.move_tab(from, to);
            self.tab_changed(cx);
        }
    }

    /// Move the active tab `places` along, as far as there are.
    fn nudge_tab(&mut self, right: bool, places: usize, cx: &mut Context<Self>) {
        let (at, n) = (self.project.tabs.active_index(), self.project.tabs.len());
        let to = if right { at.saturating_add(places).min(n.saturating_sub(1)) } else { at.saturating_sub(places) };
        self.move_tab(at, to, cx);
    }

    /// `steps` tabs along, round the ends.
    fn step_tab(&mut self, forward: bool, steps: usize, cx: &mut Context<Self>) {
        let (n, at) = (self.project.tabs.len(), self.project.tabs.active_index());
        if n > 0 {
            let steps = steps % n;
            self.activate(if forward { (at + steps) % n } else { (at + n - steps) % n }, cx);
        }
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
                    Ok(1) => match ghostwrite_project::fs::read_text(&path).map(|(text, _)| text) {
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
        if self.modal.mode != Mode::Insert || !self.in_code() {
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
        // j k scroll the cheatsheet; any other key closes it.
        if self.cheatsheet && !keystroke.modifiers.platform && self.scroll_cheatsheet(keystroke, cx) {
            return true;
        }
        if self.cheatsheet && !keystroke.modifiers.platform {
            self.cheatsheet = false;
            cx.notify();
            return true;
        }
        self.settle_view(cx);
        if self.typing_in_terminal() {
            let handled = self.terminal_key(keystroke, cx);
            if handled {
                cx.notify();
            }
            return handled;
        }
        let Some(key) = key_of(keystroke) else { return false };
        // A message lasts until the next key (which may say something new),
        // and so do the marked matches, unless it's about searching.
        self.project.status = None;
        let command = self.modal.command_for(key);
        use ghostwrite_core::Command::*;
        if !matches!(command, Some(SearchNext | SearchPrev | SearchSelectionDetectWordBounds | SearchSelection)) {
            self.find.shown = false;
        }
        let from = self.before_jump();
        self.record_key(key, command);
        let handled = (self.modal.mode == Mode::Insert && self.in_code() && self.menu_key(key, cx)) || self.mode_key(key, window, cx);
        self.recorded_key(command);
        if !matches!(command, Some(JumpBackward | JumpForward)) {
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

    /// Which file is in front, and where in it: what moving around changes.
    fn whereabouts(&self) -> (Option<DocId>, Option<ghostwrite_core::Range>) {
        (self.tab().map(|tab| tab.doc.id), self.tab().map(|tab| tab.view.selection))
    }

    /// A key for the current mode. Looking elsewhere with one while a story's up is
    /// noted: the story waits for you to come back.
    fn mode_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let before = (self.modal.mode == Mode::Ai).then(|| self.whereabouts());
        let handled = self.mode_key_here(key, window, cx);
        let story_moved_them = self.story.as_mut().is_some_and(|state| std::mem::take(&mut state.moved_them));
        if !story_moved_them && before.is_some_and(|before| before != self.whereabouts()) {
            self.wander();
        }
        handled
    }

    fn mode_key_here(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.settle_view(cx);
        if self.in_code() && self.cursor_key(key, cx) {
            return true;
        }
        let outcome = if let Some(command) = self.plugin_key(key) {
            self.modal.cancel_pending();
            // An editor command by its name, or a command line.
            match Command::named(command) {
                Some(command) => self.buffer_key(Press::Command(command), cx),
                None => {
                    if let Err(why) = self.run_command(command, window, cx) {
                        self.project.say(why);
                    }
                    return true;
                }
            }
        } else if self.modal.view.without_text() {
            self.modal.view_key(key)
        } else {
            self.buffer_key(Press::Key(key), cx)
        };
        self.outcome(outcome, window, cx)
    }

    /// Carry out what a key came to: whether it was anything.
    fn outcome(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match outcome {
            Outcome::Ignored => return false,
            Outcome::Handled => {}
            Outcome::Effect(effect) => self.effect(effect, window, cx),
            Outcome::Tree(command) => self.tree_command(command, window, cx),
            Outcome::Review(review) => self.review(review, window, cx),
        }
        true
    }

    /// The language keys go by: the file's, or for a view of a file, that file's.
    fn active_language(&self) -> Option<&'static ghostwrite_core::Language> {
        let doc = &self.tab()?.doc;
        match &doc.view_of {
            Some((file, _)) => self.project.tabs.by_id(*file)?.doc.language,
            None => doc.language,
        }
    }

    /// The command a plugin binds this key to in the view with the keys, the keys so
    /// far included. The keymap comes first: yours are in it, and the view's own.
    fn plugin_key(&self, key: Key) -> Option<&'static str> {
        if !matches!(self.modal.mode, Mode::Normal | Mode::Select) || self.modal.binds(key) {
            return None;
        }
        let name = match key {
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("ctrl-{c}"),
            _ => return None,
        };
        let pending = self.modal.pending_keys();
        let typed = if pending.is_empty() { name } else { format!("{pending} {name}") };
        // Those for the code are for a tab of its pages too, which is in a pane as the code is.
        let view = self.modal.view;
        let here = |binding: &&ghostwrite_plugin::Binding| binding.view == view || binding.view == ViewKind::Code && self.in_code();
        ghostwrite_plugin::keys(self.active_language()).into_iter().filter(here).find(|binding| binding.keys == typed).map(|binding| binding.command)
    }

    /// A key (or a command, run as if by one) for the active document, or the terminal's
    /// text while it's being read (or, with none open, for the commands that work anyway).
    fn buffer_key(&mut self, press: Press, cx: &mut Context<Self>) -> Outcome {
        if self.modal.view.without_text() {
            let Press::Command(command) = press else { return Outcome::Ignored };
            let (mut buffer, mut selection, indent) = (Buffer::default(), Range::default(), Default::default());
            let mut target = Target { buffer: &mut buffer, selection: &mut selection, indent: &indent, page_rows: 1, language: None };
            let mut regs = Regs { registers: &mut self.registers, cx };
            return self.modal.run_named(command, &mut regs, &mut target);
        }
        let reading = self.reading();
        let rows = if reading { self.reading_rows() } else { self.measured.rows.max(2) };
        let mut edited = false;
        let mut regs = Regs { registers: &mut self.registers, cx };
        let keyed = if reading { self.terminal.reading.as_mut().map(|reading| &mut reading.tab) } else { self.project.tabs.active_mut() };
        let outcome = match keyed {
            Some(tab) if reading => {
                let version = tab.doc.buffer.version();
                let outcome = press.at_each(&mut self.modal, &mut regs, tab, rows);
                if self.modal.mode == Mode::Insert {
                    self.modal.to_normal(&mut tab.target(rows));
                }
                edited = tab.doc.buffer.version() != version;
                outcome
            }
            Some(tab) => {
                let before = (tab.doc.buffer.version(), tab.view.selection);
                let outcome = press.at_each(&mut self.modal, &mut regs, tab, rows);
                // What's read-only: no inserting, and an edit is undone as it syncs.
                let read_only = tab.doc.read_only();
                let refused = read_only && (self.modal.mode == Mode::Insert || tab.doc.buffer.version() != before.0);
                if read_only && self.modal.mode == Mode::Insert {
                    self.modal.to_normal(&mut tab.target(rows));
                }
                if (tab.doc.buffer.version(), tab.view.selection) != before {
                    tab.view.autoscroll = Some(Autoscroll::Fit);
                    tab.sync();
                }
                if refused {
                    self.project.say("this is read-only");
                }
                outcome
            }
            None => {
                // Effects (`:`, `space e`, `?`) work with nothing open; edits go nowhere.
                let (mut buffer, mut selection) = (Buffer::default(), Range::default());
                let indent = Default::default();
                let mut target = Target { buffer: &mut buffer, selection: &mut selection, indent: &indent, page_rows: rows, language: None };
                let outcome = match press {
                    Press::Key(key) => self.modal.key(key, &mut regs, &mut target),
                    Press::Command(command) => self.modal.run_named(command, &mut regs, &mut target),
                };
                if self.modal.mode == Mode::Insert {
                    self.modal.to_normal(&mut target);
                }
                outcome
            }
        };
        if reading {
            self.read_on(edited);
        }
        if self.modal.mode != Mode::Insert {
            self.completion = None;
        }
        outcome
    }

    /// Carry out what a key asked for beyond the buffer.
    fn effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        // Reading the terminal: what's about the code goes back to it first.
        if self.reading() {
            match effect {
                Effect::Assist => return self.ask_about_output(window, cx),
                Effect::View(_) | Effect::ScreenLine(_) | Effect::Repeat => return,
                // There's one cursor on the terminal.
                Effect::CopySelections { .. }
                | Effect::KeepPrimarySelection
                | Effect::RemovePrimarySelection
                | Effect::RotateSelections { .. }
                | Effect::MergeSelections { .. }
                | Effect::AlignSelections
                | Effect::SelectRegex => return,
                Effect::Find { .. }
                | Effect::FindNext { .. }
                | Effect::FindSelection { .. }
                | Effect::CommandLine
                | Effect::Cheatsheet
                | Effect::RecordMacro
                | Effect::PlayMacro
                | Effect::GotoFile
                | Effect::Terminal
                | Effect::Files
                | Effect::FocusCode
                | Effect::Pages
                | Effect::Aside
                | Effect::TypeInTerminal => {}
                _ => self.focus_on(ViewKind::Code, cx),
            }
        }
        match effect {
            Effect::GotoFile => self.goto_file(window, cx),
            Effect::NextBuffer(steps) => self.step_tab(true, steps, cx),
            Effect::PrevBuffer(steps) => self.step_tab(false, steps, cx),
            Effect::MoveBufferNext(places) => self.nudge_tab(true, places, cx),
            Effect::MoveBufferPrev(places) => self.nudge_tab(false, places, cx),
            Effect::Files if self.modal.view == ViewKind::Files => self.focus_on(ViewKind::Code, cx),
            Effect::Files => self.enter_files(cx),
            Effect::FocusCode => self.focus_on(ViewKind::Code, cx),
            Effect::Pages => self.toggle_pages(cx),
            Effect::Aside => self.toggle_aside(cx),
            Effect::TypeInTerminal => self.type_in_terminal(cx),
            Effect::Scroll { by, pages } => self.scroll_view(by, pages, cx),
            Effect::Zoom(by) => {
                self.zoom_pages(by, cx);
            }
            Effect::SplitRight => {
                if let Err(why) = self.split_right(cx) {
                    self.project.say(why);
                }
            }
            Effect::OtherPane => self.other_pane(cx),
            Effect::CloseSplit => self.close_split(cx),
            Effect::CommandLine => self.open_prompt(PromptKind::Command, "", window, cx),
            Effect::Search => self.open_prompt(PromptKind::Command, "grep ", window, cx),
            Effect::Assist => self.open_story("", window, cx),
            Effect::Cheatsheet => {
                self.cheatsheet = true;
                self.cheatsheet_scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
            }
            Effect::Find { backward } => self.start_find(backward, window, cx),
            Effect::FindNext { backward, count } => self.find_next(backward, count, self.modal.mode == Mode::Select, cx),
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
            Effect::JumpBack(count) => self.jump(true, count, cx),
            Effect::JumpForward(count) => self.jump(false, count, cx),
            Effect::FindSelection { word_bounds } => self.find_selection(word_bounds, cx),
            Effect::GotoDefinition => self.goto_definition(window, cx),
            Effect::Hover => self.show_hover(window, cx),
            Effect::NextProblem => self.next_problem(cx),
            Effect::Diagnostics => self.diagnostics_story(cx),
            Effect::Terminal if self.modal.view == ViewKind::Terminal => self.focus_on(ViewKind::Code, cx),
            Effect::Terminal => self.type_in_terminal(cx),
            Effect::Run(command) => {
                if let Err(why) = self.run_command(command, window, cx) {
                    self.project.say(why);
                }
            }
            Effect::CopySelections { .. }
            | Effect::KeepPrimarySelection
            | Effect::RemovePrimarySelection
            | Effect::RotateSelections { .. }
            | Effect::MergeSelections { .. }
            | Effect::AlignSelections
            | Effect::SelectRegex => self.cursors(effect, window, cx),
        }
    }
}

/// What a key is for the document: the key, or a command run as if one were bound to it.
#[derive(Clone, Copy)]
enum Press {
    Key(Key),
    Command(Command),
}

impl Press {
    /// Carry it out at each of `tab`'s cursors (a command, at the main one).
    fn at_each(self, modal: &mut Modal, regs: &mut Regs, tab: &mut Tab, rows: usize) -> Outcome {
        match self {
            Press::Key(key) => cursors::key_at_each(modal, key, regs, tab, rows),
            Press::Command(command) => modal.run_named(command, regs, &mut tab.target(rows)),
        }
    }
}
