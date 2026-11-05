//! One open file: its buffer, where it lives on disk, its language and
//! highlights, its language server, and what's beside it (see
//! [`crate::aside`]). How it's shown (selection, scroll) is its tab's
//! [`Viewport`](crate::tabs::Viewport). A file that's only pages is a line for each, read-only, and where
//! that text is on its pages.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::aside::Aside;
use crate::rendered::Rendered;
use ghostwrite_core::Language;
use ghostwrite_core::indent::IndentUnit;
use ghostwrite_core::syntax::{self, Colors, InputEdit, Span};
use ghostwrite_core::{Buffer, Range, Rope, Text as _};
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
    /// The grammar's colors as last worked out, for the text as it was then:
    /// they're shown moved past the edits since.
    colors: Option<Arc<Colors>>,
    /// They're being worked out.
    highlighting: bool,
    /// They couldn't be, for this version: not again till it's edited.
    failed: Option<u64>,
}

/// What colors a file, as it is now, from its colors as last worked out: to [`run`](Self::run)
/// off the UI thread, and give [`Document::highlighted`] what it makes.
pub struct HighlightJob {
    language: &'static Language,
    previous: Option<Arc<Colors>>,
    version: u64,
    text: Rope,
    edits: Option<Vec<InputEdit>>,
}

impl HighlightJob {
    /// The version of the text it's for.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn run(self) -> Colors {
        Colors::work(self.language, self.previous.as_deref(), self.version, self.text, self.edits)
    }
}

