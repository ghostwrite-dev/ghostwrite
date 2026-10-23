use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::component::input::{
    self, Editor, EditorState, Input, InputEvent, InputState, TabSize, TextDecoration, TextDecorationCollection,
};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::assist::{self, Backend, EditRequest, Kind, Step};
use crate::command::{self, Command};
use crate::helix::Mode;
use crate::modal::{Effect, Modal};

mod lang;
mod review;
mod run;

use review::Assist;
pub use review::ReviewKey;
use run::ShellRun;

actions!(
    ghostwrite,
    [
        NewTab, Open, OpenProject, ShowProjects, ToggleFileTree, FocusFileTree, Save, SaveAs, CloseTab, NextTab, PrevTab,
        Quit, CancelCommand, CompleteCommand, OpenCommandLine, CancelAssist, ShellNext, ShellClose, HistoryPrev,
        HistoryNext, ToggleCheatsheet
    ]
);

pub enum EditorEvent {
    FocusTree,
    /// `:project`: the projects page, or straight into the given folder.
    OpenProject(Option<PathBuf>),
    /// Files were added or removed; the file tree should re-list.
    FilesChanged,
    /// A story step proposes creating or removing this: the tree shows it.
    ProposeCreate(Option<crate::tree::Proposal>),
    /// Asking about a file proposal: its prompt goes in the tree, under it.
    AskInTree(Option<Entity<InputState>>),
    /// An answer about a file proposal, shown in the tree under it.
    TreeNote(Option<String>),
}

pub fn bind_keys(cx: &mut App) {
    // Everything else is a modal key or a `:` command; ⌘Q stays because every Mac app quits that way.
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        // Later bindings win at the same depth, so these beat the input's own.
        KeyBinding::new("escape", CancelCommand, Some("CommandLine > Input")),
        KeyBinding::new("tab", CompleteCommand, Some("CommandLine > Input")),
        KeyBinding::new("up", HistoryPrev, Some("CommandLine > Input")),
        KeyBinding::new("down", HistoryNext, Some("CommandLine > Input")),
        KeyBinding::new("shift-/", ToggleCheatsheet, Some("EmptyEditor")),
        KeyBinding::new("escape", CancelAssist, Some("Assist > Input")),
        // With no file open there's no modal editor to catch these.
        KeyBinding::new("shift-;", OpenCommandLine, Some("EmptyEditor")),
        KeyBinding::new(":", OpenCommandLine, Some("EmptyEditor")),
        KeyBinding::new("space e", FocusFileTree, Some("EmptyEditor")),
        KeyBinding::new("tab", ShellNext, Some("EmptyEditor")),
        KeyBinding::new("escape", ShellClose, Some("EmptyEditor")),
    ]);
    cx.set_menus([
        Menu { name: "Ghostwrite".into(), items: vec![MenuItem::action("Quit Ghostwrite", Quit)], disabled: false },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("Open…", Open),
                MenuItem::action("Open Folder…", OpenProject),
                MenuItem::action("Switch Project…", ShowProjects),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As…", SaveAs),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", input::Undo),
                MenuItem::action("Redo", input::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", input::Cut),
                MenuItem::action("Copy", input::Copy),
                MenuItem::action("Paste", input::Paste),
                MenuItem::action("Select All", input::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle File Tree", ToggleFileTree),
                MenuItem::action("Focus File Tree", FocusFileTree),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
            ],
            disabled: false,
        },
    ]);
}

fn language_for(path: Option<&Path>) -> SharedString {
    path.and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("text").to_string().into()
}

struct Buffer {
    state: Entity<EditorState>,
    path: Option<PathBuf>,
    dirty: bool,
    /// The document as its language server knows it, if one serves the file.
    doc: Option<Rc<lang::Doc>>,
    /// One level of indent, detected from the file.
    indent: String,
    _subscription: Subscription,
}

impl Buffer {
    fn name(&self) -> SharedString {
        self.path.as_deref().and_then(|p| p.file_name()).map_or("untitled".into(), |n| n.to_string_lossy().into_owned().into())
    }

    /// An untitled, unedited tab, which opening a file may take over.
    fn is_pristine(&self, cx: &App) -> bool {
        self.path.is_none() && !self.dirty && self.state.read(cx).value().is_empty()
    }
}

pub struct TextEditor {
    buffers: Vec<Buffer>,
    active: usize,
    status: Option<SharedString>,
    root: PathBuf,
    project: SharedString,
    modal: Modal,
    /// The `:` prompt, while it's open.
    command: Option<(Entity<InputState>, Subscription)>,
    /// Takes the keyboard when no file is open.
    empty_focus: FocusHandle,
    /// How `space i` reaches Claude, if it can.
    ai: Option<Arc<Backend>>,
    /// The theme's caret color. We paint the buffer's cursor ourselves, so the
    /// theme's is hidden while a buffer has the keyboard and restored otherwise.
    caret: Hsla,
    /// The editor area on screen, measured each frame, for placing overlays.
    area: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Earlier `:` commands, oldest first, and where ↑/↓ is in them.
    history: Vec<String>,
    history_at: Option<usize>,
    /// The `?` cheatsheet is open.
    cheatsheet: bool,
    /// The project's language servers.
    servers: crate::lsp::Servers,
    /// A `:sh` command and its output, and the last command run.
    shell_run: Option<ShellRun>,
    last_shell: Option<String>,
    /// The open `space i` prompt.
    assist: Option<Assist>,
    _interceptor: Subscription,
}

impl EventEmitter<EditorEvent> for TextEditor {}

