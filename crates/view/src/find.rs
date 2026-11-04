//! Searching the file, as in helix: `/` (forward) and `?` (back) search as
//! it's typed; `n` and `N` go on to the next match and the one before,
//! whichever way the search went; `*` makes what's selected the search. A
//! pattern is a regex, and ignores case unless it has a capital. This is
//! what's searched for and what's found; the app runs the prompt.

use std::ops::Range as Bytes;

use ghostwrite_core::Range;
use regex::{Regex, RegexBuilder};

use crate::document::DocId;
use crate::tabs::Tab;

/// The most matches looked for (more are counted as "many").
const MAX_MATCHES: usize = 100_000;

#[derive(Default)]
pub struct Find {
    pub pattern: String,
    backward: bool,
    regex: Option<Regex>,
    /// Where the selection was when `/` was pressed: Esc goes back there,
    /// and the search starts from it as the pattern grows.
    pub origin: Option<(DocId, Range)>,
    /// The matches in a version of a document.
    matches: Option<(DocId, u64, Vec<Bytes<usize>>)>,
    /// Marking matches, until a key that isn't about searching.
    pub shown: bool,
}

impl Find {
    pub fn set(&mut self, pattern: &str, backward: bool) {
        let smart_case = !pattern.chars().any(char::is_uppercase);
        self.pattern = pattern.to_string();
        self.backward = backward;
        self.regex = (!pattern.is_empty()).then(|| RegexBuilder::new(pattern).case_insensitive(smart_case).build().ok()).flatten();
        self.matches = None;
    }

    /// Whether the search goes back (`?`).
    pub fn backward(&self) -> bool {
        self.backward
    }

    /// Whether what's searched for is a pattern (nothing, or a broken one, isn't).
    pub fn is_pattern(&self) -> bool {
        self.regex.is_some()
    }

    /// What was found is of a text that's gone (the terminal's, read again from scratch).
    pub fn forget(&mut self) {
        self.matches = None;
    }

    /// The matches in `tab`'s text, found again if they're of another version.
    pub fn matches_in(&mut self, tab: &Tab) -> &[Bytes<usize>] {
        let (id, version) = (tab.doc.id, tab.doc.buffer.version());
        let stale = !self.matches.as_ref().is_some_and(|(d, v, _)| *d == id && *v == version);
        if stale {
            let found =
                self.regex.as_ref().map_or(vec![], |re| re.find_iter(tab.doc.text()).filter(|m| !m.is_empty()).take(MAX_MATCHES).map(|m| m.range()).collect());
            self.matches = Some((id, version, found));
        }
        self.matches.as_ref().map_or(&[], |(_, _, m)| m)
    }

    /// The matches within a few screens of `tab`'s cursor, to mark (as last
    /// found: [`Find::matches_in`] finds them for the version on screen).
    pub fn near(&self, tab: &Tab) -> Vec<Bytes<usize>> {
        let Some((_, _, matches)) = self.matches.as_ref().filter(|(d, v, _)| *d == tab.doc.id && *v == tab.doc.buffer.version()) else { return vec![] };
        let line = tab.doc.buffer.line_of(tab.cursor());
        let (from, to) = (tab.doc.buffer.line_start(line.saturating_sub(300)), tab.doc.buffer.line_end(line + 300));
        let first = matches.partition_point(|m| m.end <= from);
        matches[first..].iter().take_while(|m| m.start < to).cloned().collect()
    }
}

/// `*` (`alt-*` without `word_bounds`): what's selected, at every cursor of
/// `tab`, as a search (as it is, not as a pattern), as helix's
/// `search_selection`: where it starts or ends a word, it's matched only there (`\b`).
pub fn selection_pattern(tab: &Tab, word_bounds: bool) -> String {
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
    patterns.join("|")
}

/// `n` (`N`: `backward`): the next match after `tab`'s selection (before it), of
/// `matches`, round the end of the file, as helix's `search_impl` finds it, selected
/// facing as the selection did. With `extend` (select mode), it's another selection,
/// the main one. Which match it is.
pub fn select_next(tab: &mut Tab, matches: &[Bytes<usize>], backward: bool, extend: bool) -> usize {
    let selection = tab.view.selection.min_width_1(tab.doc.text());
    let ix = if backward {
        let before = matches.partition_point(|m| m.end <= selection.from());
        before.checked_sub(1).unwrap_or(matches.len() - 1)
    } else {
        let after = matches.partition_point(|m| m.start < selection.to());
        if after == matches.len() { 0 } else { after }
    };
    let found = &matches[ix];
    let range = if tab.view.selection.head < tab.view.selection.anchor { Range::new(found.end, found.start) } else { Range::new(found.start, found.end) };
    let others = if extend { [tab.view.others.clone(), vec![tab.view.selection]].concat() } else { std::mem::take(&mut tab.view.others) };
    tab.select(range, crate::tabs::Autoscroll::Fit);
    tab.view.others = others.into_iter().filter(|other| other.bytes() != found.clone()).collect();
    ix
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