impl Document {
    /// `text` as read from `path` (its line endings are made `\n`, and put back on save).
    pub fn new(id: DocId, path: Option<PathBuf>, text: String, lsp: Option<ghostwrite_lsp::Document>) -> Self {
        let (text, crlf) = normalize(text);
        let buffer = Buffer::new(&text);
        Self {
            disk: path.as_deref().and_then(Stamp::of),
            read: Arc::from(text.as_str()),
            conflict: false,
            crlf,
            encoding: Default::default(),
            git: Default::default(),
            id,
            indent: IndentUnit::detect(&text),
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
            colors: None,
            highlighting: false,
            failed: None,
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

    /// What the language is called, for the model and the status line.
    pub fn language_name(&self) -> String {
        match (self.language, self.path.as_deref().and_then(Path::extension)) {
            (Some(language), _) => language.name.to_string(),
            (None, Some(ext)) => ext.to_string_lossy().into_owned(),
            (None, None) => "text".into(),
        }
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
        let text = &self.buffer;
        for d in &mut self.diagnostics {
            let snap = |at: usize| text.floor_boundary(at);
            d.range = snap(d.range.start)..snap(d.range.end).max(snap(d.range.start));
        }
        let edits = self.buffer.take_edits();
        if let Some(lsp) = &mut self.lsp {
            lsp.changed(self.buffer.rope(), edits);
        }
    }

    /// The colors in `range` of the text as it is: the grammar's, and the server's
    /// over them, each as last worked out, moved past the edits since. Only
    /// what's in `range` is looked at, so drawing a screenful costs as much in
    /// any file; what was edited since has none till they're worked out again.
    pub fn highlights_in(&self, range: std::ops::Range<usize>) -> Vec<Span> {
        let moved = |spans: &[Span], version: u64| match self.buffer.changes_since(version) {
            Some(edits) => syntax::spans_in(spans, &edits, range.clone()),
            None => vec![],
        };
        let grammar = self.colors.as_ref().map_or_else(Vec::new, |colors| moved(&colors.spans, colors.version));
        if self.semantic.spans.is_empty() {
            return grammar;
        }
        syntax::overlay(&grammar, &moved(&self.semantic.spans, self.semantic.version))
    }

    /// If the grammar's colors are out of date, and none are being worked out,
    /// what works them out: from the last ones, and the edits since, if it can
    /// (off the UI thread: a big file takes a while). [`Document::highlighted`] takes them.
    pub fn refresh_highlights(&mut self) -> Option<HighlightJob> {
        let version = self.buffer.version();
        if self.highlighting || self.failed == Some(version) || self.colors.as_ref().is_some_and(|colors| colors.version == version) {
            return None;
        }
        let language = self.language.filter(|_| !self.read_only())?;
        let previous = self.colors.clone().filter(|colors| std::ptr::eq(colors.language, language));
        let edits = previous.as_ref().and_then(|colors| self.buffer.changes_since(colors.version));
        self.highlighting = true;
        Some(HighlightJob { language, previous, version, text: self.buffer.rope().clone(), edits })
    }

    /// The colors for `version` couldn't be worked out (a grammar failed): they aren't tried again till it's edited.
    pub fn highlighting_failed(&mut self, version: u64) {
        (self.highlighting, self.failed) = (false, Some(version));
    }

    /// Colors worked out (by a [`HighlightJob`]): taken, if they're still of its language
    /// (a file can be renamed meanwhile), and newer than those it has, and of a version
    /// the text can be followed on from (it wasn't replaced since).
    pub fn highlighted(&mut self, colors: Colors) {
        self.highlighting = false;
        let newer = self.colors.as_ref().is_none_or(|had| had.version < colors.version);
        if self.language.is_some_and(|l| std::ptr::eq(l, colors.language)) && newer && self.buffer.changes_since(colors.version).is_some() {
            // Selecting by the syntax tree parses on from it, rather than from nothing.
            if let Some(tree) = colors.tree() {
                ghostwrite_core::nodes::offer(colors.language, &self.buffer, colors.version, tree);
            }
            self.colors = Some(Arc::new(colors));
        }
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
        let bytes = self.encoding.encode(&denormalize(&self.buffer.to_string(), self.crlf))?;
        ghostwrite_project::fs::write_atomic(path, &bytes).map_err(|e| e.to_string())?;
        if ours {
            (self.disk, self.conflict) = (Stamp::of(path), false);
        }
        self.dirty = false;
        if let Some(lsp) = self.lsp.as_ref().filter(|_| self.path.as_deref() == Some(path)) {
            // Many servers check the project on save.
            lsp.saved();
        }
        Ok(())
    }

    /// The file moved on disk: it may be another language now, with
    /// another server (or none).
    pub fn moved(&mut self, path: PathBuf, servers: &mut ghostwrite_lsp::Registry) {
        self.language = Language::for_path(&path);
        (self.colors, self.highlighting) = (None, false);
        self.aside = None;
        // Made from where it was.
        if !self.pages_are_the_text() {
            self.rendered = None;
        }
        self.semantic = Default::default();
        self.lsp = None;
        self.diagnostics.clear();
        self.buffer.take_edits();
        // Opened on its server: the whole text.
        self.lsp = server_for(servers, &path, &self.buffer.to_string());
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
        self.lsp = path.and_then(|path| server_for(servers, path, &self.buffer.to_string()));
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
        self.read = Arc::from(self.buffer.to_string());
    }

    /// Another program changed the file to `text` while this had unsaved
    /// changes: theirs go in with them if they're to other lines (as one undo
    /// step, the selection moving with the text). Whether they did.
    pub fn merge_from_disk(&mut self, text: &str, selection: Range) -> Option<Range> {
        let (theirs, _) = normalize(text.to_string());
        let ours = self.buffer.to_string();
        let merged = ghostwrite_project::merge::merge(&self.read, &ours, &theirs)?;
        // Each of their changes on its own, last first: what's between (the cursor) stays put.
        let edits = ghostwrite_project::merge::edits(&ours, &merged);
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
        self.diagnostics = ghostwrite_lsp::convert::diagnostics(self.buffer.rope(), diagnostics);
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
    use ghostwrite_core::Language;
    use ghostwrite_core::syntax::{Colors, Span};
    use ghostwrite_lsp::{Diagnostic, Severity};

    use super::{Document, next_problem};

    /// Colors every `x`, as a plugin's code might color a file: all of it each time.
    fn xs(text: &str) -> Vec<Span> {
        text.match_indices('x').map(|(i, _)| Span { range: i..i + 1, scope: 0 }).collect()
    }

    static XS: Language = Language { name: "xs", highlighter: Some(&xs), ..Language::DEFAULT };

    /// Where it's colored, start and end.
    fn colored(doc: &Document) -> Vec<(usize, usize)> {
        doc.highlights_in(0..doc.buffer.len()).into_iter().map(|s| (s.range.start, s.range.end)).collect()
    }

    #[test]
    fn colors_worked_out_meanwhile_follow_the_text_and_older_ones_never_go_over_newer() {
        let mut doc = Document::new(0, None, "a x b\n".into(), None);
        doc.language = Some(&XS);
        let job = doc.refresh_highlights().expect("they're worked out");
        assert!(doc.refresh_highlights().is_none(), "one at a time");
        // Typing goes on while they're worked out (off the UI thread).
        doc.buffer.edit(0..0, "yy");
        let (old_version, old_text) = (doc.buffer.version(), doc.buffer.rope().clone());
        doc.highlighted(job.run());
        assert_eq!(colored(&doc), [(4, 5)], "the x, moved past what was typed since");

        // Newer ones come; then, late, some for the text before them: they aren't taken.
        doc.buffer.edit(0..0, "x");
        let job = doc.refresh_highlights().expect("out of date");
        doc.highlighted(job.run());
        assert_eq!(colored(&doc), [(0, 1), (5, 6)]);
        doc.highlighted(Colors::new(&XS, old_version, old_text));
        assert_eq!(colored(&doc), [(0, 1), (5, 6)], "older than what's shown: dropped");
        assert!(doc.refresh_highlights().is_none(), "nothing left to do");

        // The text replaced while they're worked out: they can't be followed on to it, so they aren't shown.
        doc.buffer.edit(0..0, "z");
        let stale = doc.refresh_highlights().expect("out of date");
        doc.buffer.reset("b x");
        let none: [(usize, usize); 0] = [];
        assert_eq!(colored(&doc), none, "nothing, rather than colors in the wrong places");
        doc.highlighted(stale.run());
        assert_eq!(colored(&doc), none, "nor the colors of the text it replaced");
        let job = doc.refresh_highlights().expect("out of date");
        doc.highlighted(job.run());
        assert_eq!(colored(&doc), [(2, 3)]);
    }

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
