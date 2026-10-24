//! Normal and select mode: motions, selections and edits. AI and tour mode
//! use the same motions to look around, but every edit is refused.

use super::{Effect, Key, Modal, Mode, Outcome, Pending, Registers, Target};
use crate::indent::unindent;
use crate::movement::{self, Word, line_last_char};
use crate::selection::Range;
use crate::textobject;

impl Modal {
    pub(super) fn normal_key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        let sel = *t.selection;
        let extend = self.mode == Mode::Select;

        if let Some(pending) = self.pending.take() {
            let count = self.count.take().unwrap_or(1);
            return self.pending_key(pending, key, count, sel, extend, regs, t);
        }
        if let Key::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
        {
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(c as usize - '0' as usize));
            return Outcome::Handled;
        }
        let explicit_count = self.count.take();
        let count = explicit_count.unwrap_or(1);
        let b = &*t.buffer;
        let text = b.text();
        let mut sticky = None;

        let moved = match key {
            Key::Char('h') | Key::Left => Some(movement::move_horizontally(b, sel, false, count, extend)),
            Key::Char('l') | Key::Right => Some(movement::move_horizontally(b, sel, true, count, extend)),
            Key::Char('j' | 'k') | Key::Down | Key::Up => {
                let down = matches!(key, Key::Char('j') | Key::Down);
                let (r, col) = movement::move_vertically(b, sel, down, count, extend, self.sticky_col);
                sticky = Some(col);
                Some(r)
            }
            Key::Ctrl('d' | 'u' | 'f' | 'b') | Key::PageDown | Key::PageUp => {
                let half = matches!(key, Key::Ctrl('d' | 'u'));
                let down = matches!(key, Key::Ctrl('d' | 'f') | Key::PageDown);
                let rows = if half { t.page_rows.max(2) / 2 } else { t.page_rows.max(2) };
                let (r, col) = movement::move_vertically(b, sel, down, rows * count, extend, self.sticky_col);
                sticky = Some(col);
                Some(r)
            }
            Key::Char(c @ ('w' | 'b' | 'e' | 'W' | 'B' | 'E')) => {
                let target = match c {
                    'w' => Word::NextStart,
                    'b' => Word::PrevStart,
                    'e' => Word::NextEnd,
                    'W' => Word::NextLongStart,
                    'B' => Word::PrevLongStart,
                    _ => Word::NextLongEnd,
                };
                let r = movement::word_move(b, sel, count, target);
                Some(if extend { sel.put_cursor(text, r.cursor(text), true) } else { r })
            }
            Key::Char('G') => {
                let line = explicit_count.map_or(b.last_line(), |n| n.saturating_sub(1).min(b.last_line()));
                Some(sel.put_cursor(text, b.line_start(line), extend))
            }
            Key::Home => Some(sel.put_cursor(text, b.line_start_at(sel.cursor(text)), extend)),
            Key::End => Some(sel.put_cursor(text, line_last_char(b, sel.cursor(text)), extend)),
            Key::Char('x') => Some(movement::extend_line_below(b, sel, count)),
            Key::Char('%') => Some(Range::new(0, b.len())),
            Key::Char(';') => Some(Range::point(sel.cursor(text))),
            Key::Alt(';') => Some(sel.flip()),
            _ => None,
        };
        if let Some(r) = moved {
            self.sticky_col = sticky;
            *t.selection = r;
            return Outcome::Handled;
        }
        self.sticky_col = None;

        let target = sel.min_width_1(text);
        match key {
            Key::Char(':') => return Outcome::Effect(Effect::CommandLine),
            Key::Char('?') => return Outcome::Effect(Effect::Cheatsheet),
            Key::Char('g') => self.pending = Some(Pending::Goto),
            Key::Char(' ') => self.pending = Some(Pending::Space),
            Key::Char('r') => self.pending = Some(Pending::Replace),
            Key::Char('m') => self.pending = Some(Pending::Match),
            // `f` is the file tree: in and out of tree mode.
            Key::Char('f') if self.mode.reviewing() => {}
            Key::Char('f') => {
                self.to_tree();
                return Outcome::Effect(Effect::FocusTree);
            }
            Key::Char(c @ ('t' | 'F' | 'T')) => {
                self.pending = Some(Pending::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') });
            }
            Key::Char('v') if !self.mode.reviewing() => self.mode = if self.mode == Mode::Select { Mode::Normal } else { Mode::Select },
            Key::Escape => self.mode = Mode::Normal,
            Key::Char('i') => self.enter_insert(target.from(), t),
            Key::Char('a') => self.enter_insert(target.to(), t),
            Key::Char('I') => {
                let at = b.first_non_blank_at(sel.from());
                self.enter_insert(at, t);
            }
            Key::Char('A') => {
                let at = b.line_end_at(b.prev(target.to()).max(target.from()));
                self.enter_insert(at, t);
            }
            Key::Char('o') => {
                let at = b.line_end_at(b.prev(target.to()));
                let text = format!("\n{}", b.indent_at(at));
                let caret = at + text.len();
                self.edit(t, at..at, &text, Range::point(caret), Mode::Insert);
            }
            Key::Char('O') => {
                let at = b.line_start_at(target.from());
                let indent = b.indent_at(at).to_string();
                let caret = at + indent.len();
                self.edit(t, at..at, &format!("{indent}\n"), Range::point(caret), Mode::Insert);
            }
            Key::Char(c @ ('d' | 'c')) => {
                regs.yank(b.slice(target.bytes()).to_string());
                let mode = if c == 'c' { Mode::Insert } else { Mode::Normal };
                self.edit(t, target.bytes(), "", Range::point(target.from()), mode);
            }
            Key::Char('y') => {
                regs.yank(b.slice(target.bytes()).to_string());
                if !self.mode.reviewing() {
                    self.mode = Mode::Normal;
                }
            }
            Key::Char(c @ ('p' | 'P')) => {
                let text = regs.yanked();
                self.paste(t, target, &text, c == 'p', count);
            }
            Key::Char('u') if !self.mode.reviewing() => {
                if let Some(r) = t.buffer.undo() {
                    *t.selection = r.clamp(t.buffer.len());
                }
            }
            Key::Char('U') if !self.mode.reviewing() => {
                if let Some(r) = t.buffer.redo() {
                    *t.selection = r.clamp(t.buffer.len());
                }
            }
            Key::Char(c @ ('>' | '<')) => {
                let start = b.line_start_at(target.from());
                let end = b.line_end_at(b.prev(target.to()).max(target.from()));
                let unit = t.indent.as_str();
                let shifted: Vec<String> = b
                    .slice(start..end)
                    .split('\n')
                    .map(|line| match c {
                        '>' if !line.is_empty() => format!("{}{line}", unit.repeat(count)),
                        '>' => line.to_string(),
                        _ => unindent(line, count, t.indent.width.max(1)).to_string(),
                    })
                    .collect();
                let shifted = shifted.join("\n");
                let after = Range::new(start, start + shifted.len());
                self.edit(t, start..end, &shifted, after, Mode::Normal);
            }
            Key::Char('~') => {
                let swapped: String = b
                    .slice(target.bytes())
                    .chars()
                    .flat_map(|ch| -> Vec<char> {
                        if ch.is_uppercase() { ch.to_lowercase().collect() } else { ch.to_uppercase().collect() }
                    })
                    .collect();
                let after = same_shape(sel, target.from(), swapped.len());
                self.edit(t, target.bytes(), &swapped, after, self.mode);
            }
            _ => {}
        }
        Outcome::Handled
    }

    #[allow(clippy::too_many_arguments)]
    fn pending_key(&mut self, pending: Pending, key: Key, count: usize, sel: Range, extend: bool, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        let Key::Char(c) = key else { return Outcome::Handled };
        let b = &*t.buffer;
        let text = b.text();
        let cursor = sel.cursor(text);
        let to = match (pending, c) {
            (Pending::Goto, 'g') => Some(b.line_start(count - 1)),
            (Pending::Goto, 'e') => Some(b.line_start(b.last_line())),
            (Pending::Goto, 'h') => Some(b.line_start_at(cursor)),
            (Pending::Goto, 'l') => Some(line_last_char(b, cursor)),
            (Pending::Goto, 's') => Some(b.first_non_blank_at(cursor)),
            (Pending::Goto, 'n') => return Outcome::Effect(Effect::NextBuffer),
            (Pending::Goto, 'p') => return Outcome::Effect(Effect::PrevBuffer),
            (Pending::Goto, 'd') => return Outcome::Effect(Effect::GotoDefinition),
            (Pending::Find { forward, till }, ch) => {
                if let Some(r) = movement::find_char(b, sel, ch, forward, till, count, extend) {
                    *t.selection = r;
                }
                return Outcome::Handled;
            }
            (Pending::Match, 'm') => {
                if let Some(at) = textobject::matching_bracket(b, cursor) {
                    *t.selection = sel.put_cursor(text, at, extend);
                }
                return Outcome::Handled;
            }
            (Pending::Match, next @ ('s' | 'd' | 'r' | 'a' | 'i')) => {
                self.pending = Some(match next {
                    's' => Pending::SurroundAdd,
                    'd' => Pending::SurroundDelete,
                    'r' => Pending::SurroundReplace,
                    'a' => Pending::Object { around: true },
                    _ => Pending::Object { around: false },
                });
                return Outcome::Handled;
            }
            (Pending::SurroundAdd, ch) => {
                let (open, close) = textobject::pair_of(ch);
                let target = sel.min_width_1(text);
                let wrapped = format!("{open}{}{close}", b.slice(target.bytes()));
                let after = Range::new(target.from(), target.from() + wrapped.len());
                self.edit(t, target.bytes(), &wrapped, after, Mode::Normal);
                return Outcome::Handled;
            }
            (Pending::SurroundReplace, from) => {
                self.pending = Some(Pending::SurroundReplaceWith(from));
                return Outcome::Handled;
            }
            (pending @ (Pending::SurroundDelete | Pending::SurroundReplaceWith(_)), ch) => {
                let target = sel.min_width_1(text);
                let which = if let Pending::SurroundReplaceWith(from) = pending { from } else { ch };
                let found = if which == 'm' {
                    textobject::nearest_pair(b, target.from(), target.to())
                } else {
                    textobject::surrounding(b, target.from(), target.to(), textobject::pair_of(which))
                };
                let Some((left, right)) = found else { return Outcome::Handled };
                let inner = b.slice(b.next(left)..right).to_string();
                let open_len = b.next(left) - left;
                let (replacement, after) = match pending {
                    Pending::SurroundReplaceWith(_) => {
                        let (open, close) = textobject::pair_of(ch);
                        (format!("{open}{inner}{close}"), sel)
                    }
                    _ => (inner, Range::new(target.from().saturating_sub(open_len).max(left), target.to().saturating_sub(open_len).max(left))),
                };
                let after = after.clamp(b.len() - (b.next(right) - left) + replacement.len());
                self.edit(t, left..b.next(right), &replacement, after, self.mode);
                return Outcome::Handled;
            }
            (Pending::Object { around }, ch) => {
                if let Some(r) = textobject::text_object(b, sel, ch, around) {
                    *t.selection = r;
                }
                return Outcome::Handled;
            }
            (Pending::Replace, ch) => {
                let target = sel.min_width_1(text);
                let replaced: String = b.slice(target.bytes()).chars().map(|o| if o == '\n' { o } else { ch }).collect();
                let after = same_shape(sel, target.from(), replaced.len());
                self.edit(t, target.bytes(), &replaced, after, self.mode);
                return Outcome::Handled;
            }
            // Starting something else waits until the review is done; but a
            // tour can be paused to ask Claude something.
            (Pending::Space, 'i') if self.mode == Mode::Ai => return Outcome::Handled,
            (Pending::Space, 'e' | 'k' | 'd' | '/') if self.mode.reviewing() => return Outcome::Handled,
            (Pending::Space, 'e') => {
                self.to_tree();
                return Outcome::Effect(Effect::FocusTree);
            }
            (Pending::Space, 'i') => return Outcome::Effect(Effect::Assist),
            (Pending::Space, 'k') => return Outcome::Effect(Effect::Hover),
            (Pending::Space, 'd') => return Outcome::Effect(Effect::Diagnostics),
            (Pending::Space, '/') => return Outcome::Effect(Effect::Search),
            (Pending::Space, 'y') => {
                regs.set_clipboard(b.slice(sel.min_width_1(text).bytes()).to_string());
                return Outcome::Handled;
            }
            (Pending::Space, p @ ('p' | 'P')) => {
                let pasted = regs.clipboard().unwrap_or_default();
                self.paste(t, sel.min_width_1(text), &pasted, p == 'p', 1);
                return Outcome::Handled;
            }
            _ => None,
        };
        if let Some(at) = to {
            *t.selection = sel.put_cursor(text, at, extend);
        }
        Outcome::Handled
    }

    fn enter_insert(&mut self, at: usize, t: &mut Target) {
        if self.mode.reviewing() {
            return;
        }
        self.mode = Mode::Insert;
        *t.selection = Range::point(at);
    }

    /// Paste after (or before) the selection; text ending in a newline goes
    /// on its own line, as yanked with `x`. The pasted text ends up selected.
    fn paste(&mut self, t: &mut Target, target: Range, text: &str, after: bool, count: usize) {
        if text.is_empty() {
            return;
        }
        let b = &*t.buffer;
        let mut text = text.repeat(count);
        let linewise = text.ends_with('\n');
        let at = match (linewise, after) {
            (true, true) => (b.line_end_at(b.prev(target.to()).max(target.from())) + 1).min(b.len()),
            (true, false) => b.line_start_at(target.from()),
            (false, true) => target.to(),
            (false, false) => target.from(),
        };
        if linewise && at == b.len() && !b.is_empty() && b.line_start_at(at) != at {
            text = format!("\n{}", text.trim_end_matches('\n'));
        }
        let selected = Range::new(at, at + text.len());
        self.edit(t, at..at, &text, selected, Mode::Normal);
    }

    /// Replace `range` with `text`, then select `after` in `mode`. Nothing
    /// is edited while reviewing.
    fn edit(&mut self, t: &mut Target, range: std::ops::Range<usize>, text: &str, after: Range, mode: Mode) {
        if self.mode.reviewing() {
            return;
        }
        t.buffer.edit(range, text);
        self.mode = mode;
        *t.selection = after.clamp(t.buffer.len());
    }
}

/// `sel` re-made over a replacement `len` bytes long starting at `from`, in
/// its direction (a point stays a point).
fn same_shape(sel: Range, from: usize, len: usize) -> Range {
    if sel.is_empty() {
        Range::point(from)
    } else if sel.head < sel.anchor {
        Range::new(from + len, from)
    } else {
        Range::new(from, from + len)
    }
}
