//! Normal and select mode: motions, selections and edits. AI and tour mode
//! use the same motions to look around, but every edit is refused.

use super::{Effect, Key, Modal, Mode, Outcome, Pending, Place, Registers, Target};
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
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(c as usize - '0' as usize).min(super::MAX_COUNT));
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
                let (r, col) = movement::move_vertically(b, sel, down, count, extend, self.sticky_col, t.indent.width);
                sticky = Some(col);
                Some(r)
            }
            Key::Ctrl('d' | 'u' | 'f' | 'b') | Key::PageDown | Key::PageUp => {
                let half = matches!(key, Key::Ctrl('d' | 'u'));
                let down = matches!(key, Key::Ctrl('d' | 'f') | Key::PageDown);
                let rows = if half { t.page_rows.max(2) / 2 } else { t.page_rows.max(2) };
                let (r, col) = movement::move_vertically(b, sel, down, rows.saturating_mul(count), extend, self.sticky_col, t.indent.width);
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
            // `X`: out to whole lines, not on to the next.
            Key::Char('X') => {
                let lines = b.lines_range(b.line_of(sel.from())..b.line_of(b.prev(sel.to()).max(sel.from())) + 1);
                Some(Range::new(lines.start, lines.end))
            }
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
            Key::Char(c @ ('/' | '?')) => return Outcome::Effect(Effect::Find { backward: c == '?' }),
            Key::Char(c @ ('n' | 'N')) => return Outcome::Effect(Effect::FindNext { backward: c == 'N' }),
            Key::Char('*') => return Outcome::Effect(Effect::FindSelection),
            Key::Ctrl('o') => return Outcome::Effect(Effect::JumpBack),
            Key::Ctrl('i') => return Outcome::Effect(Effect::JumpForward),
            // A count before them is for the key after (`5gg`, `2fx`).
            Key::Char('g') => (self.pending, self.count) = (Some(Pending::Goto), explicit_count),
            Key::Char(' ') => (self.pending, self.count) = (Some(Pending::Space), explicit_count),
            Key::Char('r') => (self.pending, self.count) = (Some(Pending::Replace), explicit_count),
            Key::Char('m') => (self.pending, self.count) = (Some(Pending::Match), explicit_count),
            Key::Char(c @ ('f' | 't' | 'F' | 'T')) => {
                let find = Pending::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') };
                (self.pending, self.count) = (Some(find), explicit_count);
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
            // Under review nothing's deleted, so nothing's yanked either.
            Key::Char('d' | 'c') if self.mode.reviewing() => self.refused = true,
            Key::Char(c @ ('d' | 'c')) => {
                regs.yank(self.register.take(), b.slice(target.bytes()).to_string());
                let mode = if c == 'c' { Mode::Insert } else { Mode::Normal };
                self.edit(t, target.bytes(), "", Range::point(target.from()), mode);
            }
            Key::Char('y') => {
                regs.yank(self.register.take(), b.slice(target.bytes()).to_string());
                if !self.mode.reviewing() {
                    self.mode = Mode::Normal;
                }
            }
            Key::Char(c @ ('p' | 'P')) => {
                let text = regs.yanked(self.register.take());
                self.paste(t, target, &text, c == 'p', count);
            }
            // `R`: the selection replaced with what was yanked.
            Key::Char('R') => {
                let text = regs.yanked(self.register.take()).repeat(count.min(1000));
                let after = Range::new(target.from(), target.from() + text.len());
                self.edit(t, target.bytes(), &text, after, self.mode);
            }
            Key::Char('"') => (self.pending, self.count) = (Some(Pending::Register), explicit_count),
            Key::Char('z') => self.pending = Some(Pending::View),
            Key::Char(c @ (']' | '[')) => self.pending = Some(Pending::Next(c == ']')),
            Key::Char('.') => return Outcome::Effect(Effect::Repeat),
            Key::Char('Q') => return Outcome::Effect(Effect::RecordMacro),
            Key::Char('q') => return Outcome::Effect(Effect::PlayMacro),
            // `J`: the selection's lines (or this and the next) joined with a space.
            Key::Char('J') => {
                let first = b.line_of(sel.from());
                let last = b.line_of(b.prev(sel.to()).max(sel.from())).max(first + 1).min(b.last_line());
                let (start, end) = (b.line_start(first), b.line_end(last));
                let mut joined = String::new();
                for (i, line) in b.slice(start..end).split('\n').enumerate() {
                    let line = if i == 0 { line.trim_end() } else { line.trim() };
                    if i > 0 && !joined.is_empty() && !line.is_empty() {
                        joined.push(' ');
                    }
                    joined.push_str(line);
                }
                self.edit(t, start..end, &joined, Range::point(start + joined.len().saturating_sub(1)), self.mode);
            }
            // `ctrl-a`, `ctrl-x`: the number at or after the cursor on its line, up or down by the count.
            Key::Ctrl(c @ ('a' | 'x')) => {
                let cursor = sel.cursor(text);
                let (start, end) = (b.line_start_at(cursor), b.line_end_at(cursor));
                if let Some((at, len, n)) = number_at(&text[start..end], cursor - start) {
                    let by = count as i128 * if c == 'a' { 1 } else { -1 };
                    let new = (n + by).to_string();
                    let at = start + at;
                    self.edit(t, at..at + len, &new, Range::new(at, at + new.len()), self.mode);
                }
            }
            // `ctrl-c`: the selection's lines commented out, or back in.
            Key::Ctrl('c') => {
                if let Some(token) = t.language.and_then(|l| l.comment) {
                    let first = b.line_of(sel.from());
                    let last = b.line_of(b.prev(sel.to()).max(sel.from()));
                    let (start, end) = (b.line_start(first), b.line_end(last));
                    let toggled = toggle_comment(b.slice(start..end), token);
                    let after = Range::new(start, start + toggled.len());
                    self.edit(t, start..end, &toggled, after, self.mode);
                }
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
                        '>' if !line.is_empty() => format!("{}{line}", unit.repeat(count.min(100))),
                        '>' => line.to_string(),
                        _ => unindent(line, count, t.indent.width.max(1)).to_string(),
                    })
                    .collect();
                let shifted = shifted.join("\n");
                // Whole lines (`x`) stay whole: their line break stays selected.
                let line_break = target.to().saturating_sub(end).min(1);
                let after = Range::new(start, start + shifted.len() + line_break);
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
        // `r` then Enter or Tab replaces with a line break or a tab.
        let key = match (pending, key) {
            (Pending::Replace, Key::Enter) => Key::Char('\n'),
            (Pending::Replace, Key::Tab) => Key::Char('\t'),
            (_, key) => key,
        };
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
            (Pending::Goto, 'r') => return Outcome::Effect(Effect::References),
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
                let (open_len, close_len) = (b.next(left) - left, b.next(right) - right);
                let (new_open, new_close) = match pending {
                    Pending::SurroundReplaceWith(_) => {
                        let (open, close) = textobject::pair_of(ch);
                        (open.to_string(), close.to_string())
                    }
                    _ => (String::new(), String::new()),
                };
                // The selection moves with the text: inside the pair by the
                // change in the opening, after it by both.
                let map = |at: usize| {
                    if at <= left {
                        at
                    } else if at <= right {
                        at - open_len + new_open.len()
                    } else {
                        at.max(right + close_len) - open_len - close_len + new_open.len() + new_close.len()
                    }
                };
                let after = Range::new(map(sel.anchor), map(sel.head));
                self.edit(t, left..b.next(right), &format!("{new_open}{inner}{new_close}"), after, self.mode);
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
            // Ghostwrite's own keys are under space. Starting something else
            // waits until the review is done.
            (Pending::Space, 'a') if self.mode.reviewing() => return Outcome::Review(super::Review::Ask),
            (Pending::Space, 'a') => return Outcome::Effect(Effect::Assist),
            (Pending::Space, 't') => return Outcome::Effect(Effect::Terminal),
            (Pending::Space, 'e' | 'k' | 'd' | '/' | 'j') if self.mode.reviewing() => return Outcome::Handled,
            (Pending::Space, 'j') => return Outcome::Effect(Effect::NextProblem),
            (Pending::Space, 'e') => {
                self.to_tree();
                return Outcome::Effect(Effect::FocusTree);
            }
            (Pending::Space, 'k') => return Outcome::Effect(Effect::Hover),
            (Pending::Space, 'd') => return Outcome::Effect(Effect::Diagnostics),
            (Pending::Space, '/') => return Outcome::Effect(Effect::Search),
            (Pending::Space, '?') => return Outcome::Effect(Effect::Cheatsheet),
            (Pending::Register, name) => {
                self.register = Some(name);
                return Outcome::Handled;
            }
            (Pending::View, 'z' | 'c') => return Outcome::Effect(Effect::View(Place::Middle)),
            (Pending::View, 't') => return Outcome::Effect(Effect::View(Place::Top)),
            (Pending::View, 'b') => return Outcome::Effect(Effect::View(Place::Bottom)),
            (Pending::Next(forward), 'd') => return Outcome::Effect(Effect::Diagnostic { forward }),
            (Pending::Goto, 't') => return Outcome::Effect(Effect::ScreenLine(Place::Top)),
            (Pending::Goto, 'c') => return Outcome::Effect(Effect::ScreenLine(Place::Middle)),
            (Pending::Goto, 'b') => return Outcome::Effect(Effect::ScreenLine(Place::Bottom)),
            (Pending::Space, 'f') => return Outcome::Effect(Effect::PickFile),
            (Pending::Space, 'r') => return Outcome::Effect(Effect::Rename),
            (Pending::Space, '.') => return Outcome::Effect(Effect::CodeActions),
            (Pending::Space, 's') => return Outcome::Effect(Effect::Symbols),
            (Pending::Space, 'b') => return Outcome::Effect(Effect::PickBuffer),
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
            self.refused = true;
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
        // A count can't paste more than 64 MB.
        let mut text = text.repeat(count.min((64 << 20) / text.len().max(1)).max(1));
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
            self.refused = true;
            return;
        }
        t.buffer.edit(range, text);
        self.mode = mode;
        *t.selection = after.within(t.buffer.text());
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

