//! Tree mode: keys drive the file tree. Its few keys are its own, not the
//! keymap's: `g` and `space` lead on as they do in normal mode.

use super::{Effect, Key, Modal, Outcome, Pending, TreeCommand};

impl Modal {
    /// A key in tree mode. `:` and the keys under `space` work as in
    /// normal mode; `space e` leaves.
    pub fn tree_key(&mut self, key: Key) -> Outcome {
        // `g` or `space`, typed before.
        let pending = match self.pending.take() {
            Some(Pending::Keys(keys)) => Some(keys.first().copied()),
            Some(_) => Some(None),
            None => None,
        };
        const GOTO: Option<Key> = Some(Key::Char('g'));
        const SPACE: Option<Key> = Some(Key::Char(' '));
        if pending.is_none() && self.count_digit(key) {
            return Outcome::Handled;
        }
        let count = self.count.take().unwrap_or(1);
        let command = match (pending, key) {
            (Some(GOTO), Key::Char('g')) => TreeCommand::First,
            (Some(GOTO), Key::Char('e')) => TreeCommand::Last,
            (Some(SPACE), Key::Char('e')) => TreeCommand::Leave,
            (Some(SPACE), Key::Char('a')) => return Outcome::Effect(Effect::Assist),
            (Some(SPACE), Key::Char('t')) => return Outcome::Effect(Effect::Terminal),
            (Some(SPACE), Key::Char('/')) => return Outcome::Effect(Effect::Search),
            (Some(SPACE), Key::Char('f')) => return Outcome::Effect(Effect::PickFile),
            (Some(SPACE), Key::Char('b')) => return Outcome::Effect(Effect::PickBuffer),
            (Some(SPACE), Key::Char('?')) => return Outcome::Effect(Effect::Cheatsheet),
            (Some(_), _) => return Outcome::Handled,
            (None, Key::Char('g')) => {
                self.pending = Some(Pending::Keys(vec![key]));
                return Outcome::Handled;
            }
            (None, Key::Char(' ')) => {
                self.pending = Some(Pending::Keys(vec![key]));
                return Outcome::Handled;
            }
            (None, Key::Char(':')) => return Outcome::Effect(Effect::CommandLine),
            (None, Key::Char('j') | Key::Down) => TreeCommand::Down(count),
            (None, Key::Char('k') | Key::Up) => TreeCommand::Up(count),
            (None, Key::Char('G') | Key::End) => TreeCommand::Last,
            (None, Key::Home) => TreeCommand::First,
            (None, Key::Char('l') | Key::Right) => TreeCommand::Expand,
            (None, Key::Char('h') | Key::Left) => TreeCommand::Collapse,
            (None, Key::Enter) => TreeCommand::Open,
            (None, Key::Escape) => TreeCommand::Leave,
            (None, Key::Char('a')) => TreeCommand::Add,
            (None, Key::Char('r')) => TreeCommand::Rename,
            (None, Key::Char('d')) => TreeCommand::Delete,
            (None, Key::Ctrl(_) | Key::Alt(_)) => return Outcome::Ignored,
            (None, _) => return Outcome::Handled,
        };
        if command == TreeCommand::Leave {
            self.leave_tree();
        }
        Outcome::Tree(command)
    }
}
