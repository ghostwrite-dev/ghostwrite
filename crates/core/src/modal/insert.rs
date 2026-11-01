//! Insert mode: typing (which arrives through [`Modal::insert`]) and the
//! named keys around it, and the completion menu's keys. Brackets and quotes
//! close themselves, as in helix: typed, the pair goes in with the cursor
//! between; typing the closing one steps over it; Backspace between an empty
//! pair takes both; Enter between brackets opens an indented line.
//!
//! In a language with abbreviations, a backslash starts one that
//! becomes a symbol as it's typed (`\to` → `→`, see [`crate::abbreviation`]).

use super::{Key, MenuKey, Modal, Outcome, Target};
use crate::abbreviation;
use crate::language::Language;
use crate::movement;
use crate::selection::Range;

/// The brackets and quotes that close themselves.
const PAIRS: [(char, char); 6] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];

/// The closing char that typing `open` between `before` and `after` puts
/// in with it, if any: only before the end of a line, a space or a closing
/// bracket (not in the middle of a word); and a quote not after a word
/// char, where it's an apostrophe or a closing quote.
pub(super) fn paired(open: char, before: &str, after: &str) -> Option<char> {
    let &(_, close) = PAIRS.iter().find(|(o, _)| *o == open)?;
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

/// Whether `c` closes a pair.
pub(super) fn closes(c: char) -> bool {
    PAIRS.iter().any(|(_, close)| *close == c)
}

/// The pair the cursor at `at` is in the middle of, empty: `(|)`.
fn between_pair(text: &str, at: usize) -> Option<(char, char)> {
    let (prev, next) = (text[..at].chars().next_back()?, text[at..].chars().next()?);
    PAIRS.iter().copied().find(|&(o, c)| o == prev && c == next)
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
        (t.selection.is_empty() && t.selection.head == end && typed.starts_with('\\') && !typed[1..].contains(char::is_whitespace)).then_some(start)
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
                t.buffer.begin(*t.selection);
                t.buffer.edit(at..at, c.encode_utf8(&mut [0; 4]));
                *t.selection = Range::point(at + c.len_utf8());
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
        t.buffer.begin(*t.selection);
        t.buffer.edit(at..at, "\\");
        *t.selection = Range::point(at + 1);
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
        t.buffer.begin(*t.selection);
        t.buffer.edit(start..end, &text);
        *t.selection = Range::point(start + cursor);
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

    pub(super) fn insert_key(&mut self, key: Key, t: &mut Target) -> Outcome {
        // A named key but Backspace finishes an abbreviation; Tab only that.
        // (Chars arrive as text, through `insert`.)
        if t.abbreviations().is_some() && !matches!(key, Key::Backspace | Key::Char(_)) && self.finish_abbreviation(t) && key == Key::Tab {
            return Outcome::Handled;
        }
        let text = t.buffer.text();
        let at = t.selection.head;
        match key {
            Key::Escape => self.to_normal(t),
            Key::Enter => {
                let indent = t.buffer.indent_at(at).to_string();
                match between_pair(text, at).filter(|(o, _)| matches!(o, '(' | '[' | '{')).filter(|_| t.selection.is_empty()) {
                    // `{|}`: the cursor on a line of its own, indented, and the bracket under.
                    Some(_) => {
                        let inner = format!("\n{indent}{}", t.indent.as_str());
                        t.buffer.begin(*t.selection);
                        t.buffer.edit(at..at, &format!("{inner}\n{indent}"));
                        *t.selection = Range::point(at + inner.len());
                    }
                    None if t.selection.is_empty() => {
                        // The line left keeps no spaces at its end; after an
                        // opening bracket (or Python's `:`) the new one goes in a level.
                        let start = t.buffer.line_start_at(at);
                        let kept = text[start..at].trim_end_matches([' ', '\t']).len() + start;
                        let opens = text[start..kept].ends_with(t.language.map_or(Language::DEFAULT.indent_after, |l| l.indent_after));
                        let deeper = if opens { t.indent.as_str() } else { String::new() };
                        t.buffer.begin(*t.selection);
                        t.buffer.edit(kept..at, &format!("\n{indent}{deeper}"));
                        *t.selection = Range::point(kept + 1 + indent.len() + deeper.len());
                    }
                    None => {
                        self.insert(&format!("\n{indent}"), t);
                    }
                }
            }
            Key::Tab => {
                self.insert(&t.indent.as_str(), t);
            }
            Key::Backspace if t.selection.is_empty() => {
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
                let to = if between_pair(text, at).is_some() { t.buffer.next(at) } else { at };
                t.buffer.edit(from..to, "");
                *t.selection = Range::point(from);
                // Backspacing an abbreviation shortens it, until its backslash goes.
                self.abbreviating = self.abbreviating.filter(|&(start, end)| end == at && from > start).map(|(start, _)| (start, from));
            }
            // The word before the cursor (and the spaces after it), or back to the line's start.
            Key::Ctrl(c @ ('w' | 'u')) if t.selection.is_empty() => {
                let start = t.buffer.line_start_at(at);
                let before = &text[start..at];
                let from = if c == 'u' {
                    start + before.len() - before.trim_start().len()
                } else {
                    let trimmed = before.trim_end();
                    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
                    let last = trimmed.chars().next_back();
                    let kept = match last {
                        Some(ch) if word(ch) => trimmed.trim_end_matches(word),
                        Some(ch) => &trimmed[..trimmed.len() - ch.len_utf8()],
                        None => trimmed,
                    };
                    start + kept.len()
                };
                let from = if from == at { t.buffer.prev(at) } else { from };
                t.buffer.edit(from..at, "");
                *t.selection = Range::point(from);
            }
            Key::Delete if t.selection.is_empty() => {
                let to = t.buffer.next(at);
                t.buffer.edit(at..to, "");
            }
            Key::Backspace | Key::Delete => {
                let range = t.selection.bytes();
                t.buffer.edit(range.clone(), "");
                *t.selection = Range::point(range.start);
            }
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
        if !matches!(key, Key::Up | Key::Down) {
            self.sticky_col = None;
        }
        Outcome::Handled
    }
}
