//! The project being edited, as a model: its folder, the open files, the
//! file tree, its language servers, and what the status line says. The
//! features (stories, tours, completion) work on this; the editor around it
//! adds keys, focus, prompts and drawing, and decides when to switch tabs.

use std::path::{Path, PathBuf};

use ghostwrite_core::Range;
use ghostwrite_project::store::Session;
use ghostwrite_project::tree::Tree;
use gpui::SharedString;

use super::{Autoscroll, Document, Tab, Tabs};

/// Files bigger than this aren't opened, so a stray log or binary can't hang the editor.
const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;

pub struct Project {
    pub root: PathBuf,
    pub tabs: Tabs,
    pub tree: Tree,
    /// What the status line says, until something else does.
    pub status: Option<SharedString>,
    servers: ghostwrite_lsp::Registry,
}

/// The project's files as Claude's steps see them: open ones as they are on screen.
pub struct ProjectFiles<'a> {
    root: &'a Path,
    tabs: &'a Tabs,
}

impl ghostwrite_ai::placement::Files for ProjectFiles<'_> {
    fn text(&self, path: &Path) -> Option<String> {
        self.tabs.text_of(path).map(str::to_string).or_else(|| std::fs::read_to_string(path).ok())
    }

    fn all(&self) -> Vec<PathBuf> {
        ghostwrite_project::files::files(self.root, self.root).into_iter().map(|rel| self.root.join(rel)).collect()
    }
}

impl Project {
    pub fn new(root: PathBuf, servers: ghostwrite_lsp::Registry) -> Self {
        Self { tree: Tree::new(root.clone()), root, tabs: Tabs::default(), status: None, servers }
    }

    pub fn say(&mut self, message: impl Into<SharedString>) {
        self.status = Some(message.into());
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.active()
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.active_mut()
    }

    pub fn active_path(&self) -> Option<PathBuf> {
        self.tab().and_then(|t| t.doc.path.clone())
    }

