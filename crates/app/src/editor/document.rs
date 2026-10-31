//! One open file: its buffer, where it lives on disk, its language and
//! highlights, its language server, its preview (Typst) or goals (Lean). How it's shown (selection, scroll)
//! is its tab's [`View`](super::tabs::View). A PDF is its text, read-only,
//! and where that text is on its pages.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ghostwrite_core::Language;
use ghostwrite_core::indent::IndentUnit;
use ghostwrite_core::syntax::{self, Span};
use ghostwrite_core::{Buffer, Range};
use ghostwrite_lsp::Diagnostic;
use ghostwrite_project::fs::{Stamp, denormalize, normalize};
use gpui::SharedString;

pub type DocId = usize;

/// `path` opened on its language's server, if one is installed.
pub fn server_for(servers: &mut ghostwrite_lsp::Registry, path: &Path, text: &str) -> Option<ghostwrite_lsp::Document> {
    let language = Language::for_path(path)?;
    let client = servers.start(language.servers, path)?;
    Some(ghostwrite_lsp::Document::open(client, language.id(), path, text))
}

pub struct Document {
    pub id: DocId,
    pub buffer: Buffer,
    pub path: Option<PathBuf>,
    pub language: Option<&'static Language>,
    pub indent: IndentUnit,
    /// Differs from the file on disk.
    pub dirty: bool,
    /// The file as it was when last read or written: if the disk's
    /// differs, another program changed it, and saving would lose that.
    disk: Option<Stamp>,
    /// Another program changed the file while this had unsaved changes.
    pub conflict: bool,
    /// The file has `\r\n` line endings: the buffer has `\n`, and they're put back on save.
    pub crlf: bool,
    /// A Typst file's pages, compiled as it's typed.
    pub preview: Option<super::preview::Preview>,
    /// A Lean file's goals at the cursor, as its server says.
    pub goals: Option<super::goals::Goals>,
    /// Its server's colors, laid over the grammar's.
    pub semantic: super::semantic::Semantic,
    pub lsp: Option<ghostwrite_lsp::Document>,
    pub diagnostics: Vec<Diagnostic>,
    /// A PDF's pages and where its text is on them. Its text can't change.
    pub pdf: Option<Arc<ghostwrite_pdf::Pdf>>,
    /// The buffer version the language server (and `dirty`) last saw.
    synced: u64,
    /// The colors shown, for a version of the text.
    highlights: (u64, Vec<Span>),
    /// The grammar's colors as last worked out, and the text they're of.
    grammar: (u64, Arc<str>, Vec<Span>),
    /// They're being worked out.
    highlighting: bool,
}

impl Document {
    /// `text` as read from `path` (its line endings are made `\n`, and put back on save).
    pub fn new(id: DocId, path: Option<PathBuf>, text: String, lsp: Option<ghostwrite_lsp::Document>) -> Self {
        let (text, crlf) = normalize(text);
        let buffer = Buffer::new(text);
        Self {
            disk: path.as_deref().and_then(Stamp::of),
            conflict: false,
            crlf,
            id,
            indent: IndentUnit::detect(buffer.text()),
            language: path.as_deref().and_then(Language::for_path),
            synced: buffer.version(),
            buffer,
            path,
            dirty: false,
            preview: None,
            goals: None,
            semantic: Default::default(),
            lsp,
            diagnostics: vec![],
            pdf: None,
            highlights: (u64::MAX, vec![]),
            grammar: (u64::MAX, Arc::from(""), vec![]),
            highlighting: false,
        }
    }

    /// A PDF at `path`, as its text.
    pub fn pdf(id: DocId, path: PathBuf, pdf: ghostwrite_pdf::Pdf) -> Self {
        let mut doc = Self::new(id, Some(path), pdf.text.clone(), None);
        doc.pdf = Some(Arc::new(pdf));
        doc
    }

    pub fn name(&self) -> SharedString {
        self.path.as_deref().and_then(Path::file_name).map_or("untitled".into(), |n| n.to_string_lossy().into_owned().into())
    }

