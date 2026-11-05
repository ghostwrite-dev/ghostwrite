//! The keys of the views that aren't the code. Each view's own keys are a layer of
//! the keymap over the code's ([`super::keymap::VIEWS`]): `j` is a row down in Files,
//! Enter opens the file a line names on the terminal, Esc goes back to the code.
//!
//! A view with no text (Files, what's beside a file) has nothing for the code's
//! keys to move through: there, only its own keys and those that work anywhere
//! (`:`, and most under `space`) do something, and the rest are swallowed.

use super::command::Command;
use super::keymap::{Bound, Found};
use super::{Effect, Key, Modal, Mode, Outcome, Pending, Review, TreeCommand, ViewKind};

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
