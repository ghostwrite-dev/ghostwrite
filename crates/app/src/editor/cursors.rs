//! More than one cursor, as in helix: `C` adds one on the line below (`alt-C`
//! above), `s` makes one of each match of a regex in the selection, and `,`
//! goes back to one. A key is carried out at each of them, the last in the text
//! first, so that an edit only moves those already done, and all of it is one
//! undo. What a key does once (`:`, `u`, a jump to another file) stays with the
//! main cursor, which is the one the view follows.

use ghostwrite_core::{Key, Mode, Outcome, Range, Registers};
use gpui::Context;
use regex::Regex;

use super::{Autoscroll, Editor, Tab};

/// Keys that are about the whole file or the editor, not a place in it.
fn once(key: Key) -> bool {
    matches!(key, Key::Char('u' | 'U' | 'q' | 'Q' | '@' | '.') | Key::Ctrl('o' | 'i'))
}

/// `selection` with what happened at some place before it: `by` bytes more or fewer.
fn moved(selection: Range, by: isize) -> Range {
    let at = |byte: usize| byte.saturating_add_signed(by);
    Range::new(at(selection.anchor), at(selection.head))
}

/// `selections` in text order, without any that overlap one before it (the main one stays).
fn apart(mut selections: Vec<(Range, bool)>) -> Vec<(Range, bool)> {
    selections.sort_by_key(|(selection, _)| selection.bytes().start);
    let mut kept: Vec<(Range, bool)> = vec![];
    for (selection, main) in selections {
        match kept.last_mut() {
            Some((last, last_main)) if selection.bytes().start < last.bytes().end.max(last.bytes().start + 1) => {
                if main && !*last_main {
                    (*last, *last_main) = (selection, true);
                }
            }
            _ => kept.push((selection, main)),
        }
    }
    kept
}

/// Carry `key` out at every cursor of `tab`. Each starts from the modal state as it
/// was, since a key can change it (`i` is insert mode, and so isn't typed at the next).
pub(super) fn key_at_each(modal: &mut ghostwrite_core::Modal, key: Key, registers: &mut dyn Registers, tab: &mut Tab, rows: usize) -> Outcome {
    let at_rest = modal.pending_keys().is_empty();
    if tab.view.others.is_empty() || (at_rest && once(key)) {
        let outcome = modal.key(key, registers, &mut tab.target(rows));
        if at_rest && matches!(key, Key::Char('u' | 'U')) {
            tab.view.others.clear();
        }
        return outcome;
    }
    let main = tab.view.selection;
    let before = modal.clone();
    let mut todo: Vec<(Range, bool)> = tab.view.others.iter().map(|other| (*other, false)).chain([(main, true)]).collect();
    todo.sort_by_key(|(selection, _)| std::cmp::Reverse(selection.bytes().start));
    tab.doc.buffer.hold();
    let mut done: Vec<(Range, bool)> = vec![];
    for (selection, is_main) in todo {
        *modal = before.clone();
        tab.view.selection = selection;
        let length = tab.doc.buffer.len();
        let outcome = modal.key(key, registers, &mut tab.target(rows));
        if done.is_empty() && !matches!(outcome, Outcome::Handled) {
            // Not a key for each place (`:`, or text typed, which comes another way): as it was, for the main one.
            // Typing goes on in the same undo step, so nothing's committed here.
            tab.doc.buffer.hold_no_longer();
            *modal = before;
            tab.view.selection = main;
            return modal.key(key, registers, &mut tab.target(rows));
        }
        let by = tab.doc.buffer.len() as isize - length as isize;
        for (later, _) in &mut done {
            *later = moved(*later, by);
        }
        done.push((tab.view.selection, is_main));
    }
    let kept = apart(done);
    let main = kept.iter().find(|(_, is_main)| *is_main).map_or(main, |(selection, _)| *selection);
    tab.view.selection = main;
    tab.view.others = kept.into_iter().filter(|(_, is_main)| !is_main).map(|(selection, _)| selection).collect();
    if modal.mode == Mode::Insert {
        tab.doc.buffer.hold_no_longer();
    } else {
        tab.doc.buffer.release(main);
    }
    Outcome::Handled
}