    /// What the language is called, for Claude and the status line.
    pub fn language_name(&self) -> String {
        match (self.language, self.path.as_deref().and_then(Path::extension)) {
            (Some(language), _) => language.name.to_string(),
            (None, Some(ext)) => ext.to_string_lossy().into_owned(),
            (None, None) => "text".into(),
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// An untitled, unedited buffer, which opening a file may take over.
    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.dirty && self.buffer.is_empty()
    }

    /// After the buffer changed: mark it dirty and tell the language server.
    pub fn sync(&mut self) {
        if self.buffer.version() == self.synced {
            return;
        }
        // Every edit comes through here; a PDF's are undone.
        if let Some(pdf) = &self.pdf {
            self.buffer.reset(pdf.text.clone());
            self.synced = self.buffer.version();
            return;
        }
        self.synced = self.buffer.version();
        self.dirty = true;
        let edits = self.buffer.take_edits();
        if let Some(lsp) = &mut self.lsp {
            lsp.changed(self.buffer.text(), edits);
        }
    }

    /// The highlights as of the last [`Document::refresh_highlights`].
    pub fn cached_highlights(&self) -> &[Span] {
        &self.highlights.1
    }

    /// The colors for the text as it is: the grammar's, and the server's
    /// over them, each as last worked out, moved past any edit since. If
    /// the grammar's are out of date, what to work them out for (off the
    /// UI thread: a big file takes a while); [`Document::highlighted`] takes them.
    pub fn refresh_highlights(&mut self) -> Option<(&'static Language, u64, Arc<str>)> {
        let version = self.buffer.version();
        let job = (self.grammar.0 != version && !self.highlighting)
            .then(|| self.language.filter(|_| self.pdf.is_none()))
            .flatten()
            .map(|language| (language, version, Arc::<str>::from(self.buffer.text())));
        self.highlighting |= job.is_some();
        if self.highlights.0 != version {
            let text = self.buffer.text();
            let moved = |spans: &[Span], from: &str| if from == text { spans.to_vec() } else { syntax::follow_edit(spans, from, text) };
            let (_, grammar_text, grammar) = &self.grammar;
            let mut spans = moved(grammar, grammar_text);
            if !self.semantic.spans.is_empty() {
                spans = syntax::overlay(&spans, &moved(&self.semantic.spans, &self.semantic.text));
            }
            self.highlights = (version, spans);
        }
        job
    }

    /// `language`'s colors for `text`, version `version`, worked out (if
    /// it's still that language: a file can be renamed meanwhile).
    pub fn highlighted(&mut self, language: &'static Language, version: u64, text: Arc<str>, spans: Vec<Span>) {
        if !self.language.is_some_and(|l| std::ptr::eq(l, language)) {
            return;
        }
        self.highlighting = false;
        self.grammar = (version, text, spans);
        self.recolor();
    }

    /// Highlight again next time, though the text's the same.
    pub fn recolor(&mut self) {
        self.highlights.0 = u64::MAX;
    }

    /// Whether another program changed the file since it was read or written here.
    pub fn changed_on_disk(&self) -> bool {
        self.path.as_deref().is_some_and(|path| Stamp::of(path) != self.disk)
    }

    /// Write to `path`, never half-written. Somewhere new is a copy until
    /// [`Document::moved`] there. Unless `force`d, it won't write over a
    /// file another program changed since it was read, or over another file.
    pub fn save(&mut self, path: &Path, force: bool) -> Result<(), String> {
        if self.pdf.is_some() {
            return Err("a PDF is read-only".into());
        }
        let ours = self.path.as_deref() == Some(path);
        if !force && ours && self.changed_on_disk() {
            return Err(format!("{} changed on disk since it was read (:w! writes over it, :rl! takes the file's)", self.name()));
        }
        if !force && !ours && path.exists() {
            return Err(format!("{} already exists (:w! writes over it)", path.display()));
        }
        ghostwrite_project::fs::write_atomic(path, denormalize(self.buffer.text(), self.crlf).as_bytes()).map_err(|e| e.to_string())?;
        if ours {
            (self.disk, self.conflict) = (Stamp::of(path), false);
        }
        self.dirty = false;
        if let Some(lsp) = self.lsp.as_ref().filter(|_| self.path.as_deref() == Some(path)) {
            // rust-analyzer and friends check the project on save.
            lsp.saved();
        }
        Ok(())
    }

    /// The file moved on disk: it may be another language now, with
    /// another server (or none).
    pub fn moved(&mut self, path: PathBuf, servers: &mut ghostwrite_lsp::Registry) {
        self.language = Language::for_path(&path);
        self.highlights.0 = u64::MAX;
        (self.grammar, self.highlighting) = ((u64::MAX, Arc::from(""), vec![]), false);
        self.preview = None;
        self.goals = None;
        self.semantic = Default::default();
        self.lsp = None;
        self.diagnostics.clear();
        self.buffer.take_edits();
        self.lsp = server_for(servers, &path, self.buffer.text());
        self.disk = Stamp::of(&path);
        self.path = Some(path);
    }

    /// Its language server stopped: open it on a new one (or none, if it keeps stopping).
    pub fn restart_server(&mut self, path: Option<&Path>, servers: &mut ghostwrite_lsp::Registry) {
        self.lsp = None;
        self.semantic = Default::default();
        self.goals = None;
        // It's opened with the text as it is: the edits before are no news.
        self.buffer.take_edits();
        self.lsp = path.and_then(|path| server_for(servers, path, self.buffer.text()));
        self.recolor();
    }

    /// Become what's on disk, `text` as read, as one undo step; returns
    /// where `selection` is now.
    pub fn reload(&mut self, text: String, selection: Range) -> Range {
        let (text, crlf) = normalize(text);
        let selection = self.buffer.replace_all(&text, selection);
        self.crlf = crlf;
        self.sync();
        self.on_disk();
        selection
    }

    /// The disk has what's here (just written or read): no changes, no conflict.
    pub fn on_disk(&mut self) {
        self.dirty = false;
        self.conflict = false;
        self.disk = self.path.as_deref().and_then(Stamp::of);
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<ghostwrite_lsp::RawDiagnostic>) {
        self.diagnostics = ghostwrite_lsp::convert::diagnostics(self.buffer.text(), diagnostics);
    }
}
