//! The project being edited, as a model: its folder, the open files, the
//! file tree, its language servers, and what the status line says. The
//! features (stories, completion) work on this; the editor around it
//! adds keys, focus, prompts and drawing, and decides when to switch tabs.

use std::path::{Path, PathBuf};

use ghostwrite_core::Range;
use ghostwrite_project::store::Session;
use ghostwrite_project::tree::{Listing, Row, Tree};
use gpui::SharedString;

use super::{Autoscroll, Document, Tab, Tabs, document};

pub struct Project {
    pub root: PathBuf,
    pub tabs: Tabs,
    pub tree: Tree,
    /// What the status line says, until something else does.
    pub status: Option<SharedString>,
    servers: ghostwrite_lsp::Registry,
    /// Show Typst files compiled, beside them.
    pub previews: bool,
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

/// What to read from disk again, away from the UI.
pub struct Reread {
    listing: Option<Listing>,
    files: Vec<PathBuf>,
}

/// What [`Reread::read`] found: the tree's rows, and each open file's text
/// (files gone or unreadable left out: what's on screen is all there is).
pub struct Disk {
    rows: Option<(Listing, Vec<Row>)>,
    texts: Vec<(PathBuf, String)>,
    /// PDFs, read again.
    pdfs: Vec<(PathBuf, ghostwrite_pdf::Pdf)>,
}

impl Reread {
    /// Read it. Blocks: run it in the background.
    pub fn read(self) -> Disk {
        let rows = self.listing.map(|listing| {
            let rows = listing.list();
            (listing, rows)
        });
        let pdf = |p: &PathBuf| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        let (pdfs, files): (Vec<PathBuf>, Vec<PathBuf>) = self.files.into_iter().partition(pdf);
        let texts = files.into_iter().filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text))).collect();
        let pdfs = pdfs.into_iter().filter_map(|path| ghostwrite_pdf::Pdf::open(&path).ok().map(|pdf| (path, pdf))).collect();
        Disk { rows, texts, pdfs }
    }
}

impl Disk {
    /// The new text of `path`, if it was read.
    pub fn text(&self, path: &Path) -> Option<&str> {
        self.texts.iter().find(|(p, _)| p == path).map(|(_, t)| t.as_str())
    }
}

impl Project {
    pub fn new(root: PathBuf, servers: ghostwrite_lsp::Registry, previews: bool) -> Self {
        Self { tree: Tree::new(root.clone()), root, tabs: Tabs::default(), status: None, servers, previews }
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
        let lsp = path.as_deref().and_then(|p| document::server_for(&mut self.servers, p, &text));
        self.tabs.add(|id| Document::new(id, path, text, lsp))
    }

    /// A new tab for the PDF read from `path` (not switched to); returns its index.
    pub fn add_pdf(&mut self, path: PathBuf, pdf: ghostwrite_pdf::Pdf) -> usize {
        self.tabs.add(|id| Document::pdf(id, path, pdf))
    }