impl TextEditor {
    pub fn new(paths: Vec<PathBuf>, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned()).into();
        // Runs before keybindings, so modal keys win over the editor's own.
        let this = cx.entity().downgrade();
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            // The `space i` prompt floats inside the editor but is a text field of its own.
            let stack = &event.context_stack;
            if !stack.iter().any(|c| c.contains("Helix")) || stack.iter().any(|c| c.contains("Assist")) {
                return;
            }
            let Some(editor) = this.upgrade() else { return };
            if editor.update(cx, |editor, cx| editor.modal_key(&event.keystroke, window, cx)) {
                cx.stop_propagation();
            }
        });
        let modal = Modal::new();
        // Diagnostics arrive from the servers' reader threads.
        let (events, mut diagnostics) = futures::channel::mpsc::unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(crate::lsp::Event::Diagnostics { path, diagnostics }) = diagnostics.next().await {
                if this.update(cx, |this, cx| this.apply_diagnostics(&path, diagnostics, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let servers = crate::lsp::Servers::new(root.clone(), events);
        // Remember the open files for next time.
        cx.on_app_quit(|this: &mut Self, cx| {
            crate::projects::save_session(&this.root, &this.session(cx));
            async {}
        })
        .detach();
        let mut this = Self { servers, buffers: vec![], active: 0, status: None, root, project, modal, command: None, empty_focus: cx.focus_handle(), ai: Backend::from_env().map(Arc::new), caret: cx.theme().caret, area: Rc::default(), history: load_history(), history_at: None, cheatsheet: false, shell_run: None, last_shell: None, assist: None, _interceptor: interceptor };
        this.open_paths(paths, window, cx);
        this
    }

    fn push_buffer(&mut self, path: Option<PathBuf>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let (width, hard_tabs) = crate::helix::detect_indent(&text);
        let indent = if hard_tabs { "\t".to_string() } else { " ".repeat(width) };
        let doc = self.attach_lsp(path.as_deref(), &text);
        let providers = doc.clone();
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language_for(path.as_deref()))
                .line_number(true)
                .tab_size(TabSize { tab_size: width, hard_tabs })
                .default_value(text);
            if let Some(doc) = providers {
                // Docs come with `space k`, inline; the mouse only shows diagnostics.
                state.lsp_mut().completion_provider = Some(Rc::new(lang::Provider(doc)));
            }
            state
        });
        let subscription = cx.subscribe(&state, |this, state, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(buffer) = this.buffers.iter_mut().find(|b| b.state == state)
            {
                buffer.dirty = true;
                if let Some(doc) = &buffer.doc {
                    doc.changed(&state.read(cx).text().to_string());
                }
                cx.notify();
            }
        });
        if self.buffers.get(self.active).is_some_and(|b| b.is_pristine(cx)) {
            self.buffers.remove(self.active);
        }
        self.buffers.push(Buffer { state, path, dirty: false, doc, indent, _subscription: subscription });
        self.activate(self.buffers.len() - 1, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Leaving a file writes it, so disk (what Claude, :sh and the language
        // servers read) matches what you see.
        if ix != self.active
            && let Some(left) = self.buffers.get(self.active).filter(|b| b.dirty).and_then(|b| b.path.clone())
        {
            self.write(self.active, left, cx);
        }
        self.active = ix;
        self.status = None;
        self.modal.indent.clone_from(&self.buffers[ix].indent);
        self.buffers[ix].state.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Opens each path in its own tab, switching to it if it's already open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            let path = std::path::absolute(&path).unwrap_or(path);
            if let Some(ix) = self.buffers.iter().position(|b| b.path.as_ref() == Some(&path)) {
                self.activate(ix, window, cx);
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    self.status = Some(format!("couldn't open {}: {e}", path.display()).into());
                    continue;
                }
            };
            self.push_buffer(Some(path), text, window, cx);
        }
        cx.notify();
    }

    fn write(&mut self, ix: usize, path: PathBuf, cx: &mut Context<Self>) {
        let buffer = &mut self.buffers[ix];
        let text = buffer.state.read(cx).value();
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => {
                self.status = Some(format!("saved {}", path.display()).into());
                if buffer.path.as_ref() != Some(&path) {
                    let language = language_for(Some(&path));
                    buffer.state.update(cx, |s, cx| s.set_highlighter(language, cx));
                    if let Some(doc) = &buffer.doc {
                        doc.moved(&path, &text);
                    }
                }
                if let Some(doc) = &buffer.doc {
                    // rust-analyzer and friends check the project on save.
                    doc.saved();
                }
                buffer.path = Some(path);
                buffer.dirty = false;
            }
            Err(e) => self.status = Some(format!("couldn't save: {e}").into()),
        }
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.push_buffer(None, String::new(), window, cx);
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            this.update_in(cx, |this, window, cx| this.open_paths(paths, window, cx)).ok();
        })
        .detach();
    }

    /// The buffer being edited, or the message for commands that need one.
    fn current(&self) -> Result<&Buffer, String> {
        self.buffers.get(self.active).ok_or_else(|| "no file open".to_string())
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(buffer) = self.current() else { return };
        match buffer.path.clone() {
            Some(path) => self.write(self.active, path, cx),
            None => self.save_as(&SaveAs, window, cx),
        }
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(buffer) = self.current() else { return };
        let dir = buffer
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some(&buffer.name()));
        let state = buffer.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            this.update(cx, |this, cx| {
                // The tab may have moved or closed while the dialog was up.
                if let Some(ix) = this.buffers.iter().position(|b| b.state == state) {
                    this.write(ix, path, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(buffer) = self.current() else { return };
        if !buffer.dirty {
            return self.remove_active(window, cx);
        }
        let message = format!("Close {} without saving?", buffer.name());
        let answer = window.prompt(PromptLevel::Warning, &message, Some("Your changes will be lost."), &["Close", "Cancel"], cx);
        let state = buffer.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                if let Some(ix) = this.buffers.iter().position(|b| b.state == state) {
                    this.active = ix;
                    this.remove_active(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let removed = self.buffers.remove(self.active);
        if let Some(doc) = &removed.doc {
            doc.closed();
        }
        if self.buffers.is_empty() {
            self.active = 0;
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.activate(self.active.min(self.buffers.len() - 1), window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.buffers.is_empty() {
            return;
        }
        self.activate((self.active + 1) % self.buffers.len(), window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.buffers.is_empty() {
            return;
        }
        self.activate((self.active + self.buffers.len() - 1) % self.buffers.len(), window, cx);
    }

    fn modal_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cheatsheet && !keystroke.modifiers.platform {
            self.cheatsheet = false;
            cx.notify();
            return true;
        }
        if self.review_key(keystroke, window, cx) || (self.assist.is_none() && self.shell_key(keystroke, window, cx)) {
            cx.notify();
            return true;
        }
        let Some(state) = self.buffers.get(self.active).map(|b| b.state.clone()) else { return false };
        let Some(effect) = self.modal.key(keystroke, &state, window, cx) else { return false };
        match effect {
            Effect::None => {}
            Effect::NextBuffer => self.next_tab(&NextTab, window, cx),
            Effect::PrevBuffer => self.prev_tab(&PrevTab, window, cx),
            Effect::FocusTree => cx.emit(EditorEvent::FocusTree),
            Effect::CommandLine => self.open_command(window, cx),
            Effect::Assist => self.open_assist(window, cx),
            Effect::GotoDefinition => self.goto_definition(window, cx),
            Effect::Hover => self.show_hover(window, cx),
            Effect::Diagnostics => self.diagnostics_tour(window, cx),
            Effect::Search => self.open_command_with("grep ", window, cx),
            Effect::Cheatsheet => self.cheatsheet = true,
        }
        cx.notify();
        true
    }

    fn open_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_command_with("", window, cx);
    }

    /// The command line, with `typed` already in it.
    fn open_command_with(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let end = typed.len();
        let typed = typed.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(typed));
        // Typing goes after what's already there.
        input.update(cx, |s, cx| s.set_selected_range(end..end, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let line = input.read(cx).value().to_string();
                this.close_command(window, cx);
                this.remember_command(&line);
                if let Err(e) = this.run_command(&line, window, cx) {
                    this.status = (!e.is_empty()).then(|| e.into());
                }
                cx.notify();
            }
            InputEvent::Blur => {
                this.command = None;
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.command = Some((input, subscription));
        self.history_at = None;
        self.status = None;
    }

    fn remember_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.history.retain(|l| l != line);
        self.history.push(line.to_string());
        let keep = self.history.len().saturating_sub(200);
        self.history.drain(..keep);
        save_history(&self.history);
    }

    /// ↑ in the `:` line: the previous command; ↓ goes forward again.
    fn history_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.command else { return };
        let len = self.history.len();
        let at = match (self.history_at, back) {
            (None, true) if len > 0 => Some(len - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            (Some(_), false) => None,
            (None, _) => return,
        };
        self.history_at = at;
        let line = at.map(|i| self.history[i].clone()).unwrap_or_default();
        input.update(cx, |s, cx| s.set_value(line, window, cx));
        cx.notify();
    }

    fn close_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn cancel_command(&mut self, _: &CancelCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.close_command(window, cx);
    }

    fn complete_command(&mut self, _: &CompleteCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.command else { return };
        let line = input.read(cx).value().to_string();
        let (completed, candidates) = command::complete(&line, &self.root);
        if completed != line {
            input.update(cx, |s, cx| s.set_value(completed, window, cx));
        }
        self.status = (!candidates.is_empty()).then(|| candidates.join("  ").into());
        cx.notify();
    }

    /// Run a `:` command. `Err` carries the message for the status bar.
    fn run_command(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let dirty_error = |b: &Buffer| format!("{} has unsaved changes (add ! to discard them)", b.name());
        match command::parse(line)? {
            Command::Open(args) => {
                let paths: Vec<PathBuf> = args.iter().map(|a| command::resolve(a, &self.root)).collect();
                if let Some(dir) = paths.iter().find(|p| p.is_dir()) {
                    return Err(format!("{} is a folder", dir.display()));
                }
                self.open_paths(paths, window, cx);
            }
            Command::Write(path) => {
                let path = match path {
                    Some(p) => command::resolve(&p, &self.root),
                    None => self.current()?.path.clone().ok_or("no file name yet: use :write <path>")?,
                };
                self.write(self.active, path, cx);
            }
            Command::WriteAll { quit } => {
                for ix in 0..self.buffers.len() {
                    if let Some(path) = self.buffers[ix].path.clone()
                        && self.buffers[ix].dirty
                    {
                        self.write(ix, path, cx);
                    }
                }
                if let Some(b) = self.buffers.iter().find(|b| b.dirty) {
                    return Err(format!("couldn't write {}", b.name()));
                }
                self.status = Some("wrote all files".into());
                if quit {
                    cx.quit();
                }
            }
            Command::Remove { path, force } => {
                let target = match path {
                    Some(p) => command::resolve(&p, &self.root),
                    None => self.current()?.path.clone().ok_or("this buffer has no file to remove")?,
                };
                if !target.exists() {
                    return Err(format!("{} doesn't exist", target.display()));
                }
                let affected = |b: &Buffer| b.path.as_deref().is_some_and(|p| p.starts_with(&target));
                if let Some(b) = self.buffers.iter().find(|b| affected(b) && b.dirty)
                    && !force
                {
                    return Err(format!("{} has unsaved changes (use :rm! to remove it anyway)", b.name()));
                }
                crate::tree::move_to_trash(&target)?;
                for buffer in self.buffers.iter_mut().filter(|b| affected(b)) {
                    buffer.dirty = false;
                }
                self.path_deleted(&target, window, cx);
                let name = target.file_name().map_or_else(|| target.display().to_string(), |n| n.to_string_lossy().into_owned());
                self.status = Some(format!("moved {name} to the Trash").into());
                cx.emit(EditorEvent::FilesChanged);
            }
            Command::Reload { force } => {
                let buffer = self.current()?;
                if buffer.dirty && !force {
                    return Err(dirty_error(buffer));
                }
                let path = buffer.path.clone().ok_or("nothing to reload: this buffer has no file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't reload: {e}"))?;
                buffer.state.update(cx, |s, cx| s.set_value(text, window, cx));
                self.buffers[self.active].dirty = false;
            }
            Command::Tree => window.dispatch_action(Box::new(ToggleFileTree), cx),
            Command::Project(path) => {
                let path = path.map(|p| command::resolve(&p, &self.root));
                if let Some(p) = path.as_ref().filter(|p| !p.is_dir()) {
                    return Err(format!("{} isn't a folder", p.display()));
                }
                cx.emit(EditorEvent::OpenProject(path));
            }
            Command::Shell(command) => self.start_shell(command, window, cx)?,
            Command::Grep(pattern) => self.start_search(&pattern, window, cx)?,
            Command::QuitAll { force } => {
                if let Some(b) = self.buffers.iter().find(|b| b.dirty)
                    && !force
                {
                    return Err(dirty_error(b));
                }
                cx.quit();
            }
            Command::New => self.new_tab(&NewTab, window, cx),
            Command::BufferNext => self.next_tab(&NextTab, window, cx),
            Command::BufferPrev => self.prev_tab(&PrevTab, window, cx),
            Command::BufferClose { force } => {
                let buffer = self.current()?;
                if buffer.dirty && !force {
                    return Err(dirty_error(buffer));
                }
                self.remove_active(window, cx);
            }
            Command::Goto(line) => {
                let state = self.current()?.state.clone();
                state.update(cx, |st, cx| self.modal.goto_line(line, st, cx));
            }
        }
        Ok(())
    }

    pub fn set_status(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        cx.notify();
    }

    /// A file or folder was renamed: tabs at or below `from` follow it.
    pub fn path_moved(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for buffer in &mut self.buffers {
            let Some(rest) = buffer.path.as_deref().and_then(|p| p.strip_prefix(from).ok()) else { continue };
            let moved = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            if language_for(Some(&moved)) != language_for(buffer.path.as_deref()) {
                let language = language_for(Some(&moved));
                buffer.state.update(cx, |s, cx| s.set_highlighter(language, cx));
            }
            if let Some(doc) = &buffer.doc {
                doc.moved(&moved, &buffer.state.read(cx).text().to_string());
            }
            buffer.path = Some(moved);
        }
        cx.notify();
    }

    /// A file or folder went to the Trash: close its clean tabs; tabs with
    /// unsaved changes stay open so nothing is lost.
    pub fn path_deleted(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let gone = |b: &Buffer| b.path.as_deref().is_some_and(|p| p.starts_with(path));
        let kept = self.buffers.iter().filter(|b| gone(b) && b.dirty).count();
        let active_path = self.active_path();
        self.buffers.retain(|b| !gone(b) || b.dirty);
        if self.buffers.is_empty() {
            self.active = 0;
            self.focus_active(window, cx);
        } else {
            let ix = active_path.and_then(|p| self.buffers.iter().position(|b| b.path.as_ref() == Some(&p)));
            self.active = ix.unwrap_or(self.active.min(self.buffers.len() - 1));
        }
        if kept > 0 {
            self.status = Some(format!("{kept} deleted file(s) still open with unsaved changes").into());
        }
        cx.notify();
    }

    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.buffers.get(self.active) {
            Some(buffer) => buffer.state.update(cx, |s, cx| s.focus(window, cx)),
            None => self.empty_focus.focus(window, cx),
        }
    }

    #[cfg(test)]
    pub fn set_reply_for_test(&mut self, reply: &str) {
        self.ai = Some(Arc::new(crate::assist::Backend::Fixed(reply.to_string())));
    }

    /// Replies by round: the first ask, then each follow-up.
    #[cfg(test)]
    pub fn set_rounds_for_test(&mut self, replies: &[&str]) {
        self.ai = Some(Arc::new(crate::assist::Backend::Rounds(replies.iter().map(|r| r.to_string()).collect())));
    }

    /// The follow-ups asked so far in the open story.
    #[cfg(test)]
    pub fn followups_for_test(&self) -> Vec<String> {
        self.assist.as_ref().map(|a| a.history.iter().map(|(_, asked)| asked.clone()).collect()).unwrap_or_default()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The open files and their cursors, to bring back next time.
    pub fn session(&self, cx: &App) -> crate::projects::Session {
        let files: Vec<(PathBuf, u32, u32)> = self
            .buffers
            .iter()
            .filter_map(|b| {
                // Where the cursor's block starts (the editor reports its end in normal mode).
                let st = b.state.read(cx);
                let text = st.text().to_string();
                let at = st.selected_range().start.min(text.len());
                let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
                let (line, column) = (text[..at].matches('\n').count() as u32, text[line_start..at].chars().count() as u32);
                Some((b.path.clone()?, line, column))
            })
            .collect();
        let active = self.buffers.get(self.active).and_then(|a| files.iter().position(|(p, ..)| Some(p) == a.path.as_ref())).unwrap_or(0);
        crate::projects::Session { files, active }
    }

    /// Reopen a session's files (skipping ones that are gone) at their cursors.
    pub fn restore(&mut self, session: &crate::projects::Session, window: &mut Window, cx: &mut Context<Self>) {
        let existing: Vec<&(PathBuf, u32, u32)> = session.files.iter().filter(|(p, ..)| p.is_file()).collect();
        self.open_paths(existing.iter().map(|(p, ..)| p.clone()).collect(), window, cx);
        for (path, line, column) in &existing {
            let Some(buffer) = self.buffers.iter().find(|b| b.path.as_ref() == Some(path)) else { continue };
            let state = buffer.state.clone();
            state.update(cx, |st, cx| {
                let text = st.text().to_string();
                let line_start = text.match_indices('\n').nth((*line as usize).wrapping_sub(1)).map_or(0, |(i, _)| i + 1);
                let line_start = if *line == 0 { 0 } else { line_start };
                let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
                let at = text[line_start..line_end].char_indices().nth(*column as usize).map_or(line_end, |(i, _)| line_start + i);
                self.modal.select_bytes(at..at, st, cx);
                st.center_on_lines(*line as usize..*line as usize + 1, px(0.), cx);
            });
        }
        let active = existing.get(session.active).or(existing.last()).map(|(p, ..)| p.clone());
        if let Some(ix) = active.and_then(|p| self.buffers.iter().position(|b| b.path.as_ref() == Some(&p))) {
            self.activate(ix, window, cx);
        }
    }

    pub fn active_path(&self) -> Option<PathBuf> {
        self.buffers.get(self.active).and_then(|b| b.path.clone())
    }

    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.buffers.iter().any(|b| b.dirty)
    }

    fn render_tab(&self, ix: usize, cx: &mut Context<Self>) -> Tab {
        let buffer = &self.buffers[ix];
        let theme = cx.theme();
        let close = div()
            .id(("close", ix))
            .px_1()
            .rounded_sm()
            .text_color(theme.muted_foreground)
            .hover(|s| s.bg(theme.muted).text_color(theme.foreground))
            .child(if buffer.dirty { "●" } else { "×" })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active = ix;
                this.close_tab(&CloseTab, window, cx);
            }));
        Tab::new().label(buffer.name()).suffix(close)
    }
}

