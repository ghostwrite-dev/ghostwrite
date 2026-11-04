//! Insert mode: typing (which arrives through [`Modal::insert`]) and the
//! named keys around it, and the completion menu's keys. Brackets and quotes
//! close themselves, as in helix: typed, the pair goes in with the cursor
//! between; typing the closing one steps over it; Backspace between an empty
//! pair takes both; Enter between brackets opens an indented line, and in a
//! line comment goes on with the comment.
//!
//! As in helix, the selection stays while typing (after `a` it's the text
//! appended to): everything happens at its head, the bar cursor, and its
//! other end stays on its text.
//!
//! In a language with abbreviations, a backslash starts one that
//! becomes a symbol as it's typed (`\to` → `→`, see [`crate::abbreviation`]).

use super::{Key, MenuKey, Modal, Outcome, Pending, Registers, Target};
use crate::abbreviation;
use crate::language::Language;
use crate::movement::{self, Word};
use crate::selection::Range;

/// The brackets and quotes that close themselves (a language can leave some
/// out and add its own: see [`Target::pairs`](super::Target)).
pub(super) const PAIRS: [(char, char); 6] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];

/// The closing char that typing `open` between `before` and `after` puts
/// in with it, if any of `pairs`: only before the end of a line, a space or
/// a closing bracket (not in the middle of a word); and a quote not after a
/// word char, where it's an apostrophe or a closing quote.
pub(super) fn paired(open: char, before: &str, after: &str, pairs: &[(char, char)]) -> Option<char> {
    let &(_, close) = pairs.iter().find(|(o, _)| *o == open)?;
    let next = after.chars().next();
    if !next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';')) {
        return None;
    }
    let prev = before.chars().next_back();
    if open == close && prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == open) {
        return None;
    }
    Some(close)
}

/// Replace `range` (at the head) with `text`, the head going to `head`. The
/// selection's other end stays on its text, as helix maps it: before the
/// change, where it is; after, moved along; in what was replaced, at its start.
pub(super) fn put(t: &mut Target, range: std::ops::Range<usize>, text: &str, head: usize) {
    let (sel, removed) = (*t.selection, range.end - range.start);
    let anchor = match sel.anchor {
        _ if sel.is_empty() => head,
        a if a < range.start || (a == range.start && a < sel.head) => a,
        a if a >= range.end => a + text.len() - removed,
        _ => range.start,
    };
    t.buffer.begin(sel);
    t.buffer.edit(range, text);
    *t.selection = Range::new(anchor, head);
}

/// The head to `at` without an edit; a cursor alone stays one.
pub(super) fn move_head(t: &mut Target, at: usize) {
    let sel = *t.selection;
    *t.selection = if sel.is_empty() { Range::point(at) } else { Range::new(sel.anchor, at) };
}

/// The pair the cursor at `at` is in the middle of, empty: `(|)`.
fn between_pair(t: &Target, at: usize) -> Option<(char, char)> {
    let text = t.buffer.text();
    let (prev, next) = (text[..at].chars().next_back()?, text[at..].chars().next()?);
    t.pairs().into_iter().find(|&(o, c)| o == prev && c == next)
}

impl Modal {
    /// The abbreviation being typed, from its backslash to the cursor, to
    /// mark it while it is.
    pub fn abbreviation(&self) -> Option<std::ops::Range<usize>> {
        self.abbreviating.filter(|_| self.mode == super::Mode::Insert).map(|(start, end)| start..end)
    }

    /// Where the abbreviation being typed starts, if the cursor is still at its end.
    fn abbreviation_start(&self, t: &Target) -> Option<usize> {
        let (start, end) = self.abbreviating?;
        let typed = t.buffer.text().get(start..end)?;
        (t.selection.head == end && typed.starts_with('\\') && !typed[1..].contains(char::is_whitespace)).then_some(start)
    }

