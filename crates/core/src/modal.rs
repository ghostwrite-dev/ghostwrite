//! Helix-style modal editing: keys in, edits and selection changes out.
//!
//! [`Modal`] holds the mode and any half-typed command (`g`, `f`, `mi`, a
//! count). [`Modal::key`] applies one key to a [`Target`] (a buffer and its
//! selection); anything that reaches beyond the buffer, like opening the
//! command line, comes back as an [`Effect`] for the editor to carry out.
//! In normal and select mode every key is consumed, so stray letters never
//! reach the text; in insert mode typed text arrives through
//! [`Modal::insert`]. Tree mode drives the file tree instead of a buffer:
//! [`Modal::tree_key`] turns keys into [`TreeCommand`]s.
//!
//! Two modes are for reviewing: AI mode while a Claude story is up, and tour
//! mode while a tour is (`:sh` errors, diagnostics, search matches). Tab,
//! Shift+Tab, Backspace, Enter and Esc become [`Review`] keys; you can move
//! around to look, but not edit. They're entered over another mode with
//! [`Modal::enter`] and [`Modal::leave`] goes back to it, so asking Claude
//! about a tour's error goes normal → tour → AI and back again.
//!
//! While the completion menu is open, [`Modal::menu_key`] has first say.

use crate::buffer::Buffer;
use crate::indent::{IndentUnit, unindent};
use crate::movement::{self, Word, line_last_char};
use crate::selection::Range;
use crate::textobject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Select,
    /// The file tree has the keyboard.
    Tree,
    /// Reviewing a Claude story.
    Ai,
    /// Touring places: a command's errors, diagnostics, search matches.
    Tour,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Insert => "INS",
            Mode::Select => "SEL",
            Mode::Tree => "TRE",
            Mode::Ai => "AI",
            Mode::Tour => "TOUR",
        }
    }

    /// AI and tour mode: look around, decide with the review keys, don't edit.
    pub fn reviewing(self) -> bool {
        matches!(self, Mode::Ai | Mode::Tour)
    }
}

/// A key, as modal editing sees it (the UI translates its keystrokes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    Escape,
    Enter,
    Tab,
    /// Shift+Tab.
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// What the editor around the buffer should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// `gn` / `gp`
    NextBuffer,
    PrevBuffer,
    /// `space e`: tree mode has begun; show the tree.
    FocusTree,
    /// `:`
    CommandLine,
    /// `space i`: ask Claude about the selection or current line, or to change it.
    Assist,
    /// `?`
    Cheatsheet,
    /// `gd`, `space k`, `space d`: the language server's definition, docs, diagnostics.
    GotoDefinition,
    Hover,
    Diagnostics,
    /// `space /`: search the project.
    Search,
}

/// What a key does in tree mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeCommand {
    Down(usize),
    Up(usize),
    First,
    Last,
    /// `l`: open a folder, or preview a file.
    Expand,
    /// `h`: close a folder, or go up to the one holding this row.
    Collapse,
    /// Enter: open the file (and leave the tree), or toggle a folder.
    Open,
    /// Esc, or `space e` again: back to the buffer.
    Leave,
    /// `a`, `r`, `d`: add, rename, trash.
    Add,
    Rename,
    Delete,
}

/// What a key does while a story or tour is under review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Review {
    /// Tab: accept, read on, or go to the next stop.
    Next,
    /// Shift+Tab: the previous stop (a story only goes forward).
    Back,
    /// Backspace: skip this one.
    Skip,
    /// Enter: ask about it, or redirect.
    Ask,
    /// Esc: stop.
    Stop,
}

/// What a key does while the completion menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Next,
    Prev,
    Accept,
    /// Esc closes the menu (and goes on to leave insert mode).
    Close,
}

/// What became of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not a modal key (insert-mode text, or a key for someone else).
    Ignored,
    Handled,
    Effect(Effect),
    Tree(TreeCommand),
    Review(Review),
}

/// What modal editing works on.
pub struct Target<'a> {
    pub buffer: &'a mut Buffer,
    pub selection: &'a mut Range,
    pub indent: &'a IndentUnit,
    /// Rows on screen, for paging.
    pub page_rows: usize,
}

