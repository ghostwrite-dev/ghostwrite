//! The views plugins declare, and the keys of the views that aren't the code. Each
//! view's own keys are a layer of the keymap over the code's ([`super::keymap::VIEWS`],
//! and for a plugin's, what it shows: [`super::keymap::SHOWS`]): `j` is a row down in
//! Files, Enter opens the file a line names on the terminal, Esc goes back to the code.
//!
//! A view with no text (Files, the debugger, what's beside a file) has nothing for the
//! code's keys to move through: there, only its own keys and those that work anywhere
//! (`:`, and most under `space`) do something, and the rest are swallowed.

use super::command::Command;
use super::keymap::{Bound, Found};
use super::{Effect, Key, Modal, Mode, Outcome, Pending, Review, TreeCommand, ViewKind};

/// A plugin's view: which plugin's, what it's called, and what it shows. A plugin
/// declares it in its manifest, and the editor knows no view of a plugin's but by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId {
    pub plugin: &'static str,
    /// As the config, `focus_view` and a plugin's keys (`"in"`) call it: `pages`, `goals`.
    pub name: &'static str,
    pub shows: Shows,
}

/// What a plugin's view shows: a kind of content the editor knows how to draw, and the
/// keys that go with it. More can be added: each says what it's called in a manifest,
/// where it goes, and its keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Shows {
    /// A file's pages, from the plugin's renderer: drawn, with the cursor on them, a
    /// click going to the text, and zoom.
    Pages,
    /// What the plugin's panel says, kept up to date from the file's language server:
    /// headings, code, lines.
    Blocks,
}

impl Shows {
    pub const ALL: [Shows; 2] = [Shows::Pages, Shows::Blocks];

    /// Its name, as a manifest's `"shows"` says it.
    pub fn name(self) -> &'static str {
        match self {
            Shows::Pages => "pages",
            Shows::Blocks => "blocks",
        }
    }

    pub fn named(name: &str) -> Option<Shows> {
        Shows::ALL.into_iter().find(|shows| shows.name() == name)
    }

    /// Where a view of it can go: for now, pages are a tab and blocks are beside the code.
    pub fn place(self) -> ViewPlace {
        match self {
            Shows::Pages => ViewPlace::Tab,
            Shows::Blocks => ViewPlace::Beside,
        }
    }

    /// Whether there's no text for the code's keys to move through: on pages there's
    /// the text they're of.
    pub fn without_text(self) -> bool {
        self == Shows::Blocks
    }
}

/// Where a plugin's view is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewPlace {
    /// A tab of its own, in a pane as the code is, for the file in view (beside it, in the
    /// other pane of a split, if you ask).
    Tab,
    /// Right of the code.
    Beside,
}

impl ViewPlace {
    pub fn name(self) -> &'static str {
        match self {
            ViewPlace::Tab => "tab",
            ViewPlace::Beside => "beside",
        }
    }

    pub fn named(name: &str) -> Option<ViewPlace> {
        [ViewPlace::Tab, ViewPlace::Beside].into_iter().find(|place| place.name() == name)
    }
}

/// A view a plugin declares, for files of its own languages:
/// `{ "name": "goals", "title": "Goals", "shows": "blocks", "place": "beside", "key": "space g" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginView {
    pub id: ViewId,
    /// What the mode line and the cheatsheet call it.
    pub title: &'static str,
    pub place: ViewPlace,
    /// `space` and a letter that goes there and back, if it has one.
    pub key: Option<&'static str>,
}

impl PluginView {
    pub fn kind(self) -> ViewKind {
        ViewKind::Plugin(self.id)
    }
}

