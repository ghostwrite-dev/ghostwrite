//! More than one cursor, as in helix: `C` copies each selection to the line
//! below (`alt-C` above), `s` makes one of each match of a regex in the
//! selection, and `,` goes back to one (`alt-,`: all but the main one). `(`
//! and `)` make another the main one, `alt-minus` and `alt-_` merge them, and
//! `&` lines them up. A key is carried out at each of them, the last in the
//! text first, so that an edit only moves those already done, and all of it is
//! one undo; selections a key makes besides (`3o`, `alt-s`) join them. What a
//! key does once (`:`, `u`, a jump to another file) stays with the main
//! cursor, which is the one the view follows.

use ghostwrite_core::{Key, Mode, Outcome, Range, Registers, movement};
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
        tab.view.others.extend(modal.take_added());
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
        done.extend(modal.take_added().into_iter().map(|added| (added, false)));
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

/// Every selection of `tab` in text order, and which is the main one.
fn in_order(tab: &Tab) -> (Vec<Range>, usize) {
    let mut all: Vec<Range> = tab.view.others.iter().copied().chain([tab.view.selection]).collect();
    all.sort_by_key(|selection| (selection.from(), selection.to()));
    let main = all.iter().position(|selection| *selection == tab.view.selection).unwrap_or(0);
    (all, main)
}

/// `selections` as the tab's, with the one at `main` the main one.
fn set_all(tab: &mut Tab, mut selections: Vec<Range>, main: usize) {
    tab.view.selection = selections.remove(main.min(selections.len() - 1));
    tab.view.others = selections;
}

