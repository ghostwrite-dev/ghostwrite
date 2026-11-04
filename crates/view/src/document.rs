//! One open file: its buffer, where it lives on disk, its language and
//! highlights, its language server, and what's beside it (see
//! [`crate::aside`]). How it's shown (selection, scroll) is its tab's
//! [`View`](crate::tabs::View). A file that's only pages is a line for each, read-only, and where
//! that text is on its pages.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::aside::Aside;
use crate::rendered::Rendered;
use ghostwrite_core::Language;
use ghostwrite_core::indent::IndentUnit;
use ghostwrite_core::syntax::{self, Span};
use ghostwrite_core::{Buffer, Range};
use ghostwrite_lsp::Diagnostic;
use ghostwrite_project::fs::{Stamp, denormalize, normalize};

pub type DocId = usize;

/// Where a file differs from what git has: worked out off the UI thread as it changes.
#[derive(Default)]
pub struct GitChanges {
    /// The file as git has it, once it's been asked for.
    pub base: Option<Arc<str>>,
    pub asked: bool,
    pub making: bool,
    /// The buffer version the changes are of.
    pub made: Option<u64>,
    pub changes: Vec<ghostwrite_project::git::Change>,
}

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
    /// Its text then (line endings made `\n`): what unsaved changes and another
    /// program's are both changes from, to merge them.
    pub read: Arc<str>,
    /// Another program changed the file while this had unsaved changes.
    pub conflict: bool,
    /// The file has `\r\n` line endings: the buffer has `\n`, and they're put back on save.
    pub crlf: bool,
    /// How the file is stored, to be written back the same way.
    pub encoding: ghostwrite_project::fs::Encoding,
    /// Where it differs from what git has.
    pub git: GitChanges,
    /// What's beside it, if anything, made once it's first shown.
    pub aside: Option<Aside>,
    /// Its pages, if it has any, made once they're first asked for.
    pub rendered: Option<Rendered>,
    /// Its server's colors, laid over the grammar's.
    pub semantic: crate::semantic::Semantic,
    pub lsp: Option<ghostwrite_lsp::Document>,
    pub diagnostics: Vec<Diagnostic>,
    /// A tab that shows another document (its pages): which, and it's called this.
    /// It has no text of its own, and it goes when that document does.
    pub view_of: Option<(DocId, String)>,
    /// It's what's on the terminal, to read through.
    pub output: bool,
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
            read: Arc::from(buffer.text()),
            conflict: false,
            crlf,
            encoding: Default::default(),
            git: Default::default(),
            id,
            indent: IndentUnit::detect(buffer.text()),
            language: path.as_deref().and_then(Language::for_path),
            synced: buffer.version(),
            buffer,
            path,
            dirty: false,
            aside: None,
            rendered: None,
            semantic: Default::default(),
            lsp,
            diagnostics: vec![],
            view_of: None,
            output: false,
            highlights: (u64::MAX, vec![]),
            grammar: (u64::MAX, Arc::from(""), vec![]),
            highlighting: false,
        }
    }

    /// A tab called `name` showing the document `of`.
    pub fn view(id: DocId, of: DocId, name: String) -> Self {
        let mut doc = Self::new(id, None, String::new(), None);
        doc.view_of = Some((of, name));
        doc
    }

    pub fn is_view_of(&self, of: DocId) -> bool {
        self.view_of.as_ref().is_some_and(|(file, _)| *file == of)
    }

    /// The text its pages are what it comes from, if they are.
    fn fixed_text(&self) -> Option<&str> {
        self.rendered.as_ref()?.pages.as_ref()?.text()
    }

    /// Whether its pages are its text, so that's what its tab shows.
    pub fn pages_are_the_text(&self) -> bool {
        self.is_only_pages() || self.fixed_text().is_some()
    }

    /// Whether it isn't text at all: it's the pages a plugin makes of it.
    pub fn is_only_pages(&self) -> bool {
        self.language.is_some_and(|language| language.only_pages)
    }

    /// Whether it can't be edited: its text is what its pages say, or it has none.
    pub fn read_only(&self) -> bool {
        self.pages_are_the_text() || self.view_of.is_some() || self.output
    }

    pub fn name(&self) -> Arc<str> {
        if let Some((_, name)) = &self.view_of {
            return name.as_str().into();
        }
        self.path.as_deref().and_then(Path::file_name).map_or("untitled".into(), |n| n.to_string_lossy().into())
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
        self.path.is_none() && !self.dirty && self.buffer.is_empty() && self.view_of.is_none()
    }

    /// After the buffer changed: mark it dirty and tell the language server.
    pub fn sync(&mut self) {
        if self.buffer.version() == self.synced {
            return;
        }
        // The terminal's text: what reads it keeps it as the terminal has it.
        if self.output {
            self.synced = self.buffer.version();
            return;
        }
        // Every edit comes through here; those of what can't change are undone.
        if self.read_only() {
            let text = self.fixed_text().unwrap_or_default().to_string();
            self.buffer.reset(text);
            self.synced = self.buffer.version();
            return;
        }
        self.synced = self.buffer.version();
        self.dirty = true;
        // Diagnostics are of the text the server last saw: until it sends new
        // ones, kept in the text and on char boundaries, so they can be drawn.
        let text = self.buffer.text();
        for d in &mut self.diagnostics {
            let snap = |at: usize| text.floor_char_boundary(at.min(text.len()));
            d.range = snap(d.range.start)..snap(d.range.end).max(snap(d.range.start));
        }
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
            .then(|| self.language.filter(|_| !self.read_only()))
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
        if self.read_only() {
            return Err("this is read-only".into());
        }
        let ours = self.path.as_deref() == Some(path);
        if !force && ours && self.changed_on_disk() {
            return Err(format!("{} changed on disk since it was read (:w! writes over it, :rl! takes the file's)", self.name()));
        }
        if !force && !ours && path.exists() {
            return Err(format!("{} already exists (:w! writes over it)", path.display()));
        }
        let bytes = self.encoding.encode(&denormalize(self.buffer.text(), self.crlf))?;
        ghostwrite_project::fs::write_atomic(path, &bytes).map_err(|e| e.to_string())?;
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
        self.aside = None;
        // Made from where it was.
        if !self.pages_are_the_text() {
            self.rendered = None;
        }
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
        self.aside = None;
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
        self.read = Arc::from(self.buffer.text());
    }

    /// Another program changed the file to `text` while this had unsaved
    /// changes: theirs go in with them if they're to other lines (as one undo
    /// step, the selection moving with the text). Whether they did.
    pub fn merge_from_disk(&mut self, text: &str, selection: Range) -> Option<Range> {
        let (theirs, _) = normalize(text.to_string());
        let merged = ghostwrite_project::merge::merge(&self.read, self.buffer.text(), &theirs)?;
        // Each of their changes on its own, last first: what's between (the cursor) stays put.
        let edits = ghostwrite_project::merge::edits(self.buffer.text(), &merged);
        // Where a byte is once they're in: after a change (or at lines put in above it), moved
        // by it; inside one, as far into what replaced it as there is.
        let map = |at: usize| {
            let mut moved = at;
            for (range, with) in &edits {
                if range.end <= at {
                    moved = moved + with.len() - range.len();
                } else if range.start < at {
                    moved = moved - (at - range.start) + (at - range.start).min(with.len());
                }
            }
            moved
        };
        let after = Range::new(map(selection.anchor), map(selection.head));
        if !edits.is_empty() {
            self.buffer.begin(selection);
            for (range, with) in edits.iter().rev() {
                self.buffer.edit(range.clone(), with);
            }
            self.buffer.commit(after);
        }
        let selection = after;
        self.sync();
        // Still unsaved (it has what's on disk and more), but saving won't lose theirs.
        self.disk = self.path.as_deref().and_then(Stamp::of);
        self.read = Arc::from(theirs.as_str());
        self.conflict = false;
        self.dirty = merged != theirs;
        Some(selection)
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<ghostwrite_lsp::RawDiagnostic>) {
        self.diagnostics = ghostwrite_lsp::convert::diagnostics(self.buffer.text(), diagnostics);
    }
}