    /// The tab for `path`, opening the file if it isn't open (a missing file
    /// opens empty). `None` if it can't be read; the status says why.
    pub fn open(&mut self, path: &Path) -> Option<usize> {
        let path = ghostwrite_project::clean(path);
        if let Some(ix) = self.tabs.index_of_path(&path) {
            return Some(ix);
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            return match ghostwrite_pdf::Pdf::open(&path) {
                Ok(pdf) => Some(self.tabs.add(|id| Document::pdf(id, path, pdf))),
                Err(e) => {
                    self.say(format!("couldn't open {}: {e}", path.display()));
                    None
                }
            };
        }
        let max = crate::config::get().max_file_size;
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > max
        {
            self.say(format!("{} is too large to open ({} MB, limit {} MB)", path.display(), meta.len() >> 20, max >> 20));
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
    /// and the language servers read) matches what you see; never over
    /// changes another program made (the status says so instead).
    pub fn activate(&mut self, ix: usize) {
        let leaving = self.tabs.active_index();
        if ix != leaving
            && let Some(path) = self.tabs.get(leaving).filter(|t| t.doc.dirty).and_then(|t| t.doc.path.clone())
        {
            self.write(leaving, path, false);
        }
        self.tabs.set_active(ix);
        self.tree.set_active(self.active_path());
        // Keep a scroll already asked for (a story's item centered in its file).
        if let Some(tab) = self.tab_mut() {
            tab.view.autoscroll.get_or_insert(Autoscroll::Fit);
        }
    }

    /// Write tab `ix` to `path`, saying so; `force` writes over changes on
    /// disk, or another file (see [`Document::save`]). Whether it did.
    pub fn write(&mut self, ix: usize, path: PathBuf, force: bool) -> bool {
        let Some(tab) = self.tabs.get_mut(ix) else { return false };
        let saved = tab.doc.save(&path, force);
        let message = match &saved {
            Ok(()) => {
                let message = format!("saved {}", ghostwrite_project::display(&path, &self.root));
                if tab.doc.path.as_ref() != Some(&path) {
                    tab.doc.moved(path, &mut self.servers);
                }
                message
            }
            Err(e) => format!("couldn't save: {e}"),
        };
        self.say(message);
        self.tree.refresh();
        saved.is_ok()
    }

    /// Write every open file with unsaved changes (none over changes on disk).
    pub fn write_all(&mut self) {
        for ix in 0..self.tabs.len() {
            if let Some(path) = self.tabs.get(ix).filter(|t| t.doc.dirty).and_then(|t| t.doc.path.clone()) {
                self.write(ix, path, false);
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
            tab.doc.moved(moved, &mut self.servers);
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

    /// What to read again after another program changed `paths`: the tree's
    /// folders if it shows any of them, and the open files among them.
    pub fn to_reread(&self, paths: &[PathBuf]) -> Reread {
        let listing = self.tree.shows_any(paths).then(|| self.tree.listing());
        let files = self.tabs.iter().filter_map(|t| t.doc.path.clone()).filter(|p| paths.contains(p)).collect();
        Reread { listing, files }
    }

    /// Show what was read again: the tree's rows, and the open files' new
    /// text. A file with unsaved changes keeps them; the status says so.
    /// Whether anything on screen changed.
    pub fn reread(&mut self, disk: Disk) -> bool {
        let mut changed = false;
        if let Some((listing, rows)) = disk.rows {
            self.tree.listed(&listing, rows);
            changed = true;
        }
        // A PDF that's changed (rebuilt, say): its new text and pages.
        for (path, pdf) in disk.pdfs {
            if let Some(tab) = self.tabs.index_of_path(&path).and_then(|ix| self.tabs.get_mut(ix)) {
                tab.doc.pdf = Some(std::sync::Arc::new(pdf));
                tab.reload(tab.doc.pdf.as_ref().map(|p| p.text.clone()).unwrap_or_default());
                changed = true;
            }
        }
        let mut kept = vec![];
        for (path, text) in disk.texts {
            let Some(tab) = self.tabs.index_of_path(&path).and_then(|ix| self.tabs.get_mut(ix)) else { continue };
            if !tab.doc.changed_on_disk() {
                // Our own write (typing may have gone on since), or read before it landed.
                continue;
            }
            if ghostwrite_project::fs::normalize(text.clone()).0 == tab.doc.text() {
                // Our own write, or someone wrote what's on screen.
                changed |= tab.doc.dirty;
                tab.doc.on_disk();
            } else if tab.doc.dirty {
                // Saving would lose theirs: it won't, until told to (:w!).
                if !std::mem::replace(&mut tab.doc.conflict, true) {
                    kept.push(tab.doc.name());
                }
            } else {
                tab.reload(text);
                changed = true;
            }
        }
        if !kept.is_empty() {
            self.say(format!(
                "{} changed on disk; your unsaved changes are kept, and won't be saved over it (:w! does, :rl! takes the file's)",
                kept.join(", ")
            ));
            changed = true;
        }
        changed
    }

    /// Diagnostics from a server, onto the file they're for; not if they're
    /// for an older version of its text (newer ones are coming, and these
    /// would land in the wrong places).
    pub fn apply_diagnostics(&mut self, path: &Path, version: Option<i32>, diagnostics: Vec<ghostwrite_lsp::RawDiagnostic>) -> bool {
        let Some(tab) = self.tabs.index_of_path(path).and_then(|ix| self.tabs.get_mut(ix)) else { return false };
        if version.zip(tab.doc.lsp.as_ref()).is_some_and(|(v, lsp)| v < lsp.version()) {
            return false;
        }
        tab.doc.set_diagnostics(diagnostics);
        true
    }

    /// A language server stopped: say why, and start it again for its files
    /// (the registry gives up on one that keeps stopping).
    pub fn server_exited(&mut self, server: &str, why: &str) {
        let mut restarted = false;
        for tab in self.tabs.iter_mut().filter(|t| t.doc.lsp.as_ref().is_some_and(|l| l.server() == server && !l.alive())) {
            let path = tab.doc.path.clone();
            tab.doc.restart_server(path.as_deref(), &mut self.servers);
            restarted |= tab.doc.lsp.is_some();
        }
        let why = if why.is_empty() { String::new() } else { format!(": {}", why.trim()) };
        let next = if restarted { "started it again" } else { "it keeps stopping, so it's off for now" };
        self.say(format!("{server} stopped{why} ({next})"));
    }

    /// How far Lean has checked `path`: the goals pane says, and asks again once it's done.
    pub fn apply_progress(&mut self, path: &Path, checking: Option<u32>) -> bool {
        let Some(tab) = self.tabs.index_of_path(path).and_then(|ix| self.tabs.get_mut(ix)) else { return false };
        let Some(goals) = tab.doc.goals.as_mut() else { return false };
        goals.checked(checking);
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
        let active = session.files.get(session.active).filter(|(p, ..)| p.is_file()).or(existing.last().copied());
        let active = active.and_then(|(p, ..)| self.tabs.index_of_path(p));
        if let Some(ix) = active {
            self.activate(ix);
        }
    }
}