impl TextEditor {
    /// With no file open: a few ways in, instead of an empty untitled buffer.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let hint = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .gap_3()
                .child(div().w(px(96.)).text_right().font_family(theme.mono_font_family.clone()).text_color(theme.foreground).child(keys))
                .child(div().text_color(theme.muted_foreground).child(what))
        };
        div()
            .track_focus(&self.empty_focus)
            .key_context("EmptyEditor")
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .child(hint("space e", "browse the file tree"))
            .child(hint(":e <path>", "open a file"))
            .child(hint(":n", "new buffer"))
            .child(hint(":project", "switch project"))
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.modal.mode;
        let pending = self.modal.pending_label();
        // We draw the buffer's cursor ourselves (block, or bar in insert), so its own
        // blinking caret stays hidden; other text fields keep theirs.
        let theme_caret = cx.theme().caret;
        if theme_caret.a > 0. {
            self.caret = theme_caret;
        }
        let buffer_focused =
            self.buffers.get(self.active).is_some_and(|b| b.state.read(cx).focus_handle(cx).is_focused(window));
        let caret = if buffer_focused { transparent_black() } else { self.caret };
        if theme_caret != caret {
            Theme::global_mut(cx).caret = caret;
        }
        let (position, message, body) = match self.buffers.get(self.active) {
            Some(buffer) => {
                window.set_window_title(&format!("{}{} — {}", if buffer.dirty { "• " } else { "" }, buffer.name(), self.project));
                let st = buffer.state.read(cx);
                let cursor = st.cursor_position();
                let at = if mode == Mode::Insert { st.cursor() } else { self.modal.block_at(st) };
                let theme = cx.theme();
                let (color, text) = cursor_colors(mode, theme.is_dark());
                let block = BlockCursor {
                    state: buffer.state.clone(),
                    area: self.area.clone(),
                    at,
                    bar: mode == Mode::Insert,
                    color,
                    text,
                    font: theme.mono_font_family.clone(),
                    size: theme.mono_font_size,
                };
                let message = self.status.clone().unwrap_or_else(|| buffer.name());
                let state = buffer.state.clone();
                let assist = self.render_assist(&state, window, cx);
                self.mark_shell_line(&state.clone(), cx);
                let tabs: Vec<Tab> = (0..self.buffers.len()).map(|ix| self.render_tab(ix, cx)).collect();
                let body = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        TabBar::new("tabs")
                            .small()
                            .selected_index(self.active)
                            .children(tabs)
                            .on_click(cx.listener(|this, ix: &usize, window, cx| this.activate(*ix, window, cx))),
                    )
                    .child(
                        div()
                            .key_context("Helix")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(Editor::new(&state).h_full().bordered(false))
                            .child(block.into_canvas())
                            .children(assist),
                    )
                    .into_any_element();
                (format!("{}:{}", cursor.line + 1, cursor.character + 1), message, body)
            }
            None => {
                window.set_window_title(&self.project);
                let message = self.status.clone().unwrap_or_else(|| "no file open".into());
                (String::new(), message, self.render_empty(cx).into_any_element())
            }
        };
        let shell_panel = self.render_shell_panel(cx);
        let cheatsheet = self.cheatsheet.then(|| self.render_cheatsheet(cx));
        let theme = cx.theme();
        div()
            .key_context("TextEditor")
            .relative()
            .children(cheatsheet)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(|this, _: &OpenCommandLine, window, cx| this.open_command(window, cx)))
            .on_action(cx.listener(Self::shell_next))
            .on_action(cx.listener(|this, _: &HistoryPrev, window, cx| this.history_step(true, window, cx)))
            .on_action(cx.listener(|this, _: &HistoryNext, window, cx| this.history_step(false, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleCheatsheet, _, cx| {
                this.cheatsheet = !this.cheatsheet;
                cx.notify();
            }))
            .on_action(cx.listener(Self::shell_close))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(body)
            // `:sh` output, above the command line.
            .children(shell_panel)
            // The `:` prompt: its own line just above the statusline, in the editor font.
            .when_some(self.command.as_ref().map(|(input, _)| input.clone()), |d, input| {
                d.child(
                    div()
                        .key_context("CommandLine")
                        .on_action(cx.listener(Self::cancel_command))
                        .on_action(cx.listener(Self::complete_command))
                        .flex()
                        .items_center()
                        .h(px(26.))
                        .px_3()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(theme.mono_font_size)
                        .text_color(theme.foreground)
                        .child(":")
                        .child(
                            Input::new(&input)
                                .appearance(false)
                                .px_0()
                                .py_0()
                                .h(px(26.))
                                .flex_1()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        ),
                )
            })
            // Helix's statusline: mode and message on the left, position and
            // the language server on the right, one line that never wraps.
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .h(px(24.))
                    .px_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.status_bar)
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(match mode {
                                        Mode::Normal => theme.foreground,
                                        Mode::Insert => theme.success,
                                        Mode::Select => theme.warning,
                                    })
                                    .child(mode.label()),
                            )
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().child(message)),
                    )
                    .child(div().flex().flex_none().items_center().gap_3().child(pending).child(position)),
            )
    }
}

