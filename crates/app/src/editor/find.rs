//! Searching the file, as in helix: `/` (forward) and `?` (back) search as
//! it's typed and select the match; Enter keeps it, Esc goes back to where
//! it was. `n` and `N` go on to the next match and the one before, whichever
//! way the search went (in select mode the match is another selection),
//! round the end of the file; `*` makes what's selected the search, for `n`
//! to find. A pattern is a regex, and ignores case unless it has a capital.
//! While searching, every match on screen is marked.

use std::ops::Range as Bytes;

use gpui::{Context, Window};

pub(super) use ghostwrite_state::find::Find;
use ghostwrite_state::find::{select_next, selection_pattern};

use super::Autoscroll;
use super::Editor;
use super::prompt::PromptKind;

impl Editor {
    /// The matches of the search in the active document (cached per version).
    pub(super) fn matches(&mut self) -> &[Bytes<usize>] {
        let reading = self.reading();
        let Some(tab) = self.terminal.reading.as_ref().filter(|_| reading).map(|reading| &reading.tab).or_else(|| self.project.tab()) else { return &[] };
        self.find.matches_in(tab)
    }

    /// `/`, `?`: the search prompt, from the selection as it is.
    pub(super) fn start_find(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.keyed() else { return };
        self.find.origin = Some((tab.doc.id, tab.view.selection));
        self.find.set("", backward);
        self.find.shown = true;
        self.open_prompt(PromptKind::Find { backward }, "", window, cx);
    }

    /// The pattern as typed so far: select its first match from where the search began.
    pub(super) fn find_typed(&mut self, pattern: &str, cx: &mut Context<Self>) {
        let backward = self.find.backward();
        self.find.set(pattern, backward);
        let Some((doc, origin)) = self.find.origin else { return };
        if let Some(tab) = self.keyed_by_id_mut(doc) {
            tab.select(origin, Autoscroll::Fit);
        }
        if !pattern.is_empty() {
            self.find_next(backward, 1, false, cx);
        }
        cx.notify();
    }

    /// Esc in the search prompt: back to where it began.
    pub(super) fn cancel_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((doc, origin)) = self.find.origin.take()
            && let Some(tab) = self.keyed_by_id_mut(doc)
        {
            tab.select(origin, Autoscroll::Fit);
        }
        self.find.shown = false;
        self.close_prompt(window, cx);
    }

    /// Enter in the search prompt: keep the match.
    pub(super) fn end_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.origin = None;
        self.close_prompt(window, cx);
        if !self.find.is_pattern() && !self.find.pattern.is_empty() {
            self.project.say(format!("not a pattern: {}", self.find.pattern));
        } else if self.matches().is_empty() && !self.find.pattern.is_empty() {
            self.project.say(format!("no match for {}", self.find.pattern));
        }
    }

    /// `*` (`alt-*` without `word_bounds`): what's selected, at every
    /// cursor, is the search (as it is, not as a pattern), as helix's
    /// `search_selection`: where it starts or ends a word, it's matched only
    /// there (`\b`). Nothing's found yet: `n` does that.
    pub(super) fn find_selection(&mut self, word_bounds: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.keyed() else { return };
        // As typed, so a capital in it makes it match case.
        let pattern = selection_pattern(tab, word_bounds);
        self.find.set(&pattern, false);
        self.find.shown = true;
        self.project.say(format!("searching for {pattern}: n finds it"));
        cx.notify();
    }

    /// `n` (`N`: `backward`): the `count`th match after the selection (before
    /// it), round the end of the file, as helix's `search_impl` finds it, and
    /// facing as the selection did. With `extend` (select mode), it's another
    /// selection, the main one.
    pub(super) fn find_next(&mut self, backward: bool, count: usize, extend: bool, cx: &mut Context<Self>) {
        if !self.find.is_pattern() {
            return self.project.say(if self.find.pattern.is_empty() { "nothing searched for yet: / searches" } else { "not a pattern" });
        }
        let matches = self.matches().to_vec();
        if matches.is_empty() {
            return self.project.say(format!("no match for {}", self.find.pattern));
        }
        let Some(tab) = self.keyed_mut() else { return };
        let mut ix = 0;
        for _ in 0..count {
            ix = select_next(tab, &matches, backward, extend);
        }
        self.find.shown = true;
        self.project.say(format!("{}/{} {}", ix + 1, matches.len(), self.find.pattern));
        cx.notify();
    }
}