/// The problem `space j` goes to from `cursor`, and which of how many of
/// its kind it is: the next error after it (round to the first), or with
/// no errors, the next warning.
pub fn next_problem(diagnostics: &[ghostwrite_lsp::Diagnostic], cursor: usize) -> Option<(&ghostwrite_lsp::Diagnostic, usize, usize)> {
    use ghostwrite_lsp::Severity;
    let worst = [Severity::Error, Severity::Warning].into_iter().find(|s| diagnostics.iter().any(|d| d.severity == *s))?;
    let mut kind: Vec<&ghostwrite_lsp::Diagnostic> = diagnostics.iter().filter(|d| d.severity == worst).collect();
    kind.sort_by_key(|d| d.range.start);
    kind.dedup_by_key(|d| d.range.start);
    let ix = kind.iter().position(|d| d.range.start > cursor).unwrap_or(0);
    Some((kind[ix], ix + 1, kind.len()))
}

#[cfg(test)]
mod tests {
    use ghostwrite_lsp::{Diagnostic, Severity};

    use super::next_problem;

    #[test]
    fn space_j_goes_to_the_next_error_round_to_the_first_then_warnings() {
        let d = |at: usize, severity| Diagnostic { range: at..at + 1, severity, message: format!("at {at}") };
        let all = [d(30, Severity::Error), d(5, Severity::Warning), d(10, Severity::Error), d(20, Severity::Info)];
        let at = |cursor| next_problem(&all, cursor).map(|(d, nth, of)| (d.range.start, nth, of));
        assert_eq!(at(0), Some((10, 1, 2)));
        assert_eq!(at(10), Some((30, 2, 2)));
        assert_eq!(at(30), Some((10, 1, 2)), "round to the first");
        let warnings = [d(5, Severity::Warning), d(20, Severity::Info)];
        assert_eq!(next_problem(&warnings, 9).map(|(d, ..)| d.range.start), Some(5), "no errors: the warnings");
        assert!(next_problem(&[d(20, Severity::Info)], 0).is_none(), "info isn't a problem");
    }
}
