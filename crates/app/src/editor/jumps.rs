//! The jump list, as in helix: `ctrl-o` goes back to where a jump came
//! from, `ctrl-i` forward again. A jump is any move to another file, or
//! further than a screen in the same one (`gg`, `G`, `gd`, a search, `space j`).

use super::{Autoscroll, DocId, Editor};
use ghostwrite_core::Range;
use gpui::Context;

/// How many places are kept.
const KEPT: usize = 100;

#[derive(Default)]
pub(super) struct Jumps {
    places: Vec<(DocId, usize)>,
    /// Where `ctrl-o` goes from: the place before this one. At the end, nothing's been gone back to.
    at: usize,
}

impl Jumps {
    /// A jump from `from`: it's where `ctrl-o` goes back to (what was ahead is dropped).
    fn jumped(&mut self, from: (DocId, usize)) {
        self.places.truncate(self.at);
        if self.places.last() != Some(&from) {
            self.places.push(from);
        }
        if self.places.len() > KEPT {
            self.places.remove(0);
        }
        self.at = self.places.len();
    }
}

impl Editor {
    /// Where the cursor is: its file, and where in it.
    fn place(&self) -> Option<(DocId, usize)> {
        self.tab().map(|t| (t.doc.id, t.cursor()))
    }

    /// After a key: if it jumped (another file, or more than a screen away), remember where from.
    pub(super) fn note_jump(&mut self, from: Option<(DocId, usize)>) {
        let (Some(from), Some(to)) = (from, self.place()) else { return };
        let far = |tab: &super::Tab| tab.doc.buffer.line_of(from.1).abs_diff(tab.doc.buffer.line_of(to.1)) > self.measured.rows.max(2);
        if from.0 != to.0 || self.tab().is_some_and(far) {
            self.jumps.jumped(from);
        }
    }

    /// The place the cursor is now, before a key that may jump.
    pub(super) fn before_jump(&self) -> Option<(DocId, usize)> {
        self.place()
    }

    /// `ctrl-o` (`back`) and `ctrl-i`.
    pub(super) fn jump(&mut self, back: bool, cx: &mut Context<Self>) {
        let here = self.place();
        if back {
            if self.jumps.at == 0 {
                return self.project.say("no jumps to go back to");
            }
            // Leaving the end: remember it, for ctrl-i to come back to.
            if self.jumps.at == self.jumps.places.len()
                && let Some(here) = here
            {
                self.jumps.places.push(here);
            }
            self.jumps.at -= 1;
        } else {
            if self.jumps.at + 1 >= self.jumps.places.len() {
                return self.project.say("no jumps ahead");
            }
            self.jumps.at += 1;
        }
        let (doc, at) = self.jumps.places[self.jumps.at];
        let Some(ix) = self.project.tabs.index_of_id(doc) else { return self.project.say("that file was closed") };
        // Going through the list isn't jumping: it stays as it is.
        let jumps = std::mem::take(&mut self.jumps);
        self.activate(ix, cx);
        self.jumps = jumps;
        if let Some(tab) = self.tab_mut() {
            let at = at.min(tab.doc.buffer.len());
            let line = tab.doc.buffer.line_of(at);
            tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
        }
        cx.notify();
    }
}

impl Editor {
    /// `zz` `zt` `zb`: scroll so the cursor's line is in the middle, at the top, at the bottom.
    pub(super) fn view_cursor(&mut self, place: ghostwrite_core::Place, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut() else { return };
        let line = tab.doc.buffer.line_of(tab.cursor());
        tab.view.autoscroll = Some(match place {
            ghostwrite_core::Place::Top => Autoscroll::Top,
            ghostwrite_core::Place::Middle => Autoscroll::Center(line..line + 1),
            ghostwrite_core::Place::Bottom => Autoscroll::Bottom,
        });
        cx.notify();
    }

    /// `gt` `gc` `gb`: the cursor to the top, middle or bottom line on screen.
    pub(super) fn screen_line(&mut self, place: ghostwrite_core::Place, cx: &mut Context<Self>) {
        let rows = self.measured.rows.max(1);
        let Some(top) = self.measured.top_line else { return };
        let Some(tab) = self.tab_mut() else { return };
        let margin = (crate::config::get().margin_lines as usize).min(rows / 3);
        let line = match place {
            ghostwrite_core::Place::Top => top + margin,
            ghostwrite_core::Place::Middle => top + rows / 2,
            ghostwrite_core::Place::Bottom => (top + rows).saturating_sub(margin + 1),
        }
        .min(tab.doc.buffer.last_line());
        let at = tab.doc.buffer.first_non_blank_at(tab.doc.buffer.line_start(line));
        tab.select(Range::point(at), Autoscroll::Fit);
        cx.notify();
    }
}
