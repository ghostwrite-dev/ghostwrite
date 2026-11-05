//! Text from the platform: typing and input methods (marked text), into
//! the active document in insert mode. The platform counts in UTF-16.

use std::ops::Range as Bytes;

use ghostwrite_core::{Mode, Range, Rope};
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use super::{Autoscroll, Editor};

impl Editor {
    /// `text` typed at the cursor, as if from the keyboard (a macro played back).
    pub(super) fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.insert_text(text, None, false, cx);
    }

    /// Typed text, in insert mode (anywhere else it's ignored). Text being
    /// composed (a dead key's accent, an input method's) goes in `as_is`:
    /// no closing quote comes with it, nor an abbreviation.
    fn insert_text(&mut self, text: &str, replacing: Option<Bytes<usize>>, as_is: bool, cx: &mut Context<Self>) {
        if self.modal.mode != Mode::Insert || !self.in_code() {
            return;
        }
        // Composed text is recorded when it's committed, as typed.
        if replacing.is_none() || !as_is {
            self.record_text(text);
        }
        let Some(tab) = self.project.tabs.active_mut() else { return };
        // What the platform replaces (what's being composed) goes first, as
        // part of the typing; typing at the cursor leaves the selection be.
        if let Some(range) = replacing.filter(|range| *range != (tab.view.selection.head..tab.view.selection.head)) {
            let range = range.start.min(tab.doc.buffer.len())..range.end.min(tab.doc.buffer.len());
            tab.doc.buffer.begin(tab.view.selection);
            tab.doc.buffer.edit(range.clone(), "");
            tab.view.selection = Range::point(range.start);
        }
        if as_is {
            // Being composed, by an input method: at the one cursor it's asking about.
            self.modal.insert_as_is(text, &mut tab.target(1));
        } else if tab.view.others.is_empty() {
            self.modal.insert(text, &mut tab.target(1));
        } else {
            self.typed_at_each(|modal, tab| {
                modal.insert(text, &mut tab.target(1));
            });
        }
        let Some(tab) = self.project.tabs.active_mut() else { return };
        tab.view.autoscroll = Some(Autoscroll::Fit);
        tab.sync();
        self.after_typing(text, cx);
        cx.notify();
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range: Bytes<usize>, actual: &mut Option<Bytes<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let text = &self.tab()?.doc.buffer;
        let range = from_utf16(text.rope(), &range);
        actual.replace(to_utf16(text.rope(), &range));
        Some(text.slice(range).into_owned())
    }

    /// In insert mode, where typing goes: the head (the selection stays, and isn't typed over).
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let tab = self.tab()?;
        let sel = tab.view.selection;
        let range = if self.modal.mode == Mode::Insert { sel.head..sel.head } else { sel.bytes() };
        Some(UTF16Selection { range: to_utf16(tab.doc.buffer.rope(), &range), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Bytes<usize>> {
        Some(to_utf16(self.tab()?.doc.buffer.rope(), self.marked.as_ref()?))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        // Committing what was being composed. One char is as if typed: a
        // quote from a dead key (US International's `"`) closes itself.
        let composed = self.marked.is_some() && text.chars().nth(1).is_some();
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.buffer.rope(), &range?)));
        self.insert_text(text, replacing, composed, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: Option<Bytes<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.buffer.rope(), &range?)));
        let start = replacing.as_ref().map(|r| r.start).or_else(|| self.tab().map(|t| t.view.selection.head));
        self.insert_text(text, replacing, true, cx);
        if self.modal.mode == Mode::Insert {
            self.marked = start.filter(|_| !text.is_empty()).map(|s| s..s + text.len());
        }
    }

    fn bounds_for_range(&mut self, _: Bytes<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        self.measured.cursor
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Bytes as the system counts them, in UTF-16 units: the rope has them counted.
fn to_utf16(text: &Rope, range: &Bytes<usize>) -> Bytes<usize> {
    let units = |at: usize| text.char_to_utf16_cu(text.byte_to_char(at.min(text.len_bytes())));
    units(range.start)..units(range.end)
}

fn from_utf16(text: &Rope, range: &Bytes<usize>) -> Bytes<usize> {
    let byte = |units: usize| text.char_to_byte(text.utf16_cu_to_char(units.min(text.len_utf16_cu())));
    byte(range.start)..byte(range.end)
}
