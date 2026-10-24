//! Text from the platform: typing and input methods (marked text), into
//! the active document in insert mode. The platform counts in UTF-16.

use std::ops::Range as Bytes;

use ghostwrite_core::{Mode, Range};
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use super::{Autoscroll, Editor};

impl Editor {
    /// Typed text, in insert mode (anywhere else it's ignored).
    fn insert_text(&mut self, text: &str, replacing: Option<Bytes<usize>>, cx: &mut Context<Self>) {
        if self.modal.mode != Mode::Insert {
            return;
        }
        let Some(tab) = self.project.tabs.active_mut() else { return };
        if let Some(range) = replacing {
            tab.view.selection = Range::new(range.start, range.end).clamp(tab.doc.buffer.len());
        }
        self.modal.insert(text, &mut tab.target(1));
        tab.view.autoscroll = Some(Autoscroll::Fit);
        tab.sync();
        self.after_typing(text, cx);
        cx.notify();
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range: Bytes<usize>, actual: &mut Option<Bytes<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let text = self.tab()?.doc.text();
        let range = from_utf16(text, &range);
        actual.replace(to_utf16(text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let tab = self.tab()?;
        let sel = tab.view.selection;
        Some(UTF16Selection { range: to_utf16(tab.doc.text(), &sel.bytes()), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Bytes<usize>> {
        Some(to_utf16(self.tab()?.doc.text(), self.marked.as_ref()?))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.text(), &range?)));
        self.insert_text(text, replacing, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Bytes<usize>>, text: &str, _: Option<Bytes<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let replacing = self.marked.take().or_else(|| Some(from_utf16(self.tab()?.doc.text(), &range?)));
        let start = replacing.as_ref().map(|r| r.start).or_else(|| self.tab().map(|t| t.view.selection.from()));
        self.insert_text(text, replacing, cx);
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

fn to_utf16(text: &str, range: &Bytes<usize>) -> Bytes<usize> {
    let units = |at: usize| text[..at.min(text.len())].chars().map(char::len_utf16).sum::<usize>();
    units(range.start)..units(range.end)
}

fn from_utf16(text: &str, range: &Bytes<usize>) -> Bytes<usize> {
    let byte = |units: usize| {
        let mut count = 0;
        for (i, c) in text.char_indices() {
            if count >= units {
                return i;
            }
            count += c.len_utf16();
        }
        text.len()
    };
    byte(range.start)..byte(range.end)
}
