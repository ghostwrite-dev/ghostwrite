//! Helix-style modal editing: keys in, edits and selection changes out.
//!
//! [`Modal`] holds the mode and any half-typed command (`g`, `f`, `mi`, a
//! count). [`Modal::key`] applies one key to a [`Target`] (a buffer and its
//! selection); anything that reaches beyond the buffer, like opening the
//! command line, comes back as an [`Effect`] for the editor to carry out.
//! In normal and select mode every key is consumed, so stray letters never
//! reach the text; in insert mode typed text arrives through
//! [`Modal::insert`]. Tree mode drives the file tree instead of a buffer:
//! [`Modal::tree_key`] turns keys into [`TreeCommand`]s. Terminal mode
//! gives the keys to the user's shell: the editor sends them there itself,
//! and it's entered and left like the review modes, so a story under
//! review is still there to go back to.
//!
//! AI mode is for reviewing a story: Claude's, or one of the editor's
//! stops (`:output` errors, diagnostics, search matches). Tab, Enter and Esc
//! become [`Review`] keys; you can move around to look, but not edit. It's
//! entered over another mode with [`Modal::enter`], and [`Modal::leave`]
//! goes back to it.
//!
//! While the completion menu is open, [`Modal::menu_key`] has first say.
//!
//! The modes live in their own files: `normal` (normal and select mode, and
//! the motions AI mode shares), `insert` (with the completion menu's keys),
//! and `tree`.

mod insert;
mod normal;
mod tree;

#[cfg(test)]
mod tests;

use crate::abbreviation::Abbreviations;
use crate::buffer::Buffer;
use crate::indent::IndentUnit;
use crate::language::Language;
use crate::selection::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Select,
    /// The file tree has the keyboard.
    Tree,
    /// Reviewing a story.
    Ai,
    /// The terminal has the keyboard.
    Terminal,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Insert => "INS",
            Mode::Select => "SEL",
            Mode::Tree => "TRE",
            Mode::Ai => "AI",
            Mode::Terminal => "TERM",
        }
    }

    /// AI mode: look around, decide with the review keys, don't edit.
    pub fn reviewing(self) -> bool {
        self == Mode::Ai
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
    /// `ctrl-w v`: a second pane beside this one.
    SplitRight,
    /// `ctrl-w w`: the keys go to the other pane.
    OtherPane,
    /// `ctrl-w q`: only this pane.
    CloseSplit,
    /// `gn` / `gp`
    /// `gn`, `gp`: so many tabs along, round the ends (`4gn`).
    NextBuffer(usize),
    PrevBuffer(usize),
    /// `gN`, `gP`: move the tab so many places along, as far as there are (`4gN`).
    MoveBufferNext(usize),
    MoveBufferPrev(usize),
    /// `space e`: tree mode has begun; show the tree.
    FocusTree,
    /// `:`
    CommandLine,
    /// `space a`: ask Claude about the selection or current line, or to change it.
    Assist,
    /// `space ?`: every key.
    Cheatsheet,
    /// `/`, `?`: search the file, forward or back, as it's typed.
    Find {
        backward: bool,
    },
    /// `n`, `N`: the next match of the last search, or the one before.
    FindNext {
        backward: bool,
    },
    /// `*`: search for what's selected.
    FindSelection,
    /// `gr`, `space r`, `space .`, `space s`: the language server's
    /// references, renaming, code actions, and the file's symbols.
    References,
    Rename,
    CodeActions,
    Symbols,
    /// `space f`, `space b`: pick a file of the project, or an open one.
    PickFile,
    PickBuffer,
    /// `zz` `zt` `zb`: the cursor's line in the middle, at the top, at the bottom of the screen.
    View(Place),
    /// `gt` `gc` `gb`: to the top, middle, bottom line on screen.
    ScreenLine(Place),
    /// `]d`, `[d`: the next diagnostic, or the one before.
    Diagnostic {
        forward: bool,
    },
    /// `.`: the last change again.
    Repeat,
    /// `Q`: record keys (again: stop); `q`: play them.
    RecordMacro,
    PlayMacro,
    /// `ctrl-o`, `ctrl-i`: back to where a jump came from, and forward again.
    JumpBack,
    JumpForward,
    /// `gd`, `space k`, `space d`: the language server's definition, docs, diagnostics.
    GotoDefinition,
    Hover,
    Diagnostics,
    /// `space /`: search the project.
    Search,
    /// `space t`: the terminal, and terminal mode.
    Terminal,
    /// `space j`: go to the next error in the file (or warning, if it has
    /// no errors), and bring a file's pages there.
    NextProblem,
}