/// The number in `line` at `at` or after it: where, how long, and its value.
fn number_at(line: &str, at: usize) -> Option<(usize, usize, i128)> {
    let bytes = line.as_bytes();
    // Back to the start of a number the cursor is in.
    let mut start = at.min(line.len());
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let start = start + line[start..].find(|c: char| c.is_ascii_digit())?;
    let negative = start > 0 && bytes[start - 1] == b'-';
    let from = if negative { start - 1 } else { start };
    let end = start + line[start..].find(|c: char| !c.is_ascii_digit()).unwrap_or(line.len() - start);
    Some((from, end - from, line[from..end].parse().ok()?))
}

/// `lines` commented out with `token` (at the least indent), or, if they
/// all are, back in. Blank lines are left be.
fn toggle_comment(lines: &str, token: &str) -> String {
    let filled = || lines.split('\n').filter(|l| !l.trim().is_empty());
    let commented = filled().all(|l| l.trim_start().starts_with(token));
    let indent = filled().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines
        .split('\n')
        .map(|line| match () {
            _ if line.trim().is_empty() => line.to_string(),
            _ if commented => {
                let at = line.len() - line.trim_start().len();
                let rest = &line[at + token.len()..];
                format!("{}{}", &line[..at], rest.strip_prefix(' ').unwrap_or(rest))
            }
            _ => format!("{}{token} {}", &line[..indent], &line[indent..]),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn comments_go_in_at_the_least_indent_and_come_back_out() {
        let code = "fn f() {\n    x();\n\n    y();\n}";
        let commented = super::toggle_comment(code, "//");
        assert_eq!(commented, "// fn f() {\n//     x();\n\n//     y();\n// }");
        assert_eq!(super::toggle_comment(&commented, "//"), code);
        assert_eq!(super::toggle_comment("    a\n    b", "#"), "    # a\n    # b");
        assert_eq!(super::number_at("v1.2 = 30", 5), Some((7, 2, 30)));
    }
}