impl Editor {
    /// Text typed at every cursor: `insert` does it at the one it's given.
    pub(super) fn typed_at_each(&mut self, mut insert: impl FnMut(&mut ghostwrite_core::Modal, &mut Tab)) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let main = tab.view.selection;
        let mut todo: Vec<(Range, bool)> = tab.view.others.iter().map(|other| (*other, false)).chain([(main, true)]).collect();
        todo.sort_by_key(|(selection, _)| std::cmp::Reverse(selection.bytes().start));
        let mut done: Vec<(Range, bool)> = vec![];
        for (selection, is_main) in todo {
            tab.view.selection = selection;
            let length = tab.doc.buffer.len();
            insert(&mut self.modal, tab);
            let by = tab.doc.buffer.len() as isize - length as isize;
            for (later, _) in &mut done {
                *later = moved(*later, by);
            }
            done.push((tab.view.selection, is_main));
        }
        tab.view.selection = done.iter().find(|(_, is_main)| *is_main).map_or(main, |(selection, _)| *selection);
        tab.view.others = done.into_iter().filter(|(_, is_main)| !is_main).map(|(selection, _)| selection).collect();
    }

    /// A key that's about the cursors themselves, if this is one.
    pub(super) fn cursor_key(&mut self, key: Key, window: &mut gpui::Window, cx: &mut Context<Self>) -> bool {
        if !matches!(self.modal.mode, Mode::Normal | Mode::Select) || !self.modal.pending_keys().is_empty() {
            return false;
        }
        match key {
            Key::Char('C') => self.add_cursor(true),
            Key::Alt('C' | 'c') => self.add_cursor(false),
            Key::Char(',') => self.one_cursor(),
            Key::Char('s') if self.tab().is_some_and(|tab| !tab.doc.read_only()) => self.open_prompt(super::PromptKind::SelectIn, "", window, cx),
            Key::Escape if self.tab().is_some_and(|tab| !tab.view.others.is_empty()) => self.one_cursor(),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// One more cursor at the same column of the line past the furthest one.
    fn add_cursor(&mut self, below: bool) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let all = || tab.view.others.iter().chain([&tab.view.selection]).map(|selection| selection.head);
        let Some(from) = (if below { all().max() } else { all().min() }) else { return };
        let buffer = &tab.doc.buffer;
        let line = buffer.line_of(from);
        let column = from - buffer.line_start(line);
        let Some(next) = (if below { Some(line + 1).filter(|next| *next <= buffer.last_line()) } else { line.checked_sub(1) }) else { return };
        let length = buffer.line_end(next) - buffer.line_start(next);
        let text = buffer.text();
        let at = text.floor_char_boundary(buffer.line_start(next) + column.min(length));
        tab.view.others.push(Range::point(at));
    }

    /// `,`: only the main cursor.
    fn one_cursor(&mut self) {
        if let Some(tab) = self.project.tabs.active_mut() {
            tab.view.others.clear();
        }
    }

    /// `s`: a cursor on each match of `typed` in the selection, or in the whole file if there isn't one.
    pub(super) fn select_matches(&mut self, typed: &str) {
        let pattern = match Regex::new(typed).or_else(|_| Regex::new(&regex::escape(typed))) {
            Ok(pattern) => pattern,
            Err(e) => return self.project.say(e.to_string()),
        };
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let text = tab.doc.buffer.text();
        let wide = tab.view.selection.is_wide(text);
        let within = if wide { tab.view.selection.min_width_1(text).bytes() } else { 0..text.len() };
        let mut found: Vec<Range> =
            pattern.find_iter(&text[within.clone()]).filter(|m| !m.is_empty()).map(|m| Range::new(within.start + m.start(), within.start + m.end())).collect();
        let Some(first) = (!found.is_empty()).then(|| found.remove(0)) else {
            return self.project.say(format!("no matches for {typed}"));
        };
        let count = found.len() + 1;
        tab.select(first, Autoscroll::Fit);
        tab.view.others = found;
        self.project.say(format!("{count} cursor{}: , goes back to one", if count == 1 { "" } else { "s" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_that_run_into_each_other_become_one() {
        let at = |byte: usize| Range::point(byte);
        let kept = apart(vec![(at(9), false), (at(3), true), (at(3), false), (at(5), false)]);
        assert_eq!(kept, vec![(at(3), true), (at(5), false), (at(9), false)]);
    }
}