/// Two selections as one, from the first's start to the last's end (facing
/// back only if both did), as helix's `Range::merge`.
fn merged(a: Range, b: Range) -> Range {
    let (from, to) = (a.from().min(b.from()), a.to().max(b.to()));
    if a.head < a.anchor && b.head < b.anchor { Range::new(to, from) } else { Range::new(from, to) }
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
        let several = self.tab().is_some_and(|tab| !tab.view.others.is_empty());
        match key {
            Key::Char('C') | Key::Alt('C') => {
                let count = self.modal.take_count().unwrap_or(1);
                self.copy_selections(key == Key::Char('C'), count);
            }
            Key::Char(',') => self.one_cursor(),
            Key::Alt(',') => self.drop_main(),
            Key::Char(c @ ('(' | ')')) => {
                let count = self.modal.take_count().unwrap_or(1);
                self.rotate(c == ')', count);
            }
            Key::Alt('-') => self.merge(false),
            Key::Alt('_') => self.merge(true),
            Key::Char('&') => self.align(),
            Key::Char('s') if self.tab().is_some_and(|tab| !tab.doc.read_only()) => self.open_prompt(super::PromptKind::SelectIn, "", window, cx),
            // Esc in select mode ends it, keeping the cursors, as in helix; in normal mode it's back to one.
            Key::Escape if several && self.modal.mode == Mode::Select => self.modal.mode = Mode::Normal,
            Key::Escape if several => self.one_cursor(),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// `C`, `alt-C`: each selection copied to the line below (or above),
    /// `count` times, as helix's `copy_selection_on_line`; the main one's
    /// last copy is the main one.
    fn copy_selections(&mut self, below: bool, count: usize) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let (buffer, width) = (&tab.doc.buffer, tab.doc.indent.width);
        let mut all = vec![];
        for (selection, is_main) in tab.view.others.iter().map(|other| (*other, false)).chain([(tab.view.selection, true)]) {
            let copies = movement::copy_to_lines(buffer, selection, below, count, width);
            let last = copies.len();
            all.push((selection, is_main && last == 0));
            all.extend(copies.into_iter().enumerate().map(|(i, copy)| (copy, is_main && i + 1 == last)));
        }
        let kept = apart(all);
        let main = kept.iter().position(|(_, is_main)| *is_main).unwrap_or(0);
        set_all(tab, kept.into_iter().map(|(selection, _)| selection).collect(), main);
    }

    /// `,`: only the main cursor.
    fn one_cursor(&mut self) {
        if let Some(tab) = self.project.tabs.active_mut() {
            tab.view.others.clear();
        }
    }

    /// `alt-,`: all but the main cursor; the next one is the main one.
    fn drop_main(&mut self) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        if tab.view.others.is_empty() {
            return self.project.say("no other selection to keep");
        }
        let (mut all, main) = in_order(tab);
        all.remove(main);
        set_all(tab, all, main);
    }

    /// `)`, `(`: the main one is the `count`th selection on (or back), round the ends.
    fn rotate(&mut self, forward: bool, count: usize) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let (all, main) = in_order(tab);
        let n = all.len();
        let next = if forward { (main + count) % n } else { (main + n - count % n) % n };
        set_all(tab, all, next);
    }

    /// `alt-minus`: every selection merged into one; `alt-_` (`touching`): only those that touch.
    fn merge(&mut self, touching: bool) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let text = tab.doc.text();
        let (all, main) = in_order(tab);
        let all: Vec<Range> = all.iter().map(|selection| selection.min_width_1(text)).collect();
        if !touching {
            let one = all.iter().copied().reduce(merged).expect("there's always the main one");
            return set_all(tab, vec![one], 0);
        }
        let mut kept: Vec<(Range, bool)> = vec![];
        for (i, selection) in all.into_iter().enumerate() {
            match kept.last_mut() {
                Some((last, last_main)) if last.to() == selection.from() => {
                    *last = merged(*last, selection);
                    *last_main |= i == main;
                }
                _ => kept.push((selection, i == main)),
            }
        }
        let main = kept.iter().position(|(_, is_main)| *is_main).unwrap_or(0);
        set_all(tab, kept.into_iter().map(|(selection, _)| selection).collect(), main);
    }

    /// `&`: the selections lined up in columns, as helix's `align_selections`:
    /// the first selection on each line is in the first column, and so on,
    /// and spaces go before each so that its column starts at the same place.
    fn align(&mut self) {
        let Some(tab) = self.project.tabs.active_mut().filter(|tab| !tab.doc.read_only()) else { return };
        let (all, main) = in_order(tab);
        let (buffer, tab_width) = (&tab.doc.buffer, tab.doc.indent.width);
        let text = buffer.text();
        // Where each one's head is: its line, and its column on screen.
        let mut places = vec![];
        for selection in all.iter().map(|selection| selection.min_width_1(text)) {
            let line = buffer.line_of(selection.head);
            if line != buffer.line_of(selection.anchor) {
                return self.project.say("selections over more than one line can't be lined up");
            }
            places.push((line, ghostwrite_core::display_width(&text[buffer.line_start(line)..selection.head], tab_width)));
        }
        // Which column each is in, and how wide each column is (the most any
        // in it is from the one before on its line).
        let (mut nths, mut widths) = (vec![], vec![]);
        let (mut line, mut nth, mut before) = (usize::MAX, 0, 0);
        for &(on, column) in &places {
            if on != line {
                (line, nth, before) = (on, 0, 0);
            }
            let width = column.saturating_sub(before);
            match widths.get_mut(nth) {
                Some(most) => *most = width.max(*most),
                None => widths.push(width),
            }
            nths.push(nth);
            (nth, before) = (nth + 1, column);
        }
        let stops: Vec<usize> = widths
            .iter()
            .scan(0, |sum, width| {
                *sum += width;
                Some(*sum)
            })
            .collect();
        // The spaces each needs, given those put in before it on its line, from the first on.
        tab.doc.buffer.begin(tab.view.selection);
        let (mut line, mut added, mut shift) = (usize::MAX, 0, 0);
        let mut aligned = vec![];
        for ((&(on, column), &nth), selection) in places.iter().zip(&nths).zip(&all) {
            if on != line {
                (line, added) = (on, 0);
            }
            let spaces = stops[nth].saturating_sub(column + added);
            let at = selection.from() + shift;
            tab.doc.buffer.edit(at..at, &" ".repeat(spaces));
            (added, shift) = (added + spaces, shift + spaces);
            aligned.push(moved(*selection, shift as isize));
        }
        set_all(tab, aligned, main);
        tab.doc.buffer.commit(tab.view.selection);
        tab.view.autoscroll = Some(Autoscroll::Fit);
        tab.sync();
        self.modal.mode = Mode::Normal;
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
