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
//! mode while a tour is (`:output` errors, diagnostics, search matches). Tab,
//! Shift+Tab, Backspace, Enter and Esc become [`Review`] keys; you can move
//! around to look, but not edit. They're entered over another mode with
//! [`Modal::enter`] and [`Modal::leave`] goes back to it, so asking Claude
//! about a tour's error goes normal → tour → AI and back again.
//!
//! While the completion menu is open, [`Modal::menu_key`] has first say.
//!
//! The modes live in their own files: `normal` (normal and select mode, and
//! the motions AI and tour mode share), `insert` (with the completion menu's
//! keys), and `tree`.

mod insert;
mod normal;
mod tree;

#[cfg(test)]
mod tests;

use crate::buffer::Buffer;
use crate::indent::IndentUnit;
use crate::selection::Range;

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
    Find {
        forward: bool,
        till: bool,
    },
    Replace,
    /// `m`, then: `m` `s` `r` `d` `a` `i`.
    Match,
    SurroundAdd,
    SurroundDelete,
    SurroundReplace,
    SurroundReplaceWith(char),
    Object {
        around: bool,
    },
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

    /// Enter tree mode (from normal mode; any insert session is closed first).
    pub fn to_tree(&mut self) {
        self.mode = Mode::Tree;
        self.pending = None;
        self.count = None;
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
