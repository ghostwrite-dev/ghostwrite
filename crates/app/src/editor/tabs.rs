//! The open documents, each in a tab with its [`View`]: the selection and
//! where it's scrolled to. Like helix, the document is the file and the view
//! is how it's being looked at; the tab list knows nothing about policy
//! (when files are written, what's focused), which is the editor's.

use std::path::Path;

use ghostwrite_core::modal::Target;
use ghostwrite_core::Range;
use ghostwrite_lsp::Diagnostic;
use gpui::{Pixels, px};

use super::document::{DocId, Document};

/// Where the view should move to show the cursor (or some lines), next time it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Autoscroll {
    /// Just enough to show the cursor.
    Fit,
    /// These lines in the middle, plus room for what folds out under them.
    Center(std::ops::Range<usize>),
}

pub struct View {
    pub selection: Range,
    pub scroll_top: Pixels,
    pub scroll_left: Pixels,
    pub autoscroll: Option<Autoscroll>,
}

impl Default for View {
    fn default() -> Self {
        Self { selection: Range::point(0), scroll_top: px(0.), scroll_left: px(0.), autoscroll: None }
    }
}

pub struct Tab {
    pub doc: Document,
    pub view: View,
}

impl Tab {
    /// Select `range` and bring it into view.
    pub fn select(&mut self, range: Range, scroll: Autoscroll) {
        self.view.selection = range.clamp(self.doc.buffer.len());
        self.view.autoscroll = Some(scroll);
    }

    /// After an edit: the document syncs, and the selection stays in the text.
    pub fn sync(&mut self) {
        self.doc.sync();
        self.view.selection = self.view.selection.clamp(self.doc.buffer.len());
    }

    /// What modal editing works on.
    pub fn target(&mut self, page_rows: usize) -> Target<'_> {
        Target { buffer: &mut self.doc.buffer, selection: &mut self.view.selection, indent: &self.doc.indent, page_rows }
    }

    /// The char the block cursor is on.
    pub fn cursor(&self) -> usize {
        self.view.selection.cursor(self.doc.text())
    }

    /// The cursor as (zero-based line, char column).
    pub fn cursor_position(&self) -> (usize, usize) {
        ghostwrite_core::line_col(self.doc.text(), self.cursor())
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

    pub fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.doc.path.as_deref() == Some(path))
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

    /// Add a tab for a new document (taking over an untouched untitled one), and return its index.
    pub fn add(&mut self, make: impl FnOnce(DocId) -> Document) -> usize {
        let doc = make(self.next_id);
        self.next_id += 1;
        if self.active().is_some_and(|t| t.doc.is_pristine()) {
            self.tabs.remove(self.active);
        }
        self.tabs.push(Tab { doc, view: View::default() });
        self.tabs.len() - 1
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
