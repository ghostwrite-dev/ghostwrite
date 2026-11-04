//! Normal and select mode: motions, selections and edits. AI mode uses the
//! same motions to look around, but every edit is refused, and the keys
//! under `space` that would start something else wait for the review.

use super::{Effect, Key, Modal, Mode, Outcome, Pending, Place, Registers, Target};
use crate::buffer::Buffer;
use crate::indent::{self, unindent};
use crate::language::Language;
use crate::movement::{self, Word, line_last_char};
use crate::nodes;
use crate::selection::Range;
use crate::textobject;

/// A motion `alt-.` does again, as helix keeps it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Motion {
    /// `f` `t` `F` `T` and a char, or Enter (`None`): a line's end.
    Find { ch: Option<char>, forward: bool, till: bool, count: usize, extend: bool },
    /// `mi`, `ma`.
    Object { ch: char, around: bool, count: usize },
    /// `]p`, `[p`.
    Paragraph { forward: bool, count: usize },
    /// By the syntax tree (`alt-o`, `]f`, ...): see [`nodes`].
    Node(NodeMove),
}

/// A move by the syntax tree.
#[derive(Debug, Clone, Copy)]
pub(super) enum NodeMove {
    /// `alt-o`, `alt-i`.
    Expand,
    Shrink,
    /// `alt-n`, `alt-p`.
    Sibling {
        next: bool,
    },
    /// `alt-e`, `alt-b`.
    End {
        to_end: bool,
    },
    /// `]f`, `[a` and the like: to the `count`th object `name`.
    Object {
        name: &'static str,
        next: bool,
        count: usize,
    },
}

/// The syntax object a letter names after `mi`, `ma`, `]` or `[`, as helix has them.
pub(super) fn syntax_object(ch: char) -> Option<&'static str> {
    Some(match ch {
        'f' => "function",
        't' => "class",
        'a' => "parameter",
        'c' => "comment",
        'T' => "test",
        'e' => "entry",
        'x' => "xml-element",
        _ => return None,
    })
}

impl Motion {
    /// Where it takes `r` (`extend`: in select mode), in a file of `language`.
    fn from(self, b: &Buffer, r: Range, extend: bool, language: Option<&'static Language>) -> Option<Range> {
        let text = b.text();
        match self {
            Motion::Object { ch, around, .. } if let Some(name) = syntax_object(ch) => nodes::object(language?, text, r, name, around),
            Motion::Node(NodeMove::Expand) => nodes::expand(language?, text, r),
            Motion::Node(NodeMove::Shrink) => nodes::shrink(language?, text, r),
            Motion::Node(NodeMove::Sibling { next }) => nodes::sibling(language?, text, r, next),
            Motion::Node(NodeMove::End { to_end }) => nodes::node_end(language?, text, r, to_end, extend),
            Motion::Node(NodeMove::Object { name, next, count }) => {
                let to = nodes::goto_object(language?, text, r, name, next, count)?;
                // In select mode it stretches the selection there, as helix does.
                Some(if extend { Range::new(r.anchor, if to.head < r.anchor { to.anchor } else { to.head }) } else { to })
            }
            Motion::Find { ch: Some(ch), forward, till, count, extend } => movement::find_char(b, r, ch, forward, till, count, extend),
            Motion::Find { ch: None, forward, till, count, extend } => movement::find_line_end(b, r, forward, till, count, extend),
            Motion::Object { ch, around, count } => textobject::text_object(b, r, ch, around, count),
            Motion::Paragraph { forward: true, count } => Some(movement::next_paragraph(b, r, count, extend)),
            Motion::Paragraph { forward: false, count } => Some(movement::prev_paragraph(b, r, count, extend)),
        }
    }
}

impl Modal {
    pub(super) fn normal_key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        let sel = *t.selection;
        let extend = self.mode == Mode::Select;