    pub fn is_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.doc.dirty)
    }

    pub fn files(&self) -> ProjectFiles<'_> {
        ProjectFiles { root: &self.root, tabs: &self.tabs }
    }

    /// `path` for people: relative to the project.
    pub fn display(&self, path: &Path) -> String {
        ghostwrite_project::display(path, &self.root)
    }

    /// A new tab for `text` (not switched to); returns its index.
    pub fn add(&mut self, path: Option<PathBuf>, text: String) -> usize {
        let lsp = path.as_deref().and_then(|p| {
            let (client, language) = self.servers.for_path(p)?;
            Some(ghostwrite_lsp::Document::open(client, language, p, &text))
        });
        self.tabs.add(|id| Document::new(id, path, text, lsp))
    }

    /// The tab for `path`, opening the file if it isn't open (a missing file
    /// opens empty). `None` if it can't be read; the status says why.
    pub fn open(&mut self, path: &Path) -> Option<usize> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(ix) = self.tabs.index_of_path(&path) {
            return Some(ix);
        }
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > MAX_FILE_SIZE
        {
            self.say(format!("{} is too large to open ({} MB, limit {} MB)", path.display(), meta.len() >> 20, MAX_FILE_SIZE >> 20));
            return None;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => Some(self.add(Some(path), text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(self.add(Some(path), String::new())),
            Err(e) => {
                self.say(format!("couldn't open {}: {e}", path.display()));
                None
            }
        }
    }

    /// Switch tabs. Leaving a file writes it, so disk (what Claude, `:sh`
    /// and the language servers read) matches what you see.
    pub fn activate(&mut self, ix: usize) {
        let leaving = self.tabs.active_index();
        if ix != leaving
            && let Some(path) = self.tabs.get(leaving).filter(|t| t.doc.dirty).and_then(|t| t.doc.path.clone())
        {
            self.write(leaving, path);
        }
        self.tabs.set_active(ix);
        self.tree.set_active(self.active_path());
        if let Some(tab) = self.tab_mut() {
            tab.view.autoscroll = Some(Autoscroll::Fit);
        }
    }

    /// Write tab `ix` to `path`, saying so.
    pub fn write(&mut self, ix: usize, path: PathBuf) {
        let Some(tab) = self.tabs.get_mut(ix) else { return };
        let message = match tab.doc.save(path.clone()) {
            Ok(()) => format!("saved {}", ghostwrite_project::display(&path, &self.root)),
            Err(e) => format!("couldn't save {}: {e}", path.display()),
        };
        self.say(message);
        self.tree.refresh();
    }

    /// Write every open file with unsaved changes.
    pub fn write_all(&mut self) {
        for ix in 0..self.tabs.len() {
            if let Some(path) = self.tabs.get(ix).filter(|t| t.doc.dirty).and_then(|t| t.doc.path.clone()) {
                self.write(ix, path);
            }
        }
    }

    /// Close tab `ix` (unsaved changes are lost: ask first).
    pub fn close(&mut self, ix: usize) {
        self.tabs.remove(ix);
        self.tabs.set_active(ix);
        self.tree.set_active(self.active_path());
    }

    /// A file or folder was renamed: tabs at or below `from` follow it.
    pub fn path_moved(&mut self, from: &Path, to: &Path) {
        for tab in self.tabs.iter_mut() {
            let Some(rest) = tab.doc.path.as_deref().and_then(|p| p.strip_prefix(from).ok()) else { continue };
            let moved = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            tab.doc.moved(moved);
        }
        self.tree.renamed(from, to);
    }

    /// A file or folder went to the Trash: close its clean tabs; tabs with
    /// unsaved changes stay open so nothing is lost.
    pub fn path_deleted(&mut self, path: &Path) {
        let gone = |t: &Tab| t.doc.path.as_deref().is_some_and(|p| p.starts_with(path));
        let kept = self.tabs.iter().filter(|t| gone(t) && t.doc.dirty).count();
        self.tabs.retain(|t| !gone(t) || t.doc.dirty);
        self.tree.refresh();
        self.tree.set_active(self.active_path());
        if kept > 0 {
            self.say(format!("{kept} deleted file(s) still open with unsaved changes"));
        }
    }

    /// Diagnostics from a server, onto the file they're for.
    pub fn apply_diagnostics(&mut self, path: &Path, diagnostics: Vec<ghostwrite_lsp::RawDiagnostic>) -> bool {
        let Some(tab) = self.tabs.index_of_path(path).and_then(|ix| self.tabs.get_mut(ix)) else { return false };
        tab.doc.set_diagnostics(diagnostics);
        true
    }

    /// The open files and their cursors, to bring back next time.
    pub fn session(&self) -> Session {
        let files: Vec<(PathBuf, usize, usize)> = self
            .tabs
            .iter()
            .filter_map(|t| {
                let (line, column) = t.cursor_position();
                Some((t.doc.path.clone()?, line, column))
            })
            .collect();
        let active = self.active_path().and_then(|p| files.iter().position(|(f, ..)| *f == p)).unwrap_or(0);
        Session { files, active }
    }

    /// Reopen a session's files (skipping ones that are gone) at their cursors.
    pub fn restore(&mut self, session: &Session) {
        let existing: Vec<&(PathBuf, usize, usize)> = session.files.iter().filter(|(p, ..)| p.is_file()).collect();
        for (path, line, column) in &existing {
            let Some(tab) = self.open(path).and_then(|ix| self.tabs.get_mut(ix)) else { continue };
            let at = ghostwrite_core::offset_of(tab.doc.text(), *line, *column);
            tab.select(Range::point(at), Autoscroll::Center(*line..*line + 1));
        }
        let active = existing.get(session.active).or(existing.last()).and_then(|(p, ..)| self.tabs.index_of_path(p));
        if let Some(ix) = active {
            self.activate(ix);
        }
    }
}
