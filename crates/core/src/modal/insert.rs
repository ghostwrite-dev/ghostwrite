//! Insert mode: typing (which arrives through [`Modal::insert`]) and the
//! named keys around it, and the completion menu's keys.

use super::{Key, MenuKey, Modal, Outcome, Target};
use crate::movement;
use crate::selection::Range;

impl Modal {
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
        let text = t.buffer.text();
        let at = t.selection.head;
        match key {
            Key::Escape => self.to_normal(t),
            Key::Enter => {
                let indent = t.buffer.indent_at(at).to_string();
                self.insert(&format!("\n{indent}"), t);
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
                let (r, col) = movement::move_vertically(t.buffer, Range::point(at), key == Key::Down, 1, false, self.sticky_col);
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
