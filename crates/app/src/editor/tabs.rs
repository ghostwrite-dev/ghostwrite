//! The open documents, each in a tab with its [`View`]: the selection and
//! where it's scrolled to. Like helix, the document is the file and the view
//! is how it's being looked at; the tab list knows nothing about policy
//! (when files are written, what's focused), which is the editor's.

use std::path::Path;

use ghostwrite_core::Range;
use ghostwrite_core::modal::Target;
use ghostwrite_lsp::Diagnostic;
use std::time::Instant;

use gpui::{Pixels, Point, px};

use super::document::{DocId, Document};
use super::rendered::PagesScroll;

/// Where the view should move to show the cursor (or some lines), next time it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Autoscroll {
    /// Just enough to show the cursor.
    Fit,
    /// These lines in the middle, plus room for what folds out under them.
    Center(std::ops::Range<usize>),
    /// The cursor's line at the top of the screen (`zt`), or the bottom (`zb`).
    Top,
    Bottom,
}

pub struct View {
    pub selection: Range,
    /// More cursors, besides the main one that's `selection`.
    pub others: Vec<Range>,
    /// Where it's scrolled to. What's drawn may still be on its way there: see [`Motion`].
    pub scroll_top: Pixels,
    pub scroll_left: Pixels,
    pub autoscroll: Option<Autoscroll>,
    /// Where its pages are scrolled to.
    pub pages: PagesScroll,
    /// What's moving on screen.
    pub motion: Motion,
}

impl Default for View {
    fn default() -> Self {
        Self {
            selection: Range::point(0),
            others: vec![],
            scroll_top: px(0.),
            scroll_left: px(0.),
            autoscroll: None,
            pages: PagesScroll::default(),
            motion: Motion::default(),
        }
    }
}

/// What glides rather than jumps, so the eye can follow: the view scrolled by
/// the keys, the cursor, and the lines a story's step just changed, lit up for a
/// moment. `"animations": false` in the config stills them.
#[derive(Debug, Clone, Default)]
pub struct Motion {
    /// Where the text is drawn scrolled to.
    pub drawn_top: Pixels,
    /// Scrolling from a place to another, since when.
    pub scroll: Option<(Pixels, Pixels, Instant)>,
    /// Where the cursor was drawn, in the text (scrolling doesn't move it).
    pub cursor: Option<Point<Pixels>>,
    /// The cursor on its way from a place to another, since when.
    pub cursor_glide: Option<(Point<Pixels>, Point<Pixels>, Instant)>,
    /// Text just changed, lit up and fading, since when.
    pub flash: Option<(std::ops::Range<usize>, Instant)>,
}

pub struct Tab {
    pub doc: Document,
    pub view: View,
}

impl Tab {
    /// Select `range` and bring it into view.
    pub fn select(&mut self, range: Range, scroll: Autoscroll) {
        self.view.others.clear();
        self.view.selection = range.within(self.doc.text());
        self.view.autoscroll = Some(scroll);
    }

    /// After an edit: the document syncs, and the selection stays in the text.
    pub fn sync(&mut self) {
        self.doc.sync();
        self.view.selection = self.view.selection.within(self.doc.text());
    }

    /// Take the file's new text from disk as one undo step, the selection moving with the text.
    pub fn reload(&mut self, text: String) {
        self.view.selection = self.doc.reload(text, self.view.selection);
        self.sync();
    }

    /// What modal editing works on.
    pub fn target(&mut self, page_rows: usize) -> Target<'_> {
        Target { buffer: &mut self.doc.buffer, selection: &mut self.view.selection, indent: &self.doc.indent, page_rows, language: self.doc.language }
    }

    /// The char the block cursor is on.
    pub fn cursor(&self) -> usize {
        self.view.selection.cursor(self.doc.text())
    }

    /// The cursor as (zero-based line, char column).
    pub fn cursor_position(&self) -> (usize, usize) {
        let (buffer, cursor) = (&self.doc.buffer, self.cursor());
        let line = buffer.line_of(cursor);
        (line, buffer.text()[buffer.line_start(line)..cursor].chars().count())
    }

    /// The diagnostic under the cursor, most severe first.
    pub fn diagnostic_at_cursor(&self) -> Option<&Diagnostic> {
        let at = self.cursor();
        self.doc.diagnostics.iter().filter(|d| d.range.start <= at && at <= d.range.end.max(d.range.start + 1)).min_by_key(|d| d.severity)
    }
}

#[derive(Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: DocId,
    /// A tab that isn't taken over, even while it's untouched.
    held: Option<DocId>,
}

impl Tabs {
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn get(&self, ix: usize) -> Option<&Tab> {
        self.tabs.get(ix)
    }

    pub fn get_mut(&mut self, ix: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(ix)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    /// The tab of `path`, however it's spelled (see [`ghostwrite_project::same_file`]).
    pub fn index_of_path(&self, path: &Path) -> Option<usize> {
        let exactly = self.tabs.iter().position(|t| t.doc.path.as_deref() == Some(path));
        exactly.or_else(|| self.tabs.iter().position(|t| t.doc.path.as_deref().is_some_and(|p| ghostwrite_project::same_file(p, path))))
    }

    pub fn index_of_id(&self, id: DocId) -> Option<usize> {
        self.tabs.iter().position(|t| t.doc.id == id)
    }

    pub fn by_id(&self, id: DocId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.doc.id == id)
    }

    pub fn by_id_mut(&mut self, id: DocId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.doc.id == id)
    }

    /// The text of an open file, as it is on screen.
    pub fn text_of(&self, path: &Path) -> Option<&str> {
        self.tabs.iter().find(|t| t.doc.path.as_deref() == Some(path)).map(|t| t.doc.text())
    }

    /// Keep `doc` from being taken over, even while it's untouched; `None`, no longer.
    pub fn hold(&mut self, doc: Option<DocId>) {
        self.held = doc;
    }

    /// Add a tab for a new document (taking over an untouched untitled one, unless it's held), and return its index.
    pub fn add(&mut self, make: impl FnOnce(DocId) -> Document) -> usize {
        let doc = make(self.next_id);
        self.next_id += 1;
        if self.active().is_some_and(|t| t.doc.is_pristine() && Some(t.doc.id) != self.held) {
            self.tabs.remove(self.active);
        }
        self.tabs.push(Tab { doc, view: View::default() });
        self.tabs.len() - 1
    }

    /// Move the tab at `from` to be at `to`; the active tab stays the active one.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() {
            return;
        }
        let active = self.active().map(|t| t.doc.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to.min(self.tabs.len()), tab);
        self.active = active.and_then(|id| self.index_of_id(id)).unwrap_or(self.active);
    }

    pub fn set_active(&mut self, ix: usize) {
        self.active = ix.min(self.tabs.len().saturating_sub(1));
    }

    pub fn remove(&mut self, ix: usize) -> Option<Tab> {
        (ix < self.tabs.len()).then(|| self.tabs.remove(ix))
    }

    /// Keep the tabs `keep` says to, staying on the active one if it's kept.
    pub fn retain(&mut self, keep: impl FnMut(&Tab) -> bool) {
        let active = self.active().map(|t| t.doc.id);
        self.tabs.retain(keep);
        self.active = active.and_then(|id| self.index_of_id(id)).unwrap_or(self.active).min(self.tabs.len().saturating_sub(1));
    }
}