/// The yank register and the system clipboard, shared across buffers.
pub trait Registers {
    fn yanked(&self) -> String;
    fn yank(&mut self, text: String);
    fn clipboard(&self) -> Option<String>;
    fn set_clipboard(&mut self, text: String);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Goto,
    Space,
    Find { forward: bool, till: bool },
    Replace,
    /// `m`, then: `m` `s` `r` `d` `a` `i`.
    Match,
    SurroundAdd,
    SurroundDelete,
    SurroundReplace,
    SurroundReplaceWith(char),
    Object { around: bool },
}

#[derive(Debug)]
pub struct Modal {
    pub mode: Mode,
    /// The modes AI and tour mode were entered from, innermost last.
    entered_from: Vec<Mode>,
    pending: Option<Pending>,
    count: Option<usize>,
    sticky_col: Option<usize>,
}

impl Default for Modal {
    fn default() -> Self {
        Self { mode: Mode::Normal, entered_from: vec![], pending: None, count: None, sticky_col: None }
    }
}

impl Modal {
    /// Keys typed so far toward a command, for the status line.
    pub fn pending_label(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        let pending = match self.pending {
            Some(Pending::Goto) => "g",
            Some(Pending::Space) => "space",
            Some(Pending::Find { forward: true, till: false }) => "f",
            Some(Pending::Find { forward: true, till: true }) => "t",
            Some(Pending::Find { forward: false, till: false }) => "F",
            Some(Pending::Find { forward: false, till: true }) => "T",
            Some(Pending::Replace) => "r",
            Some(Pending::Match) => "m",
            Some(Pending::SurroundAdd) => "ms",
            Some(Pending::SurroundDelete) => "md",
            Some(Pending::SurroundReplace) => "mr",
            Some(Pending::SurroundReplaceWith(_)) => "mr…",
            Some(Pending::Object { around: true }) => "ma",
            Some(Pending::Object { around: false }) => "mi",
            None => "",
        };
        format!("{count}{pending}")
    }

    /// Back to normal mode, closing an insert session as one undo step.
    pub fn to_normal(&mut self, t: &mut Target) {
        if self.mode == Mode::Insert {
            *t.selection = Range::point(t.selection.head);
        }
        self.mode = Mode::Normal;
        self.pending = None;
        self.count = None;
        t.buffer.commit(*t.selection);
    }

    /// Typed text in insert mode; `false` (and nothing typed) in other modes.
    pub fn insert(&mut self, text: &str, t: &mut Target) -> bool {
        if self.mode != Mode::Insert {
            return false;
        }
        let range = t.selection.bytes();
        t.buffer.begin(*t.selection);
        t.buffer.edit(range.clone(), text);
        *t.selection = Range::point(range.start + text.len());
        self.sticky_col = None;
        true
    }

    /// Enter AI or tour mode over the current one (an insert session should
    /// be closed first). Entering the mode you're in changes nothing.
    pub fn enter(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        let from = if self.mode == Mode::Insert { Mode::Normal } else { self.mode };
        self.entered_from.push(from);
        self.mode = mode;
        self.pending = None;
        self.count = None;
    }

    /// Leave AI or tour mode for the mode it was entered from. If it isn't
    /// the current mode but one further down, it's dropped from under it.
    pub fn leave(&mut self, mode: Mode) {
        if self.mode == mode {
            self.mode = self.entered_from.pop().unwrap_or(Mode::Normal);
            self.pending = None;
            self.count = None;
        } else if let Some(at) = self.entered_from.iter().rposition(|m| *m == mode) {
            // What was entered from it goes back to what it was entered from.
            self.entered_from.remove(at);
        }
    }

    /// The mode the current one was entered from (AI and tour mode).
    pub fn entered_from(&self) -> Option<Mode> {
        self.entered_from.last().copied().filter(|_| self.mode.reviewing())
    }