impl Modal {
    /// A key in a view with no text. Tab during a review accepts, as it does
    /// anywhere; Enter and Esc are the view's own.
    pub fn view_key(&mut self, key: Key) -> Outcome {
        if self.mode.reviewing() && self.pending.is_none() && key == Key::Tab {
            self.stopping = false;
            return Outcome::Review(Review::Next);
        }
        let mut keys = match self.pending.take() {
            Some(Pending::Keys(keys)) => keys,
            _ => vec![],
        };
        if keys.is_empty() && self.count_digit(key) {
            return Outcome::Handled;
        }
        let at_rest = keys.is_empty();
        keys.push(key);
        match self.keymap.find_in(self.view, &keys) {
            Found::Keys => {
                self.pending = Some(Pending::Keys(keys));
                Outcome::Handled
            }
            Found::Bound(Bound::Command(command)) => self.run_in_view(*command),
            Found::Bound(Bound::View(name)) => {
                (self.count, self.pending) = (None, None);
                Outcome::Effect(Effect::FocusView(name))
            }
            Found::Bound(Bound::Typed(line)) if !self.mode.reviewing() => {
                self.count = None;
                Outcome::Effect(Effect::Run(line))
            }
            _ => {
                self.count = None;
                // Other ctrl keys aren't modal editing's: the app may have them.
                if at_rest && matches!(key, Key::Ctrl(_)) { Outcome::Ignored } else { Outcome::Handled }
            }
        }
    }

    /// `command`, in a view with no text: the view's own, or one that works anywhere.
    fn run_in_view(&mut self, command: Command) -> Outcome {
        if let Some(outcome) = self.view_command(command) {
            return outcome;
        }
        let waits = self.mode.reviewing() && command.waits_for_review();
        if command.anywhere() && !waits {
            let count = self.count.take().unwrap_or(1);
            if let Some(outcome) = self.effect_of(command, count) {
                return outcome;
            }
        }
        self.count = None;
        Outcome::Handled
    }

    /// What a command that's about views does, in the view with the keys: `None`
    /// for the rest, which are the code's.
    pub(super) fn view_command(&mut self, command: Command) -> Option<Outcome> {
        use Command::*;
        let files = self.view == ViewKind::Files;
        let tree = |command: TreeCommand| Some(Outcome::Tree(command));
        let count = self.count.unwrap_or(1);
        let outcome = match command {
            // From select mode, Esc is first back to normal mode, as it is in the code.
            FocusCode if self.mode == Mode::Select => {
                self.mode = Mode::Normal;
                Some(Outcome::Handled)
            }
            FocusCode => Some(Outcome::Effect(Effect::FocusCode)),
            _ if self.view == ViewKind::Terminal && command.types() => {
                if self.mode == Mode::Select {
                    self.mode = Mode::Normal;
                }
                Some(Outcome::Effect(Effect::TypeInTerminal))
            }
            FilesDown if files => tree(TreeCommand::Down(count)),
            FilesUp if files => tree(TreeCommand::Up(count)),
            FilesFirst if files => tree(TreeCommand::First),
            FilesLast if files => tree(TreeCommand::Last),
            ExpandFolder if files => tree(TreeCommand::Expand),
            CollapseFolder if files => tree(TreeCommand::Collapse),
            OpenFile if files => tree(TreeCommand::Open),
            AddFile if files => tree(TreeCommand::Add),
            RenameFile if files => tree(TreeCommand::Rename),
            DeleteFile if files => tree(TreeCommand::Delete),
            FilesDown | FilesUp | FilesFirst | FilesLast | ExpandFolder | CollapseFolder | OpenFile | AddFile | RenameFile | DeleteFile => {
                Some(Outcome::Handled)
            }
            ScrollDown | ScrollUp => Some(Outcome::Effect(Effect::Scroll { by: if command == ScrollDown { 1 } else { -1 } * count as i32, pages: false })),
            ScrollPageDown | ScrollPageUp => {
                Some(Outcome::Effect(Effect::Scroll { by: if command == ScrollPageDown { 1 } else { -1 } * count as i32, pages: true }))
            }
            ZoomIn => Some(Outcome::Effect(Effect::Zoom(1))),
            ZoomOut => Some(Outcome::Effect(Effect::Zoom(-1))),
            ZoomReset => Some(Outcome::Effect(Effect::Zoom(0))),
            _ => None,
        };
        if outcome.is_some() {
            (self.count, self.pending) = (None, None);
        }
        outcome
    }

    /// Run `command` as if a key bound to it had been pressed (a plugin's key, in its
    /// view), in whichever view has the keys.
    pub fn run_named(&mut self, command: Command, regs: &mut dyn super::Registers, t: &mut super::Target) -> Outcome {
        self.added.clear();
        if self.view.without_text() {
            return self.run_in_view(command);
        }
        t.buffer.begin(*t.selection);
        let outcome = self.command(command, vec![], regs, t);
        if self.mode != Mode::Insert {
            t.buffer.commit(*t.selection);
        }
        outcome
    }
}