        if let Some(pending) = self.pending.take() {
            let count = self.count.take().unwrap_or(1);
            return self.pending_key(pending, key, count, sel, extend, regs, t);
        }
        if self.count_digit(key) {
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
            // `G` goes to the line counted (`10G`); without a count, nowhere, as in helix.
            Key::Char('G') => explicit_count.map(|n| sel.put_cursor(text, b.line_start((n - 1).min(b.last_line())), extend)),
            Key::Home => Some(sel.put_cursor(text, b.line_start_at(sel.cursor(text)), extend)),
            Key::End => Some(sel.put_cursor(text, line_last_char(b, sel.cursor(text)), extend)),
            Key::Char('x') => Some(movement::extend_line_below(b, sel, count)),
            // `X`: out to whole lines, not on to the next, facing as it did.
            Key::Char('X') => {
                let lines = b.lines_range(b.line_of(sel.from())..b.line_of(b.prev(sel.to()).max(sel.from())) + 1);
                Some(facing(sel, lines.start, lines.end))
            }
            Key::Alt('x') => Some(shrink_to_lines(b, sel)),
            Key::Char('%') => Some(Range::new(0, b.len())),
            Key::Char(';') => Some(Range::point(sel.cursor(text))),
            Key::Alt(';') => Some(sel.flip()),
            Key::Char('_') => Some(trimmed(text, sel)),
            Key::Alt('s') => {
                let mut lines = split_lines(text, sel).into_iter();
                let first = lines.next();
                self.added = lines.collect();
                first
            }
            Key::Alt('.') => self.last_motion.map(|motion| (0..count).fold(sel, |r, _| motion.from(b, r, extend, t.language).unwrap_or(r))),
            Key::Alt(c @ ('o' | 'i' | 'n' | 'p' | 'e' | 'b')) => {
                let node = match c {
                    'o' => NodeMove::Expand,
                    'i' => NodeMove::Shrink,
                    'n' | 'p' => NodeMove::Sibling { next: c == 'n' },
                    _ => NodeMove::End { to_end: c == 'e' },
                };
                self.motion(Motion::Node(node), t);
                return Outcome::Handled;
            }
            // `alt-a`: each node beside the selection's; `alt-I`: each inside it.
            Key::Alt(c @ ('a' | 'I')) => {
                let spread = t.language.and_then(|language| nodes::spread(language, text, sel, c == 'a'));
                let mut each = spread.unwrap_or_default().into_iter();
                let first = each.next();
                self.added = each.collect();
                first
            }
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
            Key::Char(c @ ('n' | 'N')) => return Outcome::Effect(Effect::FindNext { backward: c == 'N', count }),
            Key::Char('*') => return Outcome::Effect(Effect::FindSelection { word_bounds: true }),
            Key::Alt('*') => return Outcome::Effect(Effect::FindSelection { word_bounds: false }),
            Key::Ctrl('o') => return Outcome::Effect(Effect::JumpBack(count)),
            Key::Ctrl('i') => return Outcome::Effect(Effect::JumpForward(count)),
            // A count before them is for the key after (`5gg`, `2fx`).
            Key::Char('g') => (self.pending, self.count) = (Some(Pending::Goto), explicit_count),
            Key::Char(' ') => (self.pending, self.count) = (Some(Pending::Space), explicit_count),
            Key::Ctrl('w') => self.pending = Some(Pending::Pane),
            Key::Char('r') => (self.pending, self.count) = (Some(Pending::Replace), explicit_count),
            Key::Char('m') => (self.pending, self.count) = (Some(Pending::Match), explicit_count),
            Key::Char(c @ ('f' | 't' | 'F' | 'T')) => {
                let find = Pending::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') };
                (self.pending, self.count) = (Some(find), explicit_count);
            }
            Key::Char('v') if !self.mode.reviewing() => self.mode = if self.mode == Mode::Select { Mode::Normal } else { Mode::Select },
            Key::Escape => self.mode = Mode::Normal,
            // `i` types before the selection, `a` after it; either way it
            // stays selected, as in helix (a single char is just the cursor).
            Key::Char('i') if sel.is_wide(text) => self.enter_insert(Range::new(target.to(), target.from()), t),
            Key::Char('i') => self.enter_insert(Range::point(target.from()), t),
            Key::Char('a') => self.enter_insert(Range::new(target.from(), target.to()), t),
            Key::Char(c @ ('I' | 'A')) => self.insert_on_line(t, sel, c == 'A', extend),
            Key::Char(c @ ('o' | 'O')) => self.open(t, target, c == 'o', count, true),
            // Under review nothing's deleted, so nothing's yanked either.
            Key::Char('d' | 'c') | Key::Alt('d' | 'c') if self.mode.reviewing() => self.refused = true,
            // `alt-d`, `alt-c`: the same, but nothing's yanked.
            Key::Char(c @ ('d' | 'c')) | Key::Alt(c @ ('d' | 'c')) => {
                let register = self.register.take();
                if matches!(key, Key::Char(_)) {
                    regs.yank(register, b.slice(target.bytes()).to_string());
                }
                let whole_lines = c == 'c' && whole_lines(b, target);
                let mode = if c == 'c' && !whole_lines { Mode::Insert } else { Mode::Normal };
                self.edit(t, target.bytes(), "", Range::point(target.from()), mode);
                // Lines changed: typing goes on a line of its own in their place.
                if whole_lines {
                    let at = Range::point(target.from());
                    self.open(t, at.min_width_1(t.buffer.text()), false, 1, false);
                }
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
            // `R`: the selection replaced with what was yanked (if anything was).
            Key::Char('R') => {
                let text = regs.yanked(self.register.take()).repeat(count.min(1000));
                if !text.is_empty() {
                    let after = Range::new(target.from(), target.from() + text.len());
                    self.edit(t, target.bytes(), &text, after, Mode::Normal);
                }
            }
            Key::Char('"') => (self.pending, self.count) = (Some(Pending::Register), explicit_count),
            Key::Char('z') => self.pending = Some(Pending::View),
            Key::Char(c @ (']' | '[')) => (self.pending, self.count) = (Some(Pending::Next(c == ']')), explicit_count),
            Key::Char('.') => return Outcome::Effect(Effect::Repeat),
            Key::Char('Q') => return Outcome::Effect(Effect::RecordMacro),
            Key::Char('q') => return Outcome::Effect(Effect::PlayMacro),
            // `J`: the selection's lines (or this and the next) joined; `alt-J` selects the spaces put between.
            Key::Char('J') | Key::Alt('J') => self.join(t, sel, key == Key::Alt('J')),
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
            Key::Char(c @ ('u' | 'U')) if !self.mode.reviewing() => {
                for _ in 0..count {
                    let Some(r) = (if c == 'u' { t.buffer.undo() } else { t.buffer.redo() }) else { break };
                    *t.selection = r.clamp(t.buffer.len());
                }
            }
            Key::Char(c @ ('>' | '<')) => {
                let start = b.line_start_at(target.from());
                let end = b.line_end_at(b.prev(target.to()).max(target.from()));
                let (unit, width) = (t.indent.as_str().repeat(count.min(100)), t.indent.width.max(1));
                let shifted: Vec<String> = b
                    .slice(start..end)
                    .split('\n')
                    .map(|line| match c {
                        // Blank lines stay so; others go in to the next indent stop, as in helix.
                        '>' if line.trim().is_empty() => line.to_string(),
                        '>' if t.indent.hard_tabs => format!("{unit}{line}"),
                        '>' => {
                            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                            format!("{}{line}", &unit[indent % width..])
                        }
                        _ => unindent(line, count, width).to_string(),
                    })
                    .collect();
                let shifted = shifted.join("\n");
                // Whole lines (`x`) stay whole: their line break stays selected.
                let line_break = target.to().saturating_sub(end).min(1);
                let after = Range::new(start, start + shifted.len() + line_break);
                self.edit(t, start..end, &shifted, after, Mode::Normal);
            }
            // `~` switches case; `` ` `` is lower case, `` alt-` `` upper.
            Key::Char('~') => self.change_case(t, sel, |ch| if ch.is_uppercase() { ch.to_lowercase().collect() } else { ch.to_uppercase().collect() }),
            Key::Char('`') => self.change_case(t, sel, |ch| ch.to_lowercase().collect()),
            Key::Alt('`') => self.change_case(t, sel, |ch| ch.to_uppercase().collect()),
            // Other ctrl keys aren't modal editing's: the app may have them.
            Key::Ctrl(_) => return Outcome::Ignored,
            _ => {}
        }
        Outcome::Handled
    }