    /// `c` typed with abbreviations on: `true` if it went into one. It goes
    /// on the one being typed if that can still become an abbreviation
    /// (which, once no longer one starts with it, becomes its symbol);
    /// otherwise that one's finished, and a backslash starts another.
    pub(super) fn abbreviate(&mut self, c: char, t: &mut Target) -> bool {
        let Some(table) = t.abbreviations() else { return false };
        if let Some(start) = self.abbreviation_start(t) {
            let at = t.selection.head;
            let typed = format!("{}{c}", &t.buffer.text()[start + 1..at]);
            if table.extends(&typed) {
                put(t, at..at, c.encode_utf8(&mut [0; 4]), at + c.len_utf8());
                self.abbreviating = Some((start, at + c.len_utf8()));
                if let Some(symbol) = table.eager(&typed) {
                    self.replace_abbreviation(symbol, t);
                }
                return true;
            }
            self.finish_abbreviation(t);
        }
        if c != '\\' {
            return false;
        }
        let at = t.selection.head;
        put(t, at..at, "\\", at + 1);
        self.abbreviating = Some((at, at + 1));
        true
    }

    /// Turn the abbreviation being typed into its symbol, if it stands for
    /// one, and stop typing it. `true` if it changed.
    pub(super) fn finish_abbreviation(&mut self, t: &mut Target) -> bool {
        let table = t.abbreviations();
        let symbol = self.abbreviation_start(t).zip(table).and_then(|(start, table)| table.finished(&t.buffer.text()[start + 1..t.selection.head]));
        match symbol {
            Some(symbol) => self.replace_abbreviation(symbol, t),
            None => self.abbreviating = None,
        }
        symbol.is_some()
    }

    fn replace_abbreviation(&mut self, symbol: &str, t: &mut Target) {
        let Some((start, end)) = self.abbreviating.take() else { return };
        let (text, cursor) = abbreviation::place(symbol);
        put(t, start..end, &text, start + cursor);
    }

    /// The completion menu's keys: ↑↓ or ctrl-p/n choose, Enter or Tab take it.
    pub fn menu_key(&self, key: Key) -> Option<MenuKey> {
        Some(match key {
            Key::Down | Key::Ctrl('n') => MenuKey::Next,
            Key::Up | Key::Ctrl('p') => MenuKey::Prev,
            Key::Enter | Key::Tab => MenuKey::Accept,
            Key::Escape => MenuKey::Close,
            _ => return None,
        })
    }

