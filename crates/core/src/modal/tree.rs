//! Tree mode: keys drive the file tree.

use super::{Effect, Key, Modal, Mode, Outcome, Pending, TreeCommand};

impl Modal {
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
            (Some(Pending::Space), Key::Char('a')) => return Outcome::Effect(Effect::Assist),
            (Some(Pending::Space), Key::Char('t')) => return Outcome::Effect(Effect::Terminal),
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
            (None, Key::Escape) => TreeCommand::Leave,
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
}
