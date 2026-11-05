//! More than one cursor, as in helix: `C` copies each selection to the line
//! below (`alt-C` above), `s` makes one of each match of a regex in the
//! selection, and `,` goes back to one (`alt-,`: all but the main one). `(`
//! and `)` make another the main one, `alt-minus` and `alt-_` merge them, and
//! `&` lines them up. A key is carried out at each of them, the last in the
//! text first, so that an edit only moves those already done, and all of it is
//! one undo; selections a key makes besides (`3o`, `alt-s`) join them. What a
//! key does once (`:`, `u`, a jump to another file) stays with the main
//! cursor, which is the one the view follows.

use ghostwrite_core::{Command, Key, Mode, Outcome, Range, Registers, movement};
use regex::Regex;

use crate::tabs::{Autoscroll, Tab};

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
pub fn key_at_each(modal: &mut ghostwrite_core::Modal, key: Key, registers: &mut dyn Registers, tab: &mut Tab, rows: usize) -> Outcome {
    let at_rest = modal.pending_keys().is_empty();
    // What's about the whole file or the editor, not a place in it, is done once.
    let command = modal.command_for(key).filter(|_| at_rest);
    if tab.view.others.is_empty() || command.is_some_and(Command::once) {
        let outcome = modal.key(key, registers, &mut tab.target(rows));
        if matches!(command, Some(Command::Undo | Command::Redo)) {
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

impl Tab {
    /// Text typed at every cursor: `insert` does it at the one it's given.
    pub fn typed_at_each(&mut self, modal: &mut ghostwrite_core::Modal, mut insert: impl FnMut(&mut ghostwrite_core::Modal, &mut Tab)) {
        let main = self.view.selection;
        let mut todo: Vec<(Range, bool)> = self.view.others.iter().map(|other| (*other, false)).chain([(main, true)]).collect();
        todo.sort_by_key(|(selection, _)| std::cmp::Reverse(selection.bytes().start));
        let mut done: Vec<(Range, bool)> = vec![];
        for (selection, is_main) in todo {
            self.view.selection = selection;
            let length = self.doc.buffer.len();
            insert(modal, self);
            let by = self.doc.buffer.len() as isize - length as isize;
            for (later, _) in &mut done {
                *later = moved(*later, by);
            }
            done.push((self.view.selection, is_main));
        }
        self.view.selection = done.iter().find(|(_, is_main)| *is_main).map_or(main, |(selection, _)| *selection);
        self.view.others = done.into_iter().filter(|(_, is_main)| !is_main).map(|(selection, _)| selection).collect();
    }

    /// `C`, `alt-C`: each selection copied to the line below (or above),
    /// `count` times, as helix's `copy_selection_on_line`; the main one's
    /// last copy is the main one.
    pub fn copy_selections(&mut self, below: bool, count: usize) {
        let (buffer, width) = (&self.doc.buffer, self.doc.indent.width);
        let mut all = vec![];
        for (selection, is_main) in self.view.others.iter().map(|other| (*other, false)).chain([(self.view.selection, true)]) {
            let copies = movement::copy_to_lines(buffer, selection, below, count, width);
            let last = copies.len();
            all.push((selection, is_main && last == 0));
            all.extend(copies.into_iter().enumerate().map(|(i, copy)| (copy, is_main && i + 1 == last)));
        }
        let kept = apart(all);
        let main = kept.iter().position(|(_, is_main)| *is_main).unwrap_or(0);
        set_all(self, kept.into_iter().map(|(selection, _)| selection).collect(), main);
    }

    /// `,`: only the main cursor.
    pub fn one_cursor(&mut self) {
        self.view.others.clear();
    }

    /// `alt-,`: all but the main cursor; the next one is the main one.
    pub fn drop_main(&mut self) -> Result<(), &'static str> {
        if self.view.others.is_empty() {
            return Err("no other selection to keep");
        }
        let (mut all, main) = in_order(self);
        all.remove(main);
        set_all(self, all, main);
        Ok(())
    }

    /// `)`, `(`: the main one is the `count`th selection on (or back), round the ends.
    pub fn rotate(&mut self, forward: bool, count: usize) {
        let (all, main) = in_order(self);
        let n = all.len();
        let next = if forward { (main + count) % n } else { (main + n - count % n) % n };
        set_all(self, all, next);
    }

    /// `alt-minus`: every selection merged into one; `alt-_` (`touching`): only those that touch.
    pub fn merge(&mut self, touching: bool) {
        let text = &self.doc.buffer;
        let (all, main) = in_order(self);
        let all: Vec<Range> = all.iter().map(|selection| selection.min_width_1(text)).collect();
        if !touching {
            let one = all.iter().copied().reduce(merged).expect("there's always the main one");
            return set_all(self, vec![one], 0);
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
        set_all(self, kept.into_iter().map(|(selection, _)| selection).collect(), main);
    }

    /// `&`: the selections lined up in columns, as helix's `align_selections`:
    /// the first selection on each line is in the first column, and so on,
    /// and spaces go before each so that its column starts at the same place.
    pub fn align(&mut self) -> Result<(), &'static str> {
        let tab = self;
        let (all, main) = in_order(tab);
        let (buffer, tab_width) = (&tab.doc.buffer, tab.doc.indent.width);
        let text = buffer;
        // Where each one's head is: its line, and its column on screen.
        let mut places = vec![];
        for selection in all.iter().map(|selection| selection.min_width_1(text)) {
            let line = buffer.line_of(selection.head);
            if line != buffer.line_of(selection.anchor) {
                return Err("selections over more than one line can't be lined up");
            }
            places.push((line, ghostwrite_core::display_width(&buffer.slice(buffer.line_start(line)..selection.head), tab_width)));
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
        Ok(())
    }

    /// `s`: a cursor on each match of `pattern` in the selection, or in the whole
    /// file if there isn't one. How many, if there are any.
    pub fn select_matches(&mut self, pattern: &Regex) -> Option<usize> {
        // A pattern reads a string: the whole text, made once for this version.
        let text = self.doc.buffer.whole_text();
        let wide = self.view.selection.is_wide(text);
        let within = if wide { self.view.selection.min_width_1(text).bytes() } else { 0..text.len() };
        let mut found: Vec<Range> =
            pattern.find_iter(&text[within.clone()]).filter(|m| !m.is_empty()).map(|m| Range::new(within.start + m.start(), within.start + m.end())).collect();
        let first = (!found.is_empty()).then(|| found.remove(0))?;
        let count = found.len() + 1;
        self.select(first, Autoscroll::Fit);
        self.view.others = found;
        Some(count)
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