    /// A key for the buffer, in normal, select or insert mode (or AI and
    /// tour mode, which review it).
    pub fn key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        if self.mode == Mode::Tree {
            return self.tree_key(key);
        }
        if self.mode.reviewing()
            && self.pending.is_none()
            && let Some(review) = review_of(key)
        {
            return Outcome::Review(review);
        }
        if self.mode == Mode::Insert {
            return self.insert_key(key, t);
        }
        if matches!(key, Key::Ctrl(c) if !matches!(c, 'd' | 'u' | 'f' | 'b')) {
            return Outcome::Ignored;
        }
        t.buffer.begin(*t.selection);
        let outcome = self.normal_key(key, regs, t);
        if self.mode != Mode::Insert {
            t.buffer.commit(*t.selection);
        }
        outcome
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

    /// Enter tree mode (from normal mode; any insert session is closed first).
    pub fn to_tree(&mut self) {
        self.mode = Mode::Tree;
        self.pending = None;
        self.count = None;
    }

    /// A key in tree mode. `:` and `?` work as everywhere; `space e` leaves.
    pub fn tree_key(&mut self, key: Key) -> Outcome {
        let pending = self.pending.take();
        if let Key::Char(c @ '0'..='9') = key
            && pending.is_none()
            && (c != '0' || self.count.is_some())
        {
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(c as usize - '0' as usize));
            return Outcome::Handled;
        }
        let count = self.count.take().unwrap_or(1);
        let command = match (pending, key) {
            (Some(Pending::Goto), Key::Char('g')) => TreeCommand::First,
            (Some(Pending::Goto), Key::Char('e')) => TreeCommand::Last,
            (Some(Pending::Space), Key::Char('e')) => TreeCommand::Leave,
            (Some(Pending::Space), Key::Char('i')) => return Outcome::Effect(Effect::Assist),
            (Some(Pending::Space), Key::Char('/')) => return Outcome::Effect(Effect::Search),
            (Some(_), _) => return Outcome::Handled,
            (None, Key::Char('g')) => {
                self.pending = Some(Pending::Goto);
                return Outcome::Handled;
            }
            (None, Key::Char(' ')) => {
                self.pending = Some(Pending::Space);
                return Outcome::Handled;
            }
            (None, Key::Char(':')) => return Outcome::Effect(Effect::CommandLine),
            (None, Key::Char('?')) => return Outcome::Effect(Effect::Cheatsheet),
            (None, Key::Char('j') | Key::Down) => TreeCommand::Down(count),
            (None, Key::Char('k') | Key::Up) => TreeCommand::Up(count),
            (None, Key::Char('G') | Key::End) => TreeCommand::Last,
            (None, Key::Home) => TreeCommand::First,
            (None, Key::Char('l') | Key::Right) => TreeCommand::Expand,
            (None, Key::Char('h') | Key::Left) => TreeCommand::Collapse,
            (None, Key::Enter) => TreeCommand::Open,
            (None, Key::Escape | Key::Char('f')) => TreeCommand::Leave,
            (None, Key::Char('a')) => TreeCommand::Add,
            (None, Key::Char('r')) => TreeCommand::Rename,
            (None, Key::Char('d')) => TreeCommand::Delete,
            (None, Key::Ctrl(_) | Key::Alt(_)) => return Outcome::Ignored,
            (None, _) => return Outcome::Handled,
        };
        if command == TreeCommand::Leave {
            self.mode = Mode::Normal;
        }
        Outcome::Tree(command)
    }

    fn insert_key(&mut self, key: Key, t: &mut Target) -> Outcome {
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

    fn normal_key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
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
                    .flat_map(|ch| -> Vec<char> { if ch.is_uppercase() { ch.to_lowercase().collect() } else { ch.to_uppercase().collect() } })
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

/// What a key does in AI and tour mode, if it's a review key.
fn review_of(key: Key) -> Option<Review> {
    Some(match key {
        Key::Tab => Review::Next,
        Key::BackTab => Review::Back,
        Key::Backspace => Review::Skip,
        Key::Enter => Review::Ask,
        Key::Escape => Review::Stop,
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Regs {
        yanked: String,
        clipboard: Option<String>,
    }

    impl Registers for Regs {
        fn yanked(&self) -> String {
            self.yanked.clone()
        }
        fn yank(&mut self, text: String) {
            self.yanked = text;
        }
        fn clipboard(&self) -> Option<String> {
            self.clipboard.clone()
        }
        fn set_clipboard(&mut self, text: String) {
            self.clipboard = Some(text);
        }
    }

    /// A buffer driven by typed keys, as in the editor.
    struct Session {
        modal: Modal,
        buffer: Buffer,
        selection: Range,
        regs: Regs,
    }

    impl Session {
        fn new(text: &str) -> Self {
            Self { modal: Modal::default(), buffer: Buffer::new(text), selection: Range::point(0), regs: Regs::default() }
        }

        /// Space-separated keys; `esc` and `ret` are named, anything else
        /// longer than one char is typed text.
        fn keys(&mut self, keys: &str) -> &mut Self {
            let indent = IndentUnit::default();
            for word in keys.split(' ') {
                let key = match word {
                    "esc" => Some(Key::Escape),
                    "ret" => Some(Key::Enter),
                    "space" => Some(Key::Char(' ')),
                    w if w.chars().count() == 1 && self.modal.mode != Mode::Insert => Some(Key::Char(w.chars().next().unwrap())),
                    _ => None,
                };
                let mut t = Target { buffer: &mut self.buffer, selection: &mut self.selection, indent: &indent, page_rows: 20 };
                match key {
                    Some(key) => {
                        self.modal.key(key, &mut self.regs, &mut t);
                    }
                    None => {
                        self.modal.insert(word, &mut t);
                    }
                }
            }
            self
        }

        fn text(&self) -> &str {
            self.buffer.text()
        }
    }

    #[test]
    fn word_then_delete() {
        let mut s = Session::new("foo bar baz\n");
        assert_eq!(s.keys("w d").text(), "bar baz\n");
        // A count repeats the motion; only the last word stays selected.
        assert_eq!(s.keys("2 w d").text(), "bar \n");
        assert_eq!(s.keys("u").text(), "bar baz\n");
    }

    #[test]
    fn lines_yank_paste_and_open() {
        let mut s = Session::new("one\ntwo\n");
        assert_eq!(s.keys("x y j p").text(), "one\ntwo\none\n");
        assert_eq!(s.keys("g g x d").text(), "two\none\n");
        assert_eq!(s.keys("o new esc").text(), "two\nnew\none\n");
        assert_eq!(s.keys("A ! esc g e d").text(), "two\nnew!\nne\n");
    }

    #[test]
    fn an_insert_session_is_one_undo_step() {
        let mut s = Session::new("x\n");
        s.keys("o one ret two esc");
        assert_eq!(s.text(), "x\none\ntwo\n");
        assert_eq!(s.keys("u").text(), "x\n");
    }

    #[test]
    fn find_and_change() {
        let mut s = Session::new("let x = foo(bar);\n");
        assert_eq!(s.keys("t ( c call esc").text(), "call(bar);\n");
    }

    #[test]
    fn match_mode_selects_and_surrounds() {
        let mut s = Session::new("call(a, b)\n");
        assert_eq!(s.keys("l l l l l m i ( d").text(), "call()\n");
        s.keys("u");
        assert_eq!(s.keys("g h m i w m s '").text(), "'call'(a, b)\n");
        assert_eq!(s.keys("g h l m d '").text(), "call(a, b)\n");
        assert_eq!(s.keys("g l h m r ( [").text(), "call[a, b]\n");
        s.keys("g l m m");
        assert_eq!(s.selection.head, 4);
    }

    #[test]
    fn normal_mode_swallows_letters() {
        let mut s = Session::new("abc\n");
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
        assert!(!s.modal.insert("m", &mut t));
        assert_eq!(s.modal.key(Key::Char('z'), &mut s.regs, &mut t), Outcome::Handled);
        assert_eq!(s.text(), "abc\n");
    }

    #[test]
    fn tree_mode_turns_keys_into_tree_commands() {
        let mut m = Modal::default();
        m.to_tree();
        assert_eq!(m.tree_key(Key::Char('3')), Outcome::Handled);
        assert_eq!(m.tree_key(Key::Char('j')), Outcome::Tree(TreeCommand::Down(3)));
        assert_eq!(m.tree_key(Key::Char('g')), Outcome::Handled);
        assert_eq!(m.tree_key(Key::Char('g')), Outcome::Tree(TreeCommand::First));
        assert_eq!(m.tree_key(Key::Char('x')), Outcome::Handled, "other letters are swallowed");
        assert_eq!(m.tree_key(Key::Char(':')), Outcome::Effect(Effect::CommandLine));
        assert_eq!(m.tree_key(Key::Escape), Outcome::Tree(TreeCommand::Leave));
        assert_eq!(m.mode, Mode::Normal);
        m.to_tree();
        assert_eq!(m.tree_key(Key::Char('f')), Outcome::Tree(TreeCommand::Leave), "f goes back too");
        assert_eq!(m.mode, Mode::Normal);
    }

    #[test]
    fn review_modes_take_review_keys_and_go_back_where_they_came_from() {
        let mut s = Session::new("one two\n");
        s.keys("v w");
        s.modal.enter(Mode::Tour);
        s.modal.enter(Mode::Ai);
        assert_eq!(s.modal.entered_from(), Some(Mode::Tour));
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
        assert_eq!(s.modal.key(Key::Tab, &mut s.regs, &mut t), Outcome::Review(Review::Next));
        assert_eq!(s.modal.key(Key::BackTab, &mut s.regs, &mut t), Outcome::Review(Review::Back));
        s.modal.leave(Mode::Ai);
        assert_eq!(s.modal.mode, Mode::Tour);
        s.modal.leave(Mode::Tour);
        assert_eq!(s.modal.mode, Mode::Select, "back to where it started");
        assert_eq!(s.modal.menu_key(Key::Ctrl('n')), Some(MenuKey::Next));
    }

    #[test]
    fn reviewing_moves_but_never_edits() {
        let mut s = Session::new("one two\nthree\n");
        s.modal.enter(Mode::Ai);
        s.keys("w j d c x p u ~ > r z m s ( i o");
        assert_eq!(s.text(), "one two\nthree\n", "nothing changes");
        assert_eq!(s.modal.mode, Mode::Ai, "and the mode stays");
        s.keys("space i v");
        assert_eq!(s.modal.mode, Mode::Ai, "nothing new starts mid-review");
        s.modal.leave(Mode::Ai);
        s.modal.enter(Mode::Tour);
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
        s.modal.key(Key::Char(' '), &mut s.regs, &mut t);
        assert_eq!(s.modal.key(Key::Char('i'), &mut s.regs, &mut t), Outcome::Effect(Effect::Assist), "a tour can be asked about");
    }

    #[test]
    fn leaving_a_mode_below_the_current_one_drops_it() {
        let mut m = Modal::default();
        m.enter(Mode::Tour);
        m.enter(Mode::Ai);
        m.leave(Mode::Tour);
        assert_eq!(m.mode, Mode::Ai);
        m.leave(Mode::Ai);
        assert_eq!(m.mode, Mode::Normal);
    }

    #[test]
    fn f_enters_tree_mode() {
        let mut s = Session::new("abc\n");
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
        assert_eq!(s.modal.key(Key::Char('f'), &mut s.regs, &mut t), Outcome::Effect(Effect::FocusTree));
        assert_eq!(s.modal.mode, Mode::Tree);
    }

    #[test]
    fn effects_come_back() {
        let mut s = Session::new("abc\n");
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
        assert_eq!(s.modal.key(Key::Char(' '), &mut s.regs, &mut t), Outcome::Handled);
        assert_eq!(s.modal.key(Key::Char('i'), &mut s.regs, &mut t), Outcome::Effect(Effect::Assist));
        assert_eq!(s.modal.key(Key::Char(':'), &mut s.regs, &mut t), Outcome::Effect(Effect::CommandLine));
    }
}