/// The `?` cheatsheet: every key, grouped like helix's keymap.
const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
    ("Move", &[
        ("h j k l", "left down up right"),
        ("w b e", "next word, back, word end (W B E: WORD)"),
        ("f t F T ‹c›", "find / till a char on the line"),
        ("gg ge", "file start, last line"),
        ("gh gl gs", "line start, end, first non-blank"),
        ("G  10G", "last line, line 10"),
        ("ctrl-d ctrl-u", "half page down, up"),
        ("gn gp", "next, previous tab"),
        ("gd", "go to definition"),
    ]),
    ("Select", &[
        ("v", "select mode (motions extend)"),
        ("x", "select line, again: next line"),
        ("%", "select all"),
        (";  alt-;", "collapse, flip selection"),
    ]),
    ("Edit", &[
        ("i a I A", "insert before, after, line start, end"),
        ("o O", "open line below, above"),
        ("d c", "delete, change selection"),
        ("y p P", "yank, paste after, before"),
        ("space y / p", "clipboard yank, paste"),
        ("r‹c›  ~", "replace with char, switch case"),
        ("> <", "indent, unindent"),
        ("u U", "undo, redo"),
    ]),
    ("Match  m", &[
        ("mm", "matching bracket"),
        ("ms‹c›", "surround with a pair"),
        ("mr‹a›‹b›", "replace surrounding pair"),
        ("md‹c›", "delete surrounding pair"),
        ("mi‹o›  ma‹o›", "select inside / around: w W p ( [ { < ' \" ` m"),
    ]),
    ("Language server", &[
        ("space k", "docs for what's under the cursor"),
        ("space d", "tour this file's diagnostics"),
        ("space /", "search the project (:grep)"),
        ("typing", "completion menu (enter picks)"),
    ]),
    ("Claude  space i", &[
        ("space i", "ask, or say what to change"),
        ("tab", "accept step · next note"),
        ("backspace", "skip"),
        ("enter", "redirect, or ask about it"),
        ("esc", "stop early"),
    ]),
    ("Command line  :", &[
        (":e ‹path›  :w  :wa", "open, write, write all"),
        (":n :bn :bp :bc", "new, next, previous, close tab"),
        (":rl  :rm  :42", "reload, trash file, go to line"),
        (":sh ‹cmd›", "run; tab through errors, enter fixes"),
        (":tree  :project  :qa", "file tree, projects, quit"),
        ("↑ ↓  tab", "history, complete"),
    ]),
    ("File tree  space e", &[
        ("j k  gg G", "move"),
        ("l h  enter", "open / preview, close, open file"),
        ("a r d", "add, rename, trash"),
        ("esc", "back to the editor"),
    ]),
];

impl TextEditor {
    fn render_cheatsheet(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let section = |(title, keys): &(&str, &[(&str, &str)])| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.warning).child(title.to_string()))
                .children(keys.iter().map(|(k, what)| {
                    div()
                        .flex()
                        .gap_3()
                        .text_xs()
                        .child(div().w(px(132.)).flex_none().font_family(theme.mono_font_family.clone()).text_color(theme.foreground).child(k.to_string()))
                        .child(div().text_color(theme.muted_foreground).child(what.to_string()))
                }))
        };
        let columns: Vec<Vec<usize>> = vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7]];
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background.opacity(0.6))
            .child(
                div()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Keys"))
                            .child(div().text_xs().text_color(theme.muted_foreground).child("any key closes")),
                    )
                    .child(div().flex().gap_6().children(columns.into_iter().map(|col| {
                        div().flex().flex_col().gap_4().children(col.into_iter().map(|i| section(&CHEATSHEET[i])))
                    }))),
            )
            .into_any_element()
    }
}