/// Where on screen (`zt`, `gc`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Top,
    Middle,
    Bottom,
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

/// What a key does while a story is under review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Review {
    /// Tab: accept, or read on.
    Next,
    /// Enter: ask about it, or redirect.
    Ask,
    /// Esc: stop, once Esc has been pressed twice in a row ([`Review::Stopping`] first).
    Stop,
    /// The first Esc: one more stops.
    Stopping,
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
    /// The buffer's language: whether `\alpha` types `α` (see
    /// [`crate::abbreviation`]), which quotes close themselves, and so on.
    pub language: Option<&'static Language>,
}

impl Target<'_> {
    fn abbreviations(&self) -> Option<&'static Abbreviations> {
        self.language.and_then(|l| l.abbreviations)
    }

    /// Whether `c` closes itself when typed here.
    fn pairs(&self, c: char) -> bool {
        !self.language.is_some_and(|l| l.unpaired.contains(&c))
    }
}

/// The yank register and the system clipboard, shared across buffers.
/// The registers, by name: `None` is the default one, `+` the clipboard,
/// `_` throws away, and any other char names one of its own (`"a y`).
pub trait Registers {
    fn yanked(&self, register: Option<char>) -> String;
    fn yank(&mut self, register: Option<char>, text: String);
    fn clipboard(&self) -> Option<String>;
    fn set_clipboard(&mut self, text: String);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Goto,
    Space,
    /// `ctrl-w`, then: `v` `w` `q`, as in helix.
    Pane,
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
    /// `"`, then the register's name.
    Register,
    /// `z`: where the cursor's line goes on screen.
    View,
    /// `]`, `[`: the next or previous something (`d`: diagnostic).
    Next(bool),
}

#[derive(Debug, Clone)]
pub struct Modal {
    pub mode: Mode,
    /// The modes AI and terminal mode were entered from, innermost last.
    entered_from: Vec<Mode>,
    pending: Option<Pending>,
    count: Option<usize>,
    sticky_col: Option<usize>,
    /// The abbreviation being typed (`\alp`), from its backslash to the cursor.
    abbreviating: Option<(usize, usize)>,
    /// Esc was pressed in a review: another stops it.
    stopping: bool,
    /// The last key would have edited, but a review was up.
    refused: bool,
    /// The register the next yank, delete or paste uses (`"a`).
    register: Option<char>,
}

impl Default for Modal {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            entered_from: vec![],
            pending: None,
            count: None,
            sticky_col: None,
            abbreviating: None,
            stopping: false,
            refused: false,
            register: None,
        }
    }
}

impl Modal {
    /// Keys typed so far toward a command, for the status line.
    pub fn pending_label(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        let pending = self.pending_keys();
        let register = self.register.map(|r| format!("\"{r}")).unwrap_or_default();
        format!("{register}{count}{pending}")
    }

