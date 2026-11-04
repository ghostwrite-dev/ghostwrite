//! The mouse in the code (and on pages, which stand for it): a click puts
//! the cursor, dragging from it selects, the wheel scrolls. The views find
//! what's under the mouse; what that does is here.

use ghostwrite_core::{Mode, Range};
use gpui::{Pixels, Point, px};

use super::{Autoscroll, Editor};

#[derive(Default)]
pub(super) struct Pointer {
    /// A click in the code is being dragged: it selects.
    pub(super) selecting: bool,
}

/// Where a click at `at` leaves `selection`, and the mode. It puts the
/// cursor there; with shift, or in select mode (`v`), it extends the
/// selection to there instead. A double click selects the word, a triple
/// click the line. The mode stays as it was.
fn clicked(buffer: &ghostwrite_core::Buffer, selection: Range, mode: Mode, at: usize, extend: bool, clicks: usize) -> (Range, Mode) {
    let text = buffer.text();
    let to = |anchor: usize| Range::new(anchor, if mode == Mode::Insert || at < anchor { at } else { ghostwrite_core::next_char(text, at) });
    if clicks >= 3 {
        let line = buffer.line_of(at);
        let lines = buffer.lines_range(line..line + 1);
        return (Range::new(lines.start, lines.end), mode);
    }
    if clicks == 2 {
        let word = ghostwrite_core::textobject::text_object(buffer, Range::point(at), 'w', false, 1).unwrap_or(Range::point(at));
        return (word, mode);
    }
    if extend || mode == Mode::Select { (to(selection.anchor), mode) } else { (Range::point(at), mode) }
}

impl Editor {
    /// A click in the text: see [`clicked`].
    pub(super) fn click(&mut self, at: usize, extend: bool, clicks: usize, cx: &mut gpui::Context<Self>) {
        if self.resizing() {
            return;
        }
        self.modal.leave_tree();
        self.modal.leave(Mode::Terminal);
        self.stop_reading();
        let mode = self.modal.mode;
        let Some(tab) = self.tab_mut() else { return };
        let (selection, mode) = clicked(&tab.doc.buffer, tab.view.selection, mode, at, extend, clicks);
        tab.select(selection, Autoscroll::Fit);
        self.modal.mode = mode;
        self.wander();
        self.completion = None;
        self.pointer.selecting = clicks == 1;
        cx.notify();
    }

    /// The mouse dragged, from a click, to `at`: select from where it was pressed.
    pub(super) fn drag_to(&mut self, at: usize, cx: &mut gpui::Context<Self>) {
        let mode = self.modal.mode;
        let Some(tab) = self.tab_mut() else { return };
        let (selection, _) = clicked(&tab.doc.buffer, tab.view.selection, Mode::Select, at, true, 1);
        let selection = if mode == Mode::Insert { Range::new(selection.anchor, at) } else { selection };
        if selection != tab.view.selection {
            tab.select(selection, Autoscroll::Fit);
            cx.notify();
        }
    }

    pub(super) fn scroll_by(&mut self, other: Option<super::DocId>, delta: Point<Pixels>, cx: &mut gpui::Context<Self>) {
        let tab = match other {
            Some(doc) => self.project.tabs.by_id_mut(doc),
            None => self.tab_mut(),
        };
        let Some(tab) = tab else { return };
        tab.view.scroll_top = (tab.view.scroll_top - f32::from(delta.y)).max(0.);
        if other.is_none() && delta.y != px(0.) {
            self.wander();
        }
        cx.notify();
    }
}