    pub(super) fn insert_key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        if self.pending.take() == Some(Pending::InsertRegister) {
            // `ctrl-r` and a register: what's in it, typed out (`"` is the default one).
            if let Key::Char(name) = key {
                let text = regs.yanked((name != '"').then_some(name));
                let at = t.selection.head;
                put(t, at..at, &text, at + text.len());
            }
            return Outcome::Handled;
        }
        // Helix's other names for keys.
        let key = match key {
            Key::Ctrl('h') => Key::Backspace,
            Key::Ctrl('d') => Key::Delete,
            Key::Ctrl('j') => Key::Enter,
            key => key,
        };
        // A named key but Backspace finishes an abbreviation; Tab only that.
        // (Chars arrive as text, through `insert`.)
        if t.abbreviations().is_some() && !matches!(key, Key::Backspace | Key::Char(_)) && self.finish_abbreviation(t) && key == Key::Tab {
            return Outcome::Handled;
        }
        let text = t.buffer.text();
        let at = t.selection.head;
        match key {
            Key::Escape => self.to_normal(t),
            Key::Enter => newline(t),
            // To the next indent stop (helix's smart Tab needs a syntax tree to do more).
            Key::Tab | Key::BackTab => {
                let unit = t.indent.as_str();
                let column = text[t.buffer.line_start_at(at)..at].chars().count();
                let tab = if t.indent.hard_tabs { unit.as_str() } else { &unit[column % t.indent.width.max(1)..] };
                put(t, at..at, tab, at + tab.len());
            }
            Key::Backspace => {
                let start = t.buffer.line_start_at(at);
                let before = &text[start..at];
                // In leading spaces: back to the previous indent stop.
                let from = if !before.is_empty() && before.bytes().all(|b| b == b' ') && !t.indent.hard_tabs {
                    let width = t.indent.width.max(1);
                    start + (before.len() - 1) / width * width
                } else {
                    t.buffer.prev(at)
                };
                // An empty pair goes with its opening half.
                let to = if between_pair(t, at).is_some() { t.buffer.next(at) } else { at };
                put(t, from..to, "", from);
                // Backspacing an abbreviation shortens it, until its backslash goes.
                self.abbreviating = self.abbreviating.filter(|&(start, end)| end == at && from > start).map(|(start, _)| (start, from));
            }
            Key::Delete => {
                let to = t.buffer.next(at);
                put(t, at..to, "", at);
            }
            // `ctrl-w`, `alt-backspace`: back to where the word before starts (a run of punctuation is one).
            Key::Ctrl('w') => {
                let from = movement::word_move(t.buffer, Range::point(at), 1, Word::PrevStart).from().min(at);
                put(t, from..at, "", from);
            }
            // `alt-d`: on to where this word ends.
            Key::Alt('d') => {
                let to = movement::word_move(t.buffer, Range::point(at), 1, Word::NextEnd).to().max(at);
                put(t, at..to, "", at);
            }
            // `ctrl-u`: back to the first non-blank (or the line's start if
            // already there); at the start, the line joins the one before.
            Key::Ctrl('u') => {
                let line = t.buffer.line_of(at);
                let (start, first) = (t.buffer.line_start(line), t.buffer.first_non_blank_at(at));
                let from = match () {
                    _ if at == start && line > 0 => t.buffer.line_end(line - 1),
                    _ if first < at => first,
                    _ => start,
                };
                put(t, from..at, "", from);
            }
            // `ctrl-k`: on to the line's end; at the end, its line break.
            Key::Ctrl('k') => {
                let end = t.buffer.line_end_at(at);
                let to = if at == end { t.buffer.next(at) } else { end };
                put(t, at..to, "", at);
            }
            Key::Ctrl('r') => self.pending = Some(Pending::InsertRegister),
            Key::Left => *t.selection = Range::point(t.buffer.prev(at)),
            Key::Right => *t.selection = Range::point(t.buffer.next(at)),
            Key::Home => *t.selection = Range::point(t.buffer.line_start_at(at)),
            Key::End => *t.selection = Range::point(t.buffer.line_end_at(at)),
            Key::Up | Key::Down => {
                let (r, col) = movement::move_vertically(t.buffer, Range::point(at), key == Key::Down, 1, false, self.sticky_col, t.indent.width);
                *t.selection = r;
                self.sticky_col = Some(col);
                return Outcome::Handled;
            }
            _ => return Outcome::Ignored,
        }
        self.sticky_col = None;
        Outcome::Handled
    }
}

/// Enter: a new line, as helix opens one. Spaces before the cursor go; the
/// new line has this one's indent (a level more after an opening bracket, or
/// Python's `:`), and goes on with a line comment this one starts with.
/// Between brackets the cursor gets a line of its own. With only indent
/// before the cursor, the line moves down as it is.
fn newline(t: &mut Target) {
    let text = t.buffer.text();
    let at = t.selection.head;
    let line = t.buffer.line_of(at);
    let start = t.buffer.line_start(line);
    if text[start..at].trim().is_empty() {
        return put(t, start..start, "\n", at + 1);
    }
    let kept = start + text[start..at].trim_end_matches([' ', '\t']).len();
    let indent = t.buffer.indent_at(at);
    let language = t.language.unwrap_or(&Language::DEFAULT);
    let new = if let Some(leader) = language.comment_leader(t.buffer.line(line)) {
        format!("\n{indent}{leader} ")
    } else if between_pair(t, at).is_some_and(|(open, _)| matches!(open, '(' | '[' | '{')) {
        // `{|}`: the cursor on a line of its own, indented, and the bracket under.
        let inner = format!("\n{indent}{}", t.indent.as_str());
        let new = format!("{inner}\n{indent}");
        return put(t, kept..at, &new, kept + inner.len());
    } else {
        let deeper = if text[start..kept].ends_with(language.indent_after) { t.indent.as_str() } else { String::new() };
        format!("\n{indent}{deeper}")
    };
    put(t, kept..at, &new, kept + new.len());
}