fn load_history() -> Vec<String> {
    let path = crate::projects::support_file("command-history");
    let text = path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn save_history(history: &[String]) {
    let Some(path) = crate::projects::support_file("command-history") else { return };
    let text: String = history.iter().map(|l| format!("{l}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

#[cfg(test)]
thread_local! {
    /// The last block painted, with the area it was clipped to.
    static LAST_BLOCK: std::cell::Cell<Option<(Bounds<Pixels>, Bounds<Pixels>)>> = const { std::cell::Cell::new(None) };
}

/// `ui.cursor.primary.{normal,insert,select}` from helix's gruvbox themes:
/// the block's fill, and the color of the char inside it.
fn cursor_colors(mode: Mode, dark: bool) -> (Hsla, Hsla) {
    let (fg3, blue1, orange1, bg1) =
        if dark { (0xbdae93, 0x83a598, 0xfe8019, 0x3c3836) } else { (0x665c54, 0x076678, 0xaf3a03, 0xebdbb2) };
    let fill = match mode {
        Mode::Normal => fg3,
        Mode::Insert => blue1,
        Mode::Select => orange1,
    };
    (rgb(fill).into(), rgb(bg1).into())
}

/// Helix's cursor with `cursor-shape.insert = "bar"`: a filled cell with the
/// char under it redrawn on top, or a thin bar in insert mode. Painted over the editor after it lays out, so the
/// position is from the same frame.
struct BlockCursor {
    state: Entity<EditorState>,
    /// Gets the editor area, which the canvas covers.
    area: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    at: usize,
    bar: bool,
    color: Hsla,
    text: Hsla,
    font: SharedString,
    size: Pixels,
}

impl BlockCursor {
    fn into_canvas(self) -> impl IntoElement {
        // Pinned to the corner: an absolute element without an offset lands
        // after its siblings, below the editor, and gets clipped away.
        canvas(|_, _, _| {}, move |bounds, _, window, cx| self.paint(bounds, window, cx)).absolute().top_0().left_0().size_full()
    }

    fn paint(&self, clip: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.area.set(Some(clip));
        let (caret, cell, ch) = {
            let st = self.state.read(cx);
            if !st.focus_handle(cx).is_focused(window) {
                return;
            }
            let Some(caret) = st.range_to_bounds(&(self.at..self.at)) else { return };
            let ch = st.text().chars_at(self.at).next().filter(|c| *c != '\n' && *c != '\r');
            let cell = ch
                .and_then(|c| st.range_to_bounds(&(self.at..self.at + c.len_utf8())))
                .filter(|b| b.origin.y == caret.origin.y && b.size.width > px(0.));
            (caret, cell, ch)
        };
        let line_height = caret.size.height;
        if self.bar {
            let bar = Bounds::new(caret.origin, size(px(2.), line_height));
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| window.paint_quad(fill(bar, self.color)));
            return;
        }
        let glyph = ch.filter(|c| !c.is_whitespace()).map(|c| {
            let text: SharedString = c.to_string().into();
            let run = TextRun {
                len: text.len(),
                font: font(self.font.clone()),
                color: self.text,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window.text_system().shape_line(text, self.size, &[run], None)
        });
        let width = cell.map(|b| b.size.width).or(glyph.as_ref().map(|g| g.width)).unwrap_or(self.size * 0.6);
        #[cfg(test)]
        LAST_BLOCK.set(Some((clip, Bounds::new(caret.origin, size(width, line_height)))));
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            window.paint_quad(fill(Bounds::new(caret.origin, size(width, line_height)), self.color));
            if let Some(glyph) = glyph {
                glyph.paint(caret.origin, line_height, TextAlign::Left, None, window, cx).ok();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

    use super::TextEditor;

    fn open(text: &str, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TextEditor>) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("ghostwrite-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.txt");
        std::fs::write(&path, text).unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| TextEditor::new(vec![path], dir, window, cx))
            })
            .unwrap()
        })
    }

    fn keys(window: AnyWindowHandle, keys: &str, cx: &mut TestAppContext) {
        cx.simulate_keystrokes(window, keys);
        cx.run_until_parked();
    }

    fn input(window: AnyWindowHandle, text: &str, cx: &mut TestAppContext) {
        cx.simulate_input(window, text);
        cx.run_until_parked();
    }

    fn value(editor: &Entity<TextEditor>, cx: &mut TestAppContext) -> String {
        editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).value().to_string())
    }

    #[gpui_kit::test]
    fn word_then_delete(cx: &mut TestAppContext) {
        let (w, editor) = open("foo bar baz\n", cx);
        keys(w, "w d", cx);
        assert_eq!(value(&editor, cx), "bar baz\n");
        // A count repeats the motion; only the last word stays selected.
        keys(w, "2 w d", cx);
        assert_eq!(value(&editor, cx), "bar \n");
        keys(w, "u", cx);
        assert_eq!(value(&editor, cx), "bar baz\n");
    }

    #[gpui_kit::test]
    fn normal_mode_swallows_letters(cx: &mut TestAppContext) {
        let (w, editor) = open("abc\n", cx);
        keys(w, "z q", cx);
        input(w, "m", cx);
        assert_eq!(value(&editor, cx), "abc\n");
    }

    #[gpui_kit::test]
    fn lines_yank_paste_and_open(cx: &mut TestAppContext) {
        let (w, editor) = open("one\ntwo\n", cx);
        keys(w, "x y j p", cx);
        assert_eq!(value(&editor, cx), "one\ntwo\none\n");
        keys(w, "g g x d", cx);
        assert_eq!(value(&editor, cx), "two\none\n");
        keys(w, "o", cx);
        input(w, "new", cx);
        keys(w, "escape", cx);
        assert_eq!(value(&editor, cx), "two\nnew\none\n");
        keys(w, "A", cx);
        input(w, "!", cx);
        keys(w, "escape g e d", cx);
        assert_eq!(value(&editor, cx), "two\nnew!\nne\n");
    }

    #[gpui_kit::test]
    fn find_and_change(cx: &mut TestAppContext) {
        let (w, editor) = open("let x = foo(bar);\n", cx);
        keys(w, "t ( c", cx);
        input(w, "call", cx);
        keys(w, "escape", cx);
        assert_eq!(value(&editor, cx), "call(bar);\n");
    }

    #[gpui_kit::test]
    fn block_lines_up_with_editor_caret(cx: &mut TestAppContext) {
        let (w, editor) = open("hello\nworld wide\n", cx);
        // Insert mode leaves the editor's own caret where the block would be.
        keys(w, "j w w i", cx);
        cx.update_window(w, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        editor.read_with(cx, |e, cx| {
            let st = e.buffers[e.active].state.read(cx);
            let caret = st.prepaint_caret_geometry().unwrap().cursor_bounds();
            let ours = st.range_to_bounds(&(st.cursor()..st.cursor())).unwrap();
            eprintln!("cursor={} caret={caret:?} ours={ours:?}", st.cursor());
            assert!((caret.origin.x - ours.origin.x).abs() < gpui_kit::px(1.), "{caret:?} vs {ours:?}");
            assert!(caret.origin.y >= ours.origin.y && caret.bottom() <= ours.bottom(), "{caret:?} vs {ours:?}");
        });
    }

    #[gpui_kit::test]
    fn block_is_painted_inside_the_editor(cx: &mut TestAppContext) {
        let (w, _) = open("hello\nworld\n", cx);
        keys(w, "j l", cx);
        cx.update_window(w, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        let (clip, block) = super::LAST_BLOCK.get().expect("a block was painted");
        assert!(clip.contains(&block.origin) && block.size.width > gpui_kit::px(0.), "{block:?} outside {clip:?}");
    }

    fn dir_of(editor: &Entity<TextEditor>, cx: &mut TestAppContext) -> std::path::PathBuf {
        editor.read_with(cx, |e, _| e.root.clone())
    }

    #[gpui_kit::test]
    fn command_line_opens_writes_and_closes(cx: &mut TestAppContext) {
        let (w, editor) = open("one\ntwo\nthree\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("other.txt"), "other\n").unwrap();

        // Tab completes the path; enter opens it in a new tab.
        keys(w, ":", cx);
        input(w, "e oth", cx);
        keys(w, "tab enter", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("other.txt")));

        // Edit, refuse to close the tab with changes, then write and close it.
        keys(w, "d :", cx);
        input(w, "bc", cx);
        keys(w, "enter", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.buffers.len()), 2);
        assert!(editor.read_with(cx, |e, _| e.status.clone()).is_some_and(|s| s.contains("unsaved")));
        keys(w, ":", cx);
        input(w, "w", cx);
        keys(w, "enter :", cx);
        input(w, "bc", cx);
        keys(w, "enter", cx);
        assert_eq!(std::fs::read_to_string(dir.join("other.txt")).unwrap(), "ther\n");
        assert_eq!(editor.read_with(cx, |e, _| e.buffers.len()), 1);

        // `:3` jumps to the third line; escape leaves the prompt untouched.
        keys(w, ":", cx);
        input(w, "3", cx);
        keys(w, "enter d", cx);
        assert_eq!(value(&editor, cx), "one\ntwo\nhree\n");
        keys(w, ":", cx);
        input(w, "bc", cx);
        keys(w, "escape", cx);
        assert!(editor.read_with(cx, |e, _| e.command.is_none()));
        assert_eq!(editor.read_with(cx, |e, _| e.buffers.len()), 1);
    }

    #[gpui_kit::test]
    fn rm_trashes_the_current_file(cx: &mut TestAppContext) {
        let (w, editor) = open("keep\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("junk.txt"), "junk\n").unwrap();
        keys(w, ":", cx);
        input(w, "e junk.txt", cx);
        keys(w, "enter d :", cx);
        input(w, "rm", cx);
        keys(w, "enter", cx);
        assert!(dir.join("junk.txt").exists(), "unsaved changes refuse :rm");
        keys(w, ":", cx);
        input(w, "rm!", cx);
        keys(w, "enter", cx);
        assert!(!dir.join("junk.txt").exists());
        assert_eq!(editor.read_with(cx, |e, _| e.buffers.len()), 1);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("doc.txt")));
    }

    fn fixed(editor: &Entity<TextEditor>, reply: &str, cx: &mut TestAppContext) {
        let reply = reply.to_string();
        editor.update(cx, |e, _| e.ai = Some(std::sync::Arc::new(crate::assist::Backend::Fixed(reply))));
    }

    fn rounds(editor: &Entity<TextEditor>, replies: &[&str], cx: &mut TestAppContext) {
        let replies = replies.iter().map(|r| r.to_string()).collect();
        editor.update(cx, |e, _| e.ai = Some(std::sync::Arc::new(crate::assist::Backend::Rounds(replies))));
    }

    /// Let the (fixed) reply land before reviewing.
    fn wait_for_steps(editor: &Entity<TextEditor>, cx: &mut TestAppContext) {
        cx.run_until_parked();
        assert!(editor.read_with(cx, |e, _| e.assist.as_ref().is_none_or(|a| a.writing.is_none())), "Claude's reply never arrived");
    }

    fn reviewing(editor: &Entity<TextEditor>, cx: &mut TestAppContext) -> bool {
        editor.read_with(cx, |e, _| e.assist.is_some())
    }

    fn current(editor: &Entity<TextEditor>, cx: &mut TestAppContext) -> Option<(usize, usize)> {
        editor.read_with(cx, |e, _| e.assist.as_ref().and_then(|a| a.current))
    }

    fn status(editor: &Entity<TextEditor>, cx: &mut TestAppContext) -> Option<String> {
        editor.read_with(cx, |e, _| e.status.as_ref().map(|s| s.to_string()))
    }

    fn ask(w: AnyWindowHandle, editor: &Entity<TextEditor>, what: &str, cx: &mut TestAppContext) {
        keys(w, "space i", cx);
        input(w, what, cx);
        keys(w, "enter", cx);
        wait_for_steps(editor, cx);
    }

    const TWO_STEPS: &str = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n<step>\n<old>\ndef add(a, b):\n</old>\n<new>\ndef add(a: int, b: int) -> int:\n</new>\n</step>\n";
    const ADD: &str = "def add(a, b):\n    pass\n";

    #[gpui_kit::test]
    fn steps_are_reviewed_and_the_last_tab_finishes(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        fixed(&editor, TWO_STEPS, cx);
        keys(w, "j space i", cx);
        input(w, "implement it", cx);
        // Typing in the prompt doesn't drive normal mode.
        assert_eq!(value(&editor, cx), ADD);
        keys(w, "enter", cx);
        wait_for_steps(&editor, cx);
        assert_eq!(value(&editor, cx), ADD, "nothing changes before accepting");
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return a + b\n");
        assert_eq!(current(&editor, cx), Some((0, 1)));
        // Skipping the last step finishes: no Esc needed.
        keys(w, "backspace", cx);
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 1 of 2 steps"));
        // Each accepted step is its own undo.
        keys(w, "u", cx);
        assert_eq!(value(&editor, cx), ADD);
    }

    #[gpui_kit::test]
    fn a_story_only_goes_forward(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        fixed(&editor, TWO_STEPS, cx);
        ask(w, &editor, "types and body", cx);
        keys(w, "backspace", cx);
        assert_eq!(current(&editor, cx), Some((0, 1)));
        keys(w, "shift-tab", cx);
        assert_eq!(current(&editor, cx), Some((0, 1)), "no going back to the skipped step");
        assert_eq!(value(&editor, cx), ADD, "and Shift+Tab doesn't touch the text");
        keys(w, "tab", cx);
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 1 of 2 steps"));
    }

    #[gpui_kit::test]
    fn escape_stops_early_and_keeps_what_was_accepted(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        fixed(&editor, TWO_STEPS, cx);
        ask(w, &editor, "types and body", cx);
        keys(w, "tab escape", cx);
        assert!(!reviewing(&editor, cx));
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return a + b\n");
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 1 of 2 steps"));
    }

    #[gpui_kit::test]
    fn review_centers_on_the_current_step(cx: &mut TestAppContext) {
        let mut text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        text.push_str("    pass\n");
        let (w, editor) = open(&text, cx);
        fixed(&editor, "<step>\n<old>\n    pass\n</old>\n<new>\n    done()\n</new>\n</step>\n", cx);
        ask(w, &editor, "finish", cx);
        cx.update_window(w, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        editor.read_with(cx, |e, cx| {
            let st = e.buffers[e.active].state.read(cx);
            let view = st.input_bounds();
            let line = st.range_to_bounds(&(text.len() - 9..text.len() - 9)).expect("the step's line is on screen");
            let middle = view.origin.y + view.size.height / 2.;
            assert!((line.origin.y - middle).abs() < view.size.height / 4., "{line:?} not near the middle of {view:?}");
        });
    }

    #[gpui_kit::test]
    fn a_plain_answer_is_read_through_and_finishes(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        fixed(&editor, "<answer>\nIt adds two numbers.\n\nIt doesn't check types.\n</answer>\n", cx);
        ask(w, &editor, "what does this do", cx);
        assert_eq!(current(&editor, cx), Some((0, 0)));
        keys(w, "tab", cx);
        assert_eq!(current(&editor, cx), Some((0, 1)), "each paragraph is a note");
        keys(w, "tab", cx);
        assert!(!reviewing(&editor, cx), "the last Tab finishes");
        assert_eq!(value(&editor, cx), ADD, "answers never edit");
    }

    #[gpui_kit::test]
    fn answer_notes_jump_through_the_code(cx: &mut TestAppContext) {
        let mut text = String::from("def first():\n    return 1\n");
        text.extend((0..120).map(|i| format!("# filler {i}\n")));
        text.push_str("def second():\n    return first() + 1\n");
        let (w, editor) = open(&text, cx);
        fixed(&editor, "<answer>\n<note>\n<at>\ndef first():\n</at>\nfirst returns one.\n</note>\n<note>\n<at>\n    return first() + 1\n</at>\nsecond builds on it.\n</note>\n</answer>\n", cx);
        let cursor = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).selected_range().start);
        ask(w, &editor, "how do these relate", cx);
        assert_eq!(cursor(cx), 0, "the first note is about `def first`");
        keys(w, "tab", cx);
        assert_eq!(cursor(cx), text.find("    return first() + 1").unwrap(), "Tab jumps to the second note's line");
        assert_eq!(value(&editor, cx), text);
    }

    #[gpui_kit::test]
    fn a_correction_replaces_the_plan(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        let first = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n";
        let second = "<step>\n<old>\n    pass\n</old>\n<new>\n    return sum((a, b))\n</new>\n</step>\n";
        rounds(&editor, &[first, second], cx);
        ask(w, &editor, "implement it", cx);
        keys(w, "enter", cx);
        input(w, "use sum()", cx);
        assert_eq!(value(&editor, cx), ADD, "typing the redirect doesn't edit");
        keys(w, "enter", cx);
        wait_for_steps(&editor, cx);
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return sum((a, b))\n");
        assert!(!reviewing(&editor, cx));
    }

    #[gpui_kit::test]
    fn a_question_mid_plan_becomes_notes_in_the_plan(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        let answer = "<answer>\n<note>\nBecause both are ints here.\n</note>\n</answer>\n";
        rounds(&editor, &[TWO_STEPS, answer], cx);
        ask(w, &editor, "implement it", cx);
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return a + b\n");
        // Ask about the next step: the answer is inserted right here.
        keys(w, "enter", cx);
        input(w, "why int?", cx);
        keys(w, "enter", cx);
        wait_for_steps(&editor, cx);
        assert_eq!(current(&editor, cx), Some((1, 0)), "the answer's note comes next");
        // Tab reads past it and carries on with the plan's step 2.
        keys(w, "tab", cx);
        assert_eq!(current(&editor, cx), Some((0, 1)));
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a: int, b: int) -> int:\n    return a + b\n");
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 2 of 2 steps"));
    }

    #[gpui_kit::test]
    fn escape_leaves_a_redirect_but_keeps_reviewing(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        fixed(&editor, TWO_STEPS, cx);
        ask(w, &editor, "implement it", cx);
        keys(w, "enter", cx);
        input(w, "never mind", cx);
        keys(w, "escape", cx);
        assert!(editor.read_with(cx, |e, _| e.assist.as_ref().is_some_and(|a| !a.redirecting)));
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return a + b\n");
    }

    fn run_sh(w: AnyWindowHandle, editor: &Entity<TextEditor>, command: &str, cx: &mut TestAppContext) {
        keys(w, ":", cx);
        input(w, &format!("sh {command}"), cx);
        keys(w, "enter", cx);
        for _ in 0..200 {
            cx.run_until_parked();
            if editor.read_with(cx, |e, _| e.shell_run.as_ref().is_none_or(|r| !r.stops_pending())) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("`{command}` never finished");
    }

    #[gpui_kit::test]
    fn sh_shows_output_and_the_last_tab_finishes(cx: &mut TestAppContext) {
        let (w, editor) = open("x\n", cx);
        run_sh(w, &editor, "echo hello; echo world", cx);
        assert!(editor.read_with(cx, |e, _| e.shell_run.is_some()));
        keys(w, "tab", cx);
        assert!(editor.read_with(cx, |e, _| e.shell_run.is_none()));
        assert_eq!(status(&editor, cx).as_deref(), Some("`echo hello; echo world`: ✓ exit 0"));
        assert_eq!(value(&editor, cx), "x\n");
    }

    #[gpui_kit::test]
    fn sh_errors_are_stops_across_files(cx: &mut TestAppContext) {
        let (w, editor) = open("x\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("a.py"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("b.py"), "uno\n").unwrap();
        run_sh(w, &editor, "printf 'error: bad\\n  --> a.py:2:1\\n\\nerror: worse\\n --> b.py:1\\n'; exit 1", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("a.py")), "the first error opens its file");
        let line = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).cursor_position().line);
        assert_eq!(line(cx), 1);
        keys(w, "tab", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("b.py")));
        keys(w, "shift-tab", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("a.py")));
        keys(w, "tab tab", cx);
        assert!(editor.read_with(cx, |e, _| e.shell_run.is_none()));
        assert_eq!(status(&editor, cx).as_deref(), Some("`printf 'error: bad\\n  --> a.py:2:1\\n\\nerror: worse\\n --> b.py:1\\n'; exit 1`: ✗ exit 1, 2 locations"));
    }

    #[gpui_kit::test]
    fn space_slash_tours_the_projects_matches(cx: &mut TestAppContext) {
        let (w, editor) = open("let total = 1;\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("b.txt"), "nothing\nprint(total)\n").unwrap();
        let selected = |cx: &mut TestAppContext| {
            editor.read_with(cx, |e, cx| {
                let st = e.buffers[e.active].state.read(cx);
                (e.active_path().unwrap().file_name().unwrap().to_string_lossy().to_string(), st.text().to_string()[st.selected_range()].to_string())
            })
        };
        keys(w, "space /", cx);
        input(w, "TOTAL", cx);
        keys(w, "enter", cx);
        assert_eq!(status(&editor, cx).as_deref(), Some("no matches for TOTAL"), "a capital makes it exact");
        keys(w, "space /", cx);
        input(w, "total", cx);
        keys(w, "enter", cx);
        assert_eq!(selected(cx), ("b.txt".into(), "total".into()), "files in order; the match is selected");
        keys(w, "tab", cx);
        assert_eq!(selected(cx), ("doc.txt".into(), "total".into()));
        keys(w, "shift-tab", cx);
        assert_eq!(selected(cx).0, "b.txt");
        // Enter asks Claude right there.
        keys(w, "enter", cx);
        assert!(editor.read_with(cx, |e, _| e.assist.as_ref().is_some_and(|a| !a.asked)));
        keys(w, "escape", cx);
        assert_eq!(selected(cx).0, "doc.txt", "and the tour goes on");
        keys(w, "tab", cx);
        assert!(editor.read_with(cx, |e, _| e.shell_run.is_none()), "the last Tab finishes");
    }

    #[gpui_kit::test]
    fn enter_hands_an_error_to_claude_then_moves_on(cx: &mut TestAppContext) {
        let (w, editor) = open("x\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("a.py"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("b.py"), "uno\n").unwrap();
        fixed(&editor, "<step>\n<old>\ntwo\n</old>\n<new>\n2\n</new>\n</step>\n", cx);
        run_sh(w, &editor, "printf 'error: bad\\n  --> a.py:2\\n\\nerror: worse\\n --> b.py:1\\n'", cx);
        keys(w, "enter", cx);
        assert_eq!(editor.read_with(cx, |e, cx| e.assist.as_ref().map(|a| a.input.read(cx).value().to_string())), Some("fix this".into()));
        assert!(editor.read_with(cx, |e, _| e.assist.as_ref().is_some_and(|a| a.context.as_deref().is_some_and(|c| c.contains("error: bad")))));
        keys(w, "enter", cx);
        wait_for_steps(&editor, cx);
        keys(w, "tab", cx);
        assert_eq!(std::fs::read_to_string(dir.join("a.py")).unwrap(), "one\n2\n", "moving on to b.py writes a.py");
        let a = dir.join("a.py");
        assert_eq!(editor.read_with(cx, |e, cx| e.buffers.iter().find(|b| b.path.as_ref() == Some(&a)).map(|b| b.state.read(cx).value().to_string())), Some("one\n2\n".into()));
        // Claude is done: on to the next error, in b.py.
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("b.py")));
        assert!(editor.read_with(cx, |e, _| e.shell_run.is_some()));
    }

    #[gpui_kit::test]
    fn an_edit_asked_during_a_tour_is_inserted_and_the_tour_goes_on(cx: &mut TestAppContext) {
        let (w, editor) = open(ADD, cx);
        let tour = "<answer>\n<note>\n<at>\n    pass\n</at>\nThis does nothing yet.\n</note>\n<note>\n<at>\ndef add(a, b):\n</at>\nNo type hints.\n</note>\n</answer>\n";
        let fix = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n";
        rounds(&editor, &[tour, fix], cx);
        ask(w, &editor, "walk me through this", cx);
        assert_eq!(current(&editor, cx), Some((0, 0)));
        // At the first note: "then fix it" comes back as a step, right here.
        keys(w, "enter", cx);
        input(w, "implement it", cx);
        keys(w, "enter", cx);
        wait_for_steps(&editor, cx);
        assert_eq!(current(&editor, cx), Some((1, 0)), "the edit is inserted after the note");
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "def add(a, b):\n    return a + b\n");
        // ...and the tour carries on with the second note.
        assert_eq!(current(&editor, cx), Some((0, 1)));
        keys(w, "tab", cx);
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 1 of 1 steps"));
    }

    #[gpui_kit::test]
    fn match_mode_selects_and_surrounds(cx: &mut TestAppContext) {
        let (w, editor) = open("call(a, b)\n", cx);
        // mi( selects inside the parens; d deletes it.
        keys(w, "l l l l l m i ( d", cx);
        assert_eq!(value(&editor, cx), "call()\n");
        keys(w, "u", cx);
        // ms' wraps the word under the cursor; md' takes the quotes off again.
        keys(w, "g h m i w m s '", cx);
        assert_eq!(value(&editor, cx), "'call'(a, b)\n");
        keys(w, "g h l m d '", cx);
        assert_eq!(value(&editor, cx), "call(a, b)\n");
        // mr([ swaps the parens for brackets.
        keys(w, "g l h m r ( [", cx);
        assert_eq!(value(&editor, cx), "call[a, b]\n");
        // mm jumps between the brackets.
        keys(w, "g l m m", cx);
        assert_eq!(editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).selected_range().start), 4);
    }

    #[gpui_kit::test]
    fn up_and_down_walk_the_command_history(cx: &mut TestAppContext) {
        let (w, editor) = open("x\n", cx);
        editor.update(cx, |e, _| e.history.clear());
        for command in ["n", "bp"] {
            keys(w, ":", cx);
            input(w, command, cx);
            keys(w, "enter", cx);
        }
        let typed = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.command.as_ref().map(|(i, _)| i.read(cx).value().to_string()));
        keys(w, ":", cx);
        keys(w, "up", cx);
        assert_eq!(typed(cx).as_deref(), Some("bp"));
        keys(w, "up", cx);
        assert_eq!(typed(cx).as_deref(), Some("n"));
        keys(w, "down", cx);
        assert_eq!(typed(cx).as_deref(), Some("bp"));
        keys(w, "down", cx);
        assert_eq!(typed(cx).as_deref(), Some(""));
    }

    #[gpui_kit::test]
    fn question_mark_shows_the_cheatsheet_and_any_key_closes_it(cx: &mut TestAppContext) {
        let (w, editor) = open("one\ntwo\n", cx);
        keys(w, "?", cx);
        assert!(editor.read_with(cx, |e, _| e.cheatsheet));
        keys(w, "j", cx);
        assert!(editor.read_with(cx, |e, _| !e.cheatsheet));
        assert_eq!(editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).cursor_position().line), 0, "the closing key does nothing else");
    }

    #[gpui_kit::test]
    fn a_story_can_create_a_file_and_write_into_it(cx: &mut TestAppContext) {
        let (w, editor) = open("fn main() {}\n", cx);
        let dir = dir_of(&editor, cx);
        let story = "<create>\nsrc/util.rs\n</create>\n<step>\n<file>\nsrc/util.rs\n</file>\n<old>\n</old>\n<new>\npub fn one() -> u32 { 1 }\n</new>\n</step>\n<step>\n<old>\nfn main() {}\n</old>\n<new>\nmod util;\nfn main() {}\n</new>\n</step>\n";
        fixed(&editor, story, cx);
        ask(w, &editor, "move a helper into util", cx);
        assert!(!dir.join("src/util.rs").exists(), "nothing is created before Tab");
        keys(w, "tab", cx);
        assert!(dir.join("src/util.rs").is_file(), "Tab creates the file, folders included");
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("src/util.rs")), "and opens it");
        keys(w, "tab", cx);
        let util = dir.join("src/util.rs");
        let util_text = editor.read_with(cx, |e, cx| e.buffers.iter().find(|b| b.path.as_ref() == Some(&util)).map(|b| b.state.read(cx).value().to_string()));
        assert_eq!(util_text.as_deref(), Some("pub fn one() -> u32 { 1 }\n"), "the next step writes into it");
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("doc.txt")), "then back to the first file for its step");
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "mod util;\nfn main() {}\n");
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 3 of 3 steps"));
    }

    /// Live, with rust-analyzer: diagnostics arrive, `gd` jumps, completions come.
    /// `cargo test rust_analyzer_end_to_end -- --ignored`
    #[gpui_kit::test]
    #[ignore]
    fn rust_analyzer_end_to_end(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-ra-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        let main = "fn helper() -> u32 { 1 }\nfn main() {\n    let x: u32 = \"no\";\n    helper();\n}\n";
        std::fs::write(root.join("src/main.rs"), main).unwrap();
        // A real server answers from its own thread.
        cx.executor().allow_parking();
        let editor_window = cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut e = TextEditor::new(vec![], root.clone(), window, cx);
                    e.servers.enabled = true;
                    e.open_paths(vec![root.join("src/main.rs")], window, cx);
                    e
                })
            })
            .unwrap()
        });
        let (w, editor) = editor_window;
        // rust-analyzer needs a while to index; wait for the type error.
        let mut found = false;
        for _ in 0..600 {
            cx.run_until_parked();
            let n = editor.read_with(cx, |e, cx| e.buffers[0].state.read(cx).diagnostics().map_or(0, |d| d.len()));
            if n > 0 {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(found, "rust-analyzer reported no diagnostics");
        // gd on `helper()` in main jumps to its definition on line 1.
        keys(w, "j j j g s", cx);
        keys(w, "g d", cx);
        let mut jumped = false;
        for _ in 0..100 {
            cx.run_until_parked();
            if editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).cursor_position().line) == 0 {
                jumped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(jumped, "gd didn't reach `fn helper`");
        std::fs::remove_dir_all(root).ok();
    }

    /// Live, with basedpyright: only real errors, not strict-mode noise.
    /// `cargo test basedpyright_is_not_noisy -- --ignored`
    #[gpui_kit::test]
    #[ignore]
    fn basedpyright_is_not_noisy(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-pyright-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.py"), "import json\n\n\ndef f(a):\n    return json.dumps(a)\n\n\nprint(f(1), undefined_name)\n").unwrap();
        cx.executor().allow_parking();
        let (_, editor) = cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut e = TextEditor::new(vec![], root.clone(), window, cx);
                    e.servers.enabled = true;
                    e.open_paths(vec![root.join("main.py")], window, cx);
                    e
                })
            })
            .unwrap()
        });
        let count = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.buffers[0].state.read(cx).diagnostics().map_or(0, |d| d.len()));
        for _ in 0..300 {
            cx.run_until_parked();
            if count(cx) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Let any later publish land too.
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let messages = editor.read_with(cx, |e, cx| {
            e.buffers[0].state.read(cx).diagnostics().map(|d| d.iter().map(|x| x.diagnostic.message.to_string()).collect::<Vec<_>>()).unwrap_or_default()
        });
        println!("{messages:?}");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("undefined_name"));
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn a_story_can_remove_a_file(cx: &mut TestAppContext) {
        let (w, editor) = open("x\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("old.txt"), "stale\n").unwrap();
        fixed(&editor, "<remove>\nold.txt\n</remove>\n", cx);
        ask(w, &editor, "drop the old file", cx);
        assert!(dir.join("old.txt").exists(), "nothing is removed before Tab");
        keys(w, "tab", cx);
        assert!(!dir.join("old.txt").exists());
        assert!(!reviewing(&editor, cx));
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 1 of 1 steps"));
    }


    #[gpui_kit::test]
    fn docs_notes_leave_the_cursor_alone(cx: &mut TestAppContext) {
        let (w, editor) = open("def f():\n    return compute(1, 2)\n", cx);
        keys(w, "j w w", cx);
        let at = |cx: &mut TestAppContext| editor.read_with(cx, |e, cx| e.buffers[e.active].state.read(cx).selected_range());
        let before = at(cx);
        cx.update_window(w, |_, window, cx| editor.update(cx, |e, cx| e.show_note("compute(a, b): adds", window, cx))).unwrap();
        cx.run_until_parked();
        assert!(reviewing(&editor, cx), "the docs show");
        assert_eq!(at(cx), before, "and the cursor stays put");
        keys(w, "tab", cx);
        assert!(!reviewing(&editor, cx));
        assert_eq!(at(cx), before);
    }

    #[gpui_kit::test]
    fn an_unnamed_step_is_found_in_its_project_file(cx: &mut TestAppContext) {
        let (w, editor) = open("y = 0\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("other.py"), "x = 1\nprint(x)\n").unwrap();
        fixed(&editor, "<step>\n<old>\nx = 1\n</old>\n<new>\nx = 2\n</new>\n</step>\n", cx);
        ask(w, &editor, "bump x", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("other.py")), "it goes to the file with those lines");
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "x = 2\nprint(x)\n");
        assert!(!reviewing(&editor, cx));
    }

    #[gpui_kit::test]
    fn leaving_a_file_writes_it(cx: &mut TestAppContext) {
        let (w, editor) = open("one\n", cx);
        let dir = dir_of(&editor, cx);
        std::fs::write(dir.join("second.txt"), "two\n").unwrap();
        keys(w, "d", cx);
        assert_eq!(std::fs::read_to_string(dir.join("doc.txt")).unwrap(), "one\n", "not saved while you're in it");
        keys(w, ":", cx);
        input(w, "e second.txt", cx);
        keys(w, "enter", cx);
        assert_eq!(std::fs::read_to_string(dir.join("doc.txt")).unwrap(), "ne\n", "written when you leave");
        assert!(editor.read_with(cx, |e, _| e.buffers.iter().all(|b| !b.dirty)));
    }

    #[gpui_kit::test]
    fn a_story_can_rename_a_file_and_keep_editing_it(cx: &mut TestAppContext) {
        let (w, editor) = open("one\n", cx);
        let dir = dir_of(&editor, cx);
        let story = "<rename>\ndoc.txt\nnotes/renamed.txt\n</rename>\n<step>\n<file>\nnotes/renamed.txt\n</file>\n<old>\none\n</old>\n<new>\nuno\n</new>\n</step>\n";
        fixed(&editor, story, cx);
        ask(w, &editor, "move it into notes", cx);
        assert!(dir.join("doc.txt").exists(), "nothing moves before Tab");
        keys(w, "tab", cx);
        assert!(!dir.join("doc.txt").exists() && dir.join("notes/renamed.txt").is_file(), "renamed, folder made");
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(dir.join("notes/renamed.txt")), "the tab followed");
        keys(w, "tab", cx);
        assert_eq!(value(&editor, cx), "uno\n");
        assert_eq!(status(&editor, cx).as_deref(), Some("applied 2 of 2 steps"));
    }
}
