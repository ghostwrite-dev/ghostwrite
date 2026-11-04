//! Searching the file, as in helix: `/` (forward) and `?` (back) search as
//! it's typed and select the match; Enter keeps it, Esc goes back to where
//! it was. `n` and `N` go on to the next match and the one before, whichever
//! way the search went (in select mode the match is another selection),
//! round the end of the file; `*` makes what's selected the search, for `n`
//! to find. A pattern is a regex, and ignores case unless it has a capital.
//! While searching, every match on screen is marked.

use std::ops::Range as Bytes;

use ghostwrite_core::Range;
use gpui::{Context, Window};
use regex::{Regex, RegexBuilder};

use super::prompt::PromptKind;
use super::{Autoscroll, DocId, Editor};

/// The most matches looked for (more are counted as "many").
const MAX_MATCHES: usize = 100_000;

#[derive(Default)]
pub(super) struct Find {
    pub(super) pattern: String,
    backward: bool,
    regex: Option<Regex>,
    /// Where the selection was when `/` was pressed: Esc goes back there,
    /// and the search starts from it as the pattern grows.
    origin: Option<(DocId, Range)>,
    /// The matches in a version of a document.
    matches: Option<(DocId, u64, Vec<Bytes<usize>>)>,
    /// Marking matches, until a key that isn't about searching.
    pub(super) shown: bool,
}

impl Find {
    fn set(&mut self, pattern: &str, backward: bool) {
        let smart_case = !pattern.chars().any(char::is_uppercase);
        self.pattern = pattern.to_string();
        self.backward = backward;
        self.regex = (!pattern.is_empty()).then(|| RegexBuilder::new(pattern).case_insensitive(smart_case).build().ok()).flatten();
        self.matches = None;
    }
}

impl Find {
    /// What was found is of a text that's gone (the terminal's, read again from scratch).
    pub(super) fn forget(&mut self) {
        self.matches = None;
    }

    /// The matches within a few screens of `tab`'s cursor, to mark (as last
    /// found: [`Editor::matches`] finds them for the version on screen).
    pub(super) fn near(&self, tab: &super::Tab) -> Vec<Bytes<usize>> {
        let Some((_, _, matches)) = self.matches.as_ref().filter(|(d, v, _)| *d == tab.doc.id && *v == tab.doc.buffer.version()) else { return vec![] };
        let line = tab.doc.buffer.line_of(tab.cursor());
        let (from, to) = (tab.doc.buffer.line_start(line.saturating_sub(300)), tab.doc.buffer.line_end(line + 300));
        let first = matches.partition_point(|m| m.end <= from);
        matches[first..].iter().take_while(|m| m.start < to).cloned().collect()
    }
}

impl Editor {
    /// The matches of the search in the active document (cached per version).
    pub(super) fn matches(&mut self) -> &[Bytes<usize>] {
        let Some(tab) = self.keyed() else { return &[] };
        let (id, version) = (tab.doc.id, tab.doc.buffer.version());
        let stale = !self.find.matches.as_ref().is_some_and(|(d, v, _)| *d == id && *v == version);
        if stale {
            let found = self
                .find
                .regex
                .as_ref()
                .map_or(vec![], |re| re.find_iter(tab.doc.text()).filter(|m| !m.is_empty()).take(MAX_MATCHES).map(|m| m.range()).collect());
            self.find.matches = Some((id, version, found));
        }
        self.find.matches.as_ref().map_or(&[], |(_, _, m)| m)
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
        let backward = self.find.backward;
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
        if self.find.regex.is_none() && !self.find.pattern.is_empty() {
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
        let text = tab.doc.text();
        let mut patterns: Vec<String> = vec![];
        for selection in [tab.view.selection].iter().chain(&tab.view.others) {
            let bytes = selection.min_width_1(text).bytes();
            let start = if word_bounds && word_starts(text, bytes.start) { "\\b" } else { "" };
            let end = if word_bounds && word_ends(text, bytes.end) { "\\b" } else { "" };
            let pattern = format!("{start}{}{end}", regex::escape(&text[bytes]));
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
        // As typed, so a capital in it makes it match case.
        let pattern = patterns.join("|");
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
        if self.find.regex.is_none() {
            return self.project.say(if self.find.pattern.is_empty() { "nothing searched for yet: / searches" } else { "not a pattern" });
        }
        let matches = self.matches().to_vec();
        if matches.is_empty() {
            return self.project.say(format!("no match for {}", self.find.pattern));
        }
        let Some(tab) = self.keyed_mut() else { return };
        let mut ix = 0;
        for _ in 0..count {
            let selection = tab.view.selection.min_width_1(tab.doc.text());
            ix = if backward {
                let before = matches.partition_point(|m| m.end <= selection.from());
                before.checked_sub(1).unwrap_or(matches.len() - 1)
            } else {
                let after = matches.partition_point(|m| m.start < selection.to());
                if after == matches.len() { 0 } else { after }
            };
            let found = &matches[ix];
            let range =
                if tab.view.selection.head < tab.view.selection.anchor { Range::new(found.end, found.start) } else { Range::new(found.start, found.end) };
            let others = if extend { [tab.view.others.clone(), vec![tab.view.selection]].concat() } else { std::mem::take(&mut tab.view.others) };
            tab.select(range, Autoscroll::Fit);
            tab.view.others = others.into_iter().filter(|other| other.bytes() != found.clone()).collect();
        }
        self.find.shown = true;
        self.project.say(format!("{}/{} {}", ix + 1, matches.len(), self.find.pattern));
        cx.notify();
    }
}

/// Whether `at` starts a word (helix's `is_at_word_start`).
fn word_starts(text: &str, at: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text[at..].chars().next().is_some_and(word) && !text[..at].chars().next_back().is_some_and(word)
}

/// Whether `at` ends a word: one before, none after (and not the end of the text, as in helix).
fn word_ends(text: &str, at: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text[..at].chars().next_back().is_some_and(word) && text[at..].chars().next().is_some_and(|c| !word(c))
}