    /// The command started, if one is: `space`, `g`.
    pub fn pending_keys(&self) -> &'static str {
        match self.pending {
            Some(Pending::Goto) => "g",
            Some(Pending::Space) => "space",
            Some(Pending::Pane) => "ctrl-w",
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
            Some(Pending::Register) => "\"",
            Some(Pending::View) => "z",
            Some(Pending::Next(true)) => "]",
            Some(Pending::Next(false)) => "[",
            None => "",
        }
    }

    /// Forget the command started, and its count.
    pub fn cancel_pending(&mut self) {
        (self.pending, self.count) = (None, None);
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
    /// A typed opening bracket or quote comes with its closing one (see
    /// [`insert::paired`]), and typing a closing one that's next steps over
    /// it. In a language with abbreviations, a backslash starts an
    /// abbreviation (see [`Modal::abbreviate`]).
    pub fn insert(&mut self, text: &str, t: &mut Target) -> bool {
        if self.mode != Mode::Insert {
            return false;
        }
        if t.abbreviations().is_some() {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if t.selection.is_empty() && self.abbreviate(c, t) => return true,
                (Some(_), None) => {}
                _ => {
                    self.finish_abbreviation(t);
                }
            }
        }
        let range = t.selection.bytes();
        self.sticky_col = None;
        let (before, after) = (&t.buffer.text()[..range.start], &t.buffer.text()[range.end..]);
        let mut typed = text.chars();
        if let (Some(c), None, true) = (typed.next(), typed.next(), range.is_empty()) {
            if insert::closes(c) && after.starts_with(c) {
                *t.selection = Range::point(range.start + c.len_utf8());
                return true;
            }
            if let Some(close) = insert::paired(c, before, after).filter(|_| t.pairs(c)) {
                t.buffer.begin(*t.selection);
                t.buffer.edit(range.clone(), &format!("{c}{close}"));
                *t.selection = Range::point(range.start + c.len_utf8());
                return true;
            }
        }
        t.buffer.begin(*t.selection);
        t.buffer.edit(range.clone(), text);
        *t.selection = Range::point(range.start + text.len());
        true
    }

    /// Text typed in insert mode just as it is: no pairs, no abbreviations
    /// (what an input method is composing, or commits).
    pub fn insert_as_is(&mut self, text: &str, t: &mut Target) -> bool {
        if self.mode != Mode::Insert {
            return false;
        }
        self.sticky_col = None;
        let range = t.selection.bytes();
        t.buffer.begin(*t.selection);
        t.buffer.edit(range.clone(), text);
        *t.selection = Range::point(range.start + text.len());
        true
    }

    /// Enter AI or terminal mode over the current one (an insert session should
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

    /// Leave AI or terminal mode for the mode it was entered from. If it isn't
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

    /// Whether the last key would have edited the text, but a review was up (and say so once).
    pub fn take_refused(&mut self) -> bool {
        std::mem::take(&mut self.refused)
    }

    /// The mode AI mode was entered from.
    pub fn entered_from(&self) -> Option<Mode> {
        self.entered_from.last().copied().filter(|_| self.mode.reviewing())
    }

    /// A key for the buffer, in normal, select or insert mode (or AI mode,
    /// which reviews it).
    pub fn key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        if self.mode == Mode::Tree {
            return self.tree_key(key);
        }
        if self.mode == Mode::Terminal {
            return Outcome::Ignored;
        }
        let stopping = std::mem::take(&mut self.stopping);
        if self.mode.reviewing()
            && self.pending.is_none()
            && let Some(review) = review_of(key)
        {
            // Esc out of habit doesn't end a story: it takes two.
            let review = match review {
                Review::Stop if !stopping => Review::Stopping,
                review => review,
            };
            self.stopping = review == Review::Stopping;
            return Outcome::Review(review);
        }
        if self.mode == Mode::Insert {
            return self.insert_key(key, t);
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

/// The largest count: more is taken as this (`99999999999p` doesn't run out of memory).
const MAX_COUNT: usize = 100_000;

impl Modal {
    /// A digit of a count (`10j`), taken if it is one: `0` only goes on one.
    fn count_digit(&mut self, key: Key) -> bool {
        let Key::Char(c @ '0'..='9') = key else { return false };
        if c == '0' && self.count.is_none() {
            return false;
        }
        self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(c as usize - '0' as usize).min(MAX_COUNT));
        true
    }
}

/// What a key does in AI mode, if it's a review key.
fn review_of(key: Key) -> Option<Review> {
    Some(match key {
        Key::Tab => Review::Next,
        Key::Enter => Review::Ask,
        Key::Escape => Review::Stop,
        _ => return None,
    })
}
