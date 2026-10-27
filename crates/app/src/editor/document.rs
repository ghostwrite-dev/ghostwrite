//! One open file: its buffer, where it lives on disk, its language and
//! highlights, its language server, and its live preview (Typst). How it's shown (selection, scroll)
//! is its tab's [`View`](super::tabs::View).

use std::path::{Path, PathBuf};

use ghostwrite_core::Buffer;
use ghostwrite_core::Language;
use ghostwrite_core::indent::IndentUnit;
use ghostwrite_core::syntax::Span;
use ghostwrite_lsp::Diagnostic;
use gpui::SharedString;

pub type DocId = usize;

/// `path` opened on its language's server, if one is installed.
pub fn server_for(servers: &mut ghostwrite_lsp::Registry, path: &Path, text: &str) -> Option<ghostwrite_lsp::Document> {
    let language = Language::for_path(path)?;
    let client = servers.start(language.servers)?;
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
    /// Before `lsp`, so it stops before the file closes on the server.
    pub preview: Option<ghostwrite_lsp::Preview>,
    pub lsp: Option<ghostwrite_lsp::Document>,
    pub diagnostics: Vec<Diagnostic>,
    /// The buffer version the language server (and `dirty`) last saw.
    synced: u64,
    highlights: (u64, Vec<Span>),
}

impl Document {
    pub fn new(id: DocId, path: Option<PathBuf>, text: String, lsp: Option<ghostwrite_lsp::Document>) -> Self {
        let buffer = Buffer::new(text);
        Self {
            id,
            indent: IndentUnit::detect(buffer.text()),
            language: path.as_deref().and_then(Language::for_path),
            synced: buffer.version(),
            buffer,
            path,
            dirty: false,
            preview: None,
            lsp,
            diagnostics: vec![],
            highlights: (u64::MAX, vec![]),
        }
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
        self.synced = self.buffer.version();
        self.dirty = true;
        if let Some(lsp) = &mut self.lsp {
            lsp.changed(self.buffer.text());
        }
    }

    /// The highlights as of the last [`Document::refresh_highlights`].
    pub fn cached_highlights(&self) -> &[Span] {
        &self.highlights.1
    }

    /// Re-highlight if the text changed since last time.
    pub fn refresh_highlights(&mut self) {
        let version = self.buffer.version();
        if self.highlights.0 != version {
            let spans = self.language.map(|l| l.highlight(self.buffer.text())).unwrap_or_default();
            self.highlights = (version, spans);
        }
    }

    /// Write to `path`. Somewhere new is a copy until [`Document::moved`] there.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.buffer.text())?;
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
        // Closes it on the old server first.
        self.preview = None;
        self.lsp = None;
        self.diagnostics.clear();
        self.lsp = server_for(servers, &path, self.buffer.text());
        self.path = Some(path);
    }

    /// Replace everything with what's on disk.
    pub fn reload(&mut self, text: String) {
        self.buffer.reset(text);
        self.sync();
        self.dirty = false;
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<ghostwrite_lsp::RawDiagnostic>) {
        self.diagnostics = ghostwrite_lsp::convert::diagnostics(self.buffer.text(), diagnostics);
    }
}