    #[allow(clippy::too_many_arguments)]
    fn pending_key(&mut self, pending: Pending, key: Key, count: usize, sel: Range, extend: bool, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        // `r` then Enter or Tab replaces with a line break or a tab, and `f`
        // then Tab finds one; `ctrl-w` takes its keys with ctrl too.
        let key = match (pending, key) {
            (Pending::Replace, Key::Enter) => Key::Char('\n'),
            (Pending::Replace | Pending::Find { .. }, Key::Tab) => Key::Char('\t'),
            (Pending::Pane, Key::Ctrl(c)) => Key::Char(c),
            (Pending::Find { forward, till }, Key::Enter) => {
                self.motion(Motion::Find { ch: None, forward, till, count, extend }, t);
                return Outcome::Handled;
            }
            (_, key) => key,
        };
        let Key::Char(c) = key else { return Outcome::Handled };
        let b = &*t.buffer;
        let text = b.text();
        let cursor = sel.cursor(text);
        let to = match (pending, c) {
            (Pending::Goto, 'g') => Some(b.line_start((count - 1).min(b.last_line()))),
            (Pending::Goto, 'e') => Some(b.line_start(b.last_line())),
            (Pending::Goto, 'h') => Some(b.line_start_at(cursor)),
            (Pending::Goto, 'l') => Some(line_last_char(b, cursor)),
            (Pending::Goto, 's') => Some(b.first_non_blank_at(cursor)),
            (Pending::Goto, '.') => b.last_edit(),
            (Pending::Goto, 'n') => return Outcome::Effect(Effect::NextBuffer(count)),
            (Pending::Goto, 'p') => return Outcome::Effect(Effect::PrevBuffer(count)),
            (Pending::Goto, 'N') => return Outcome::Effect(Effect::MoveBufferNext(count)),
            (Pending::Goto, 'P') => return Outcome::Effect(Effect::MoveBufferPrev(count)),
            (Pending::Goto, 'd') => return Outcome::Effect(Effect::GotoDefinition),
            (Pending::Goto, 'f') => return Outcome::Effect(Effect::GotoFile),
            (Pending::Goto, 'r') => return Outcome::Effect(Effect::References),
            (Pending::Goto, 't') => return Outcome::Effect(Effect::ScreenLine(Place::Top)),
            (Pending::Goto, 'c') => return Outcome::Effect(Effect::ScreenLine(Place::Middle)),
            (Pending::Goto, 'b') => return Outcome::Effect(Effect::ScreenLine(Place::Bottom)),
            (Pending::Find { forward, till }, ch) => {
                self.motion(Motion::Find { ch: Some(ch), forward, till, count, extend }, t);
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
                // The count is for which pair (`2md(`: the second out).
                self.count = Some(count);
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
                self.count = Some(count);
                return Outcome::Handled;
            }
            (pending @ (Pending::SurroundDelete | Pending::SurroundReplaceWith(_)), ch) => {
                let target = sel.min_width_1(text);
                let which = if let Pending::SurroundReplaceWith(from) = pending { from } else { ch };
                let Some((left, right)) = textobject::pair_around(b, target.from(), target.to(), which, count) else { return Outcome::Handled };
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
                self.edit(t, left..b.next(right), &format!("{new_open}{inner}{new_close}"), after, Mode::Normal);
                return Outcome::Handled;
            }
            (Pending::Object { around }, ch) => {
                self.motion(Motion::Object { ch, around, count }, t);
                return Outcome::Handled;
            }
            (Pending::Replace, ch) => {
                let target = sel.min_width_1(text);
                let replaced: String = b.slice(target.bytes()).chars().map(|o| if o == '\n' { o } else { ch }).collect();
                let after = same_shape(sel, target.from(), replaced.len());
                self.edit(t, target.bytes(), &replaced, after, Mode::Normal);
                return Outcome::Handled;
            }
            (Pending::Pane, 'v') => return Outcome::Effect(Effect::SplitRight),
            (Pending::Pane, 'w' | 'h' | 'l') => return Outcome::Effect(Effect::OtherPane),
            (Pending::Pane, 'q' | 'o') => return Outcome::Effect(Effect::CloseSplit),
            (Pending::Pane, _) => return Outcome::Handled,
            // Ghostwrite's own keys are under space. While reviewing, `space a`
            // asks about what's under review; looking around (files, open files,
            // symbols, docs, the tree, the terminal, copying, the cheatsheet) is free, and
            // starting anything else waits until it's done.
            (Pending::Space, 'a') if self.mode.reviewing() => return Outcome::Review(super::Review::Ask),
            (Pending::Space, c) if self.mode.reviewing() && !matches!(c, 't' | 'y' | 'p' | '?' | 'f' | 'b' | 's' | 'k' | 'e') => return Outcome::Handled,
            (Pending::Space, 'a') => return Outcome::Effect(Effect::Assist),
            (Pending::Space, 't') => return Outcome::Effect(Effect::Terminal),
            (Pending::Space, 'j') => return Outcome::Effect(Effect::NextProblem),
            (Pending::Space, 'e') => {
                self.to_tree();
                return Outcome::Effect(Effect::FocusTree);
            }
            (Pending::Space, 'k') => return Outcome::Effect(Effect::Hover),
            (Pending::Space, 'd') => return Outcome::Effect(Effect::Diagnostics),
            (Pending::Space, '/') => return Outcome::Effect(Effect::Search),
            (Pending::Space, '?') => return Outcome::Effect(Effect::Cheatsheet),
            (Pending::Space, 'f') => return Outcome::Effect(Effect::PickFile),
            (Pending::Space, 'r') => return Outcome::Effect(Effect::Rename),
            (Pending::Space, '.') => return Outcome::Effect(Effect::CodeActions),
            (Pending::Space, 's') => return Outcome::Effect(Effect::Symbols),
            (Pending::Space, 'b') => return Outcome::Effect(Effect::PickBuffer),
            (Pending::Register, name) => {
                self.register = Some(name);
                return Outcome::Handled;
            }
            (Pending::View, 'z' | 'c') => return Outcome::Effect(Effect::View(Place::Middle)),
            (Pending::View, 't') => return Outcome::Effect(Effect::View(Place::Top)),
            (Pending::View, 'b') => return Outcome::Effect(Effect::View(Place::Bottom)),
            (Pending::Next(forward), 'd') => return Outcome::Effect(Effect::Diagnostic { forward }),
            (Pending::Next(next), ch) if let Some(name) = syntax_object(ch) => {
                self.motion(Motion::Node(NodeMove::Object { name, next, count }), t);
                return Outcome::Handled;
            }
            (Pending::Next(forward), 'p') => {
                self.motion(Motion::Paragraph { forward, count }, t);
                return Outcome::Handled;
            }
            // `] space`, `[ space`: blank lines below the selection, or above.
            (Pending::Next(below), ' ') => {
                let (first, last) = (b.line_of(sel.from()), b.line_of(b.prev(sel.to()).max(sel.from())));
                let at = movement::start_of(b, if below { last + 1 } else { first });
                let moved = |p: usize| if p > at || (p == at && !below) { p + count } else { p };
                let after = Range::new(moved(sel.anchor), moved(sel.head));
                self.edit(t, at..at, &"\n".repeat(count), after, self.mode);
                return Outcome::Handled;
            }
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

    /// Carry out a motion, and keep it for `alt-.`.
    fn motion(&mut self, motion: Motion, t: &mut Target) {
        self.last_motion = Some(motion);
        if let Some(r) = motion.from(t.buffer, *t.selection, self.mode == Mode::Select, t.language) {
            *t.selection = r;
        }
    }

    /// Into insert mode, with `selection`: its head is where typing goes.
    fn enter_insert(&mut self, selection: Range, t: &mut Target) {
        if self.mode.reviewing() {
            self.refused = true;
            return;
        }
        self.mode = Mode::Insert;
        *t.selection = selection;
    }

    /// `I`, `A` (`end`): typing at the cursor's line's first non-blank, or
    /// its end; in select mode the selection stretches there. An empty line
    /// is indented first, as a line opened after the one above would be.
    fn insert_on_line(&mut self, t: &mut Target, sel: Range, end: bool, extend: bool) {
        let b = &*t.buffer;
        let text = b.text();
        let line = b.line_of(sel.cursor(text));
        let start = b.line_start(line);
        if b.line(line).is_empty() {
            let opens = t.language.unwrap_or(&Language::DEFAULT).indent_after;
            let indent = line.checked_sub(1).map_or(String::new(), |above| indent::for_newline(b.line(above), opens, t.indent));
            let after = Range::point(start + indent.len());
            return self.edit(t, start..start, &indent, after, Mode::Insert);
        }
        let at = if end { b.line_end(line) } else { b.first_non_blank_at(start) };
        let r = sel.put_cursor(text, at, extend);
        self.enter_insert(if extend { Range::new(r.anchor, at) } else { r }, t);
    }

    /// `o`, `O` (and `c` on whole lines): `count` lines opened below the
    /// selection (or above), each with a cursor, indented as Enter would,
    /// and going on with a line comment the line has (if `comments`).
    fn open(&mut self, t: &mut Target, target: Range, below: bool, count: usize, comments: bool) {
        if self.mode.reviewing() {
            self.refused = true;
            return;
        }
        let b = &*t.buffer;
        let line = b.line_of(if below { b.prev(target.to()) } else { target.from() });
        let language = t.language.unwrap_or(&Language::DEFAULT);
        let leader = language.comment_leader(b.line(line)).filter(|_| comments);
        let indent = match (leader, below, line.checked_sub(1)) {
            (Some(_), ..) => b.indent_at(b.line_start(line)).to_string(),
            (None, true, _) => indent::for_newline(b.line(line), language.indent_after, t.indent),
            (None, false, Some(above)) => indent::for_newline(b.line(above), language.indent_after, t.indent),
            (None, false, None) => b.indent_at(0).to_string(),
        };
        let one = format!("{indent}{}", leader.map(|l| format!("{l} ")).unwrap_or_default());
        let step = one.len() + 1;
        let (at, opened, first) = match (below, line.checked_sub(1)) {
            (true, _) => (b.line_end(line), format!("\n{one}").repeat(count), b.line_end(line) + step),
            (false, Some(above)) => (b.line_end(above), format!("\n{one}").repeat(count), b.line_end(above) + step),
            (false, None) => (0, format!("{one}\n").repeat(count), one.len()),
        };
        self.edit(t, at..at, &opened, Range::point(first), Mode::Insert);
        self.added = (1..count).map(|i| Range::point(first + i * step)).collect();
    }

    /// `J`: the selection's lines joined (or this one and the next), as
    /// helix joins them: each line's indent goes, and the comment leader it
    /// shares with the line before; a space goes between, unless what's
    /// joined is blank. With `select_spaces` (`alt-J`), the spaces are selected.
    fn join(&mut self, t: &mut Target, sel: Range, select_spaces: bool) {
        let b = &*t.buffer;
        let text = b.text();
        let first = b.line_of(sel.from());
        let mut last = b.line_of(b.prev(sel.to()).max(sel.from()));
        if first == last {
            last = (last + 1).min(b.line_count() - 1);
        }
        let language = t.language.unwrap_or(&Language::DEFAULT);
        let mut leader = language.comment_leader(b.line(first));
        // Each join: from the line's end to what's kept of the next, and whether a space goes there.
        let mut joins = vec![];
        for line in first..last {
            let next = b.line(line + 1);
            let mut end = b.line_start(line + 1) + next.len() - next.trim_start_matches([' ', '\t']).len();
            match language.comment_leader(next) {
                Some(next_leader) if Some(next_leader) == leader => {
                    end += next_leader.len();
                    end += text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
                }
                Some(next_leader) => leader = Some(next_leader),
                None => {}
            }
            joins.push((b.line_end(line), end, end != b.line_end(line + 1)));
        }
        let (Some(&(from, ..)), Some(&(.., to, _))) = (joins.first(), joins.last()) else { return };
        let mut joined = String::new();
        let mut kept = from;
        for &(start, end, space) in &joins {
            joined.push_str(&text[kept..start]);
            joined.push_str(if space { " " } else { "" });
            kept = end;
        }
        // Where a place in the text is after the joins.
        let moved = |at: usize| {
            let mut by = 0isize;
            for &(start, end, space) in &joins {
                if at >= end {
                    by += space as isize - (end - start) as isize;
                } else if at > start {
                    return start.saturating_add_signed(by);
                } else {
                    break;
                }
            }
            at.saturating_add_signed(by)
        };
        let mut spaces = joins.iter().filter(|(.., space)| *space).map(|&(start, ..)| Range::point(moved(start)));
        let after = match spaces.next().filter(|_| select_spaces) {
            Some(space) => space,
            None => Range::new(moved(sel.anchor), moved(sel.head)),
        };
        let added: Vec<Range> = if select_spaces { spaces.collect() } else { vec![] };
        self.edit(t, from..to, &joined, after, self.mode);
        if !self.mode.reviewing() {
            self.added = added;
        }
    }

    /// The selection's text with each char changed by `change`; select mode ends.
    fn change_case(&mut self, t: &mut Target, sel: Range, change: impl Fn(char) -> Vec<char>) {
        let target = sel.min_width_1(t.buffer.text());
        let changed: String = t.buffer.slice(target.bytes()).chars().flat_map(change).collect();
        let after = same_shape(sel, target.from(), changed.len());
        self.edit(t, target.bytes(), &changed, after, Mode::Normal);
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

/// `from..to`, facing the way `sel` does.
fn facing(sel: Range, from: usize, to: usize) -> Range {
    if sel.head < sel.anchor { Range::new(to, from) } else { Range::new(from, to) }
}

/// Whether `r` is whole lines, line breaks and all (`x`), as helix's
/// `selection_is_linewise` asks before `c` opens a line in their place.
fn whole_lines(b: &Buffer, r: Range) -> bool {
    let text = b.text();
    let (from, to) = (r.from(), r.to());
    text[from..to].contains('\n') && b.line_start_at(from) == from && (to == b.len() || text[..to].ends_with('\n'))
}

/// `alt-x`: in to the whole lines in the selection, as helix's
/// `shrink_to_line_bounds`; on one line it stays as it is.
fn shrink_to_lines(b: &Buffer, r: Range) -> Range {
    let (first, last) = (b.line_of(r.from()), b.line_of(b.prev(r.to()).max(r.from())));
    if first == last {
        return r;
    }
    let start = if b.line_start(first) == r.from() { r.from() } else { movement::start_of(b, first + 1) };
    let end = if movement::start_of(b, last + 1) == r.to() { r.to() } else { b.line_start(last) };
    facing(r, start, end)
}

/// `_`: the selection without the whitespace at its ends; nothing but
/// whitespace, and it's the cursor alone.
fn trimmed(text: &str, r: Range) -> Range {
    let selected = &text[r.bytes()];
    if selected.trim().is_empty() {
        return Range::point(r.cursor(text));
    }
    let start = r.from() + selected.len() - selected.trim_start().len();
    facing(r, start, start + selected.trim().len())
}

/// `alt-s`: the selection split at its line breaks, as helix's
/// `split_on_newline` (the line breaks themselves left out).
fn split_lines(text: &str, r: Range) -> Vec<Range> {
    if r.is_empty() {
        return vec![r];
    }
    let mut lines = vec![];
    let mut start = r.from();
    for (i, _) in text[r.bytes()].match_indices('\n') {
        lines.push(Range::new(start, r.from() + i));
        start = r.from() + i + 1;
    }
    if start < r.to() {
        lines.push(Range::new(start, r.to()));
    }
    lines
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
    // Indentation is spaces and tabs (one byte each), so it's sliced on char boundaries.
    let lead = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
    let filled = || lines.split('\n').filter(|l| !l.trim().is_empty());
    let commented = filled().all(|l| l[lead(l)..].starts_with(token));
    let indent = filled().map(lead).min().unwrap_or(0);
    lines
        .split('\n')
        .map(|line| match () {
            _ if line.trim().is_empty() => line.to_string(),
            _ if commented => {
                let at = lead(line);
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
    fn commenting_lines_indented_with_odd_spaces_doesnt_split_a_char() {
        // A non-breaking space isn't indentation: it stays in the text.
        let out = super::toggle_comment(" a\n\u{a0}b", "//");
        assert_eq!(out, "//  a\n// \u{a0}b");
        assert_eq!(super::toggle_comment(&out, "//"), " a\n\u{a0}b", "and back");
    }
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
