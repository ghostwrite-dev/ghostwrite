//! Two panes side by side, each showing a tab: code beside its pages, say, or two
//! files. One has the keys; the other shows its tab, and a click there brings the keys
//! over. Going to the tab that's in the other pane moves to that pane, not the tab.
//! Each tab keeps its own cursor and scroll, so one tab isn't in both panes.

use gpui::{AnyElement, Context, div, prelude::*};

use super::element::CodeView;
use super::page_view::PageView;
use super::{DocId, Editor};
use crate::theme::theme;

pub(super) struct Split {
    /// The tab in the pane without the keys.
    pub(super) other: DocId,
    /// Which side that pane is on, so that panes stay put as the keys move.
    pub(super) other_on_right: bool,
}

impl Editor {
    /// The tab in the other pane, if there are two panes showing two tabs.
    pub(super) fn other_pane_doc(&self) -> Option<DocId> {
        let other = self.split.as_ref()?.other;
        (self.project.tabs.by_id(other).is_some() && self.tab().is_some_and(|tab| tab.doc.id != other)).then_some(other)
    }

    /// `space w v`, `:vsplit`: a second pane, with this file's pages if they're open, else the next tab.
    pub(super) fn split_right(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let here = self.tab().ok_or("no file open")?.doc.id;
        let pages = self.project.tabs.iter().find(|tab| tab.doc.is_view_of(here)).map(|tab| tab.doc.id);
        let count = self.project.tabs.len();
        let next = (1..count).map(|step| (self.project.tabs.active_index() + step) % count).find_map(|ix| self.project.tabs.get(ix).map(|tab| tab.doc.id));
        let other = pages.or(next).ok_or("open another file first: each pane shows its own tab")?;
        self.split = Some(Split { other, other_on_right: true });
        cx.notify();
        Ok(())
    }

    /// `space w w`: the keys go to the other pane.
    pub(super) fn other_pane(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = self.other_pane_doc() {
            self.go_to_doc(doc, cx);
        }
    }

    /// `space w q`, `space w o`, `:only`: only the pane with the keys.
    pub(super) fn close_split(&mut self, cx: &mut Context<Self>) {
        self.split = None;
        cx.notify();
    }

    /// The tab of `doc` gets the keys.
    pub(super) fn go_to_doc(&mut self, doc: DocId, cx: &mut Context<Self>) {
        if let Some(ix) = self.project.tabs.index_of_id(doc) {
            self.activate(ix, cx);
        }
    }

    /// About to go to tab `ix`: if it's in the other pane, the keys go there, and
    /// the tab they leave stays in its pane.
    pub(super) fn split_follows(&mut self, ix: usize) {
        let Some(here) = self.tab().map(|tab| tab.doc.id) else { return };
        let Some(going) = self.project.tabs.get(ix).map(|tab| tab.doc.id) else { return };
        if let Some(split) = self.split.as_mut().filter(|split| split.other == going && going != here) {
            split.other = here;
            split.other_on_right = !split.other_on_right;
        }
    }

    /// What's in the other pane.
    pub(super) fn render_other_pane(&self, doc: DocId, cx: &mut Context<Self>) -> AnyElement {
        if self.pages_of(doc).is_some() {
            let view = PageView::of_other_pane(cx.entity(), self.focus.clone(), doc);
            return div().size_full().debug_selector(|| "other-pane".to_string()).child(view).into_any_element();
        }
        let view = CodeView::of_other_pane(cx.entity(), self.focus.clone(), doc);
        div().size_full().debug_selector(|| "other-pane".to_string()).child(view).into_any_element()
    }

    /// The panes side by side, with a line between them.
    pub(super) fn beside(&self, keys: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let Some(doc) = self.other_pane_doc() else { return keys };
        let on_right = self.split.as_ref().is_none_or(|split| split.other_on_right);
        let border = theme(cx).border;
        let other = self.render_other_pane(doc, cx);
        let pane = |view: AnyElement, line: bool| div().flex_1().min_w_0().h_full().when(line, |d| d.border_l_1().border_color(border)).child(view);
        let (left, right) = if on_right { (keys, other) } else { (other, keys) };
        div().size_full().flex().child(pane(left, false)).child(pane(right, true)).into_any_element()
    }
}
