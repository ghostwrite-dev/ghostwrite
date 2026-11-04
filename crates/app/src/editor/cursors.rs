//! More than one cursor, as in helix: `C` copies each selection to the line
//! below (`alt-C` above), `s` makes one of each match of a regex in the
//! selection, and `,` goes back to one (`alt-,`: all but the main one). `(`
//! and `)` make another the main one, `alt-minus` and `alt-_` merge them, and
//! `&` lines them up. A key is carried out at each of them, the last in the
//! text first, so that an edit only moves those already done, and all of it is
//! one undo; selections a key makes besides (`3o`, `alt-s`) join them. What a
//! key does once (`:`, `u`, a jump to another file) stays with the main
//! cursor, which is the one the view follows.

use ghostwrite_core::{Effect, Key, Mode};
use gpui::Context;
use regex::Regex;

pub(super) use ghostwrite_view::cursors::key_at_each;

use super::{Editor, Tab};

impl Editor {
    /// Text typed at every cursor: `insert` does it at the one it's given.
    pub(super) fn typed_at_each(&mut self, insert: impl FnMut(&mut ghostwrite_core::Modal, &mut Tab)) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        tab.typed_at_each(&mut self.modal, insert);
    }

    /// `f` done to the active tab, if there is one.
    fn with_tab(&mut self, f: impl FnOnce(&mut Tab)) {
        if let Some(tab) = self.project.tabs.active_mut() {
            f(tab);
        }
    }

    /// Esc with more than one cursor: in select mode it ends it, keeping the
    /// cursors, as in helix; in normal mode it's back to one.
    pub(super) fn cursor_key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if !matches!(self.modal.mode, Mode::Normal | Mode::Select) || !self.modal.pending_keys().is_empty() || key != Key::Escape {
            return false;
        }
        if !self.tab().is_some_and(|tab| !tab.view.others.is_empty()) {
            return false;
        }
        match self.modal.mode {
            Mode::Select => self.modal.mode = Mode::Normal,
            _ => self.with_tab(Tab::one_cursor),
        }
        cx.notify();
        true
    }

    /// A command about the cursors themselves.
    pub(super) fn cursors(&mut self, effect: Effect, window: &mut gpui::Window, cx: &mut Context<Self>) {
        match effect {
            Effect::CopySelections { below, count } => self.with_tab(|tab| tab.copy_selections(below, count)),
            Effect::KeepPrimarySelection => self.with_tab(Tab::one_cursor),
            Effect::RemovePrimarySelection => self.drop_main(),
            Effect::RotateSelections { forward, count } => self.with_tab(|tab| tab.rotate(forward, count)),
            Effect::MergeSelections { touching } => self.with_tab(|tab| tab.merge(touching)),
            Effect::AlignSelections => self.align(),
            Effect::SelectRegex if self.tab().is_some_and(|tab| !tab.doc.read_only()) => self.open_prompt(super::PromptKind::SelectIn, "", window, cx),
            _ => {}
        }
    }

    /// `alt-,`: all but the main cursor; the next one is the main one.
    fn drop_main(&mut self) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        if let Err(why) = tab.drop_main() {
            self.project.say(why);
        }
    }

    /// `&`: the selections lined up in columns (see [`Tab::align`]).
    fn align(&mut self) {
        let Some(tab) = self.project.tabs.active_mut().filter(|tab| !tab.doc.read_only()) else { return };
        match tab.align() {
            Ok(()) => self.modal.mode = Mode::Normal,
            Err(why) => self.project.say(why),
        }
    }

    /// `s`: a cursor on each match of `typed` in the selection, or in the whole file if there isn't one.
    pub(super) fn select_matches(&mut self, typed: &str) {
        let pattern = match Regex::new(typed).or_else(|_| Regex::new(&regex::escape(typed))) {
            Ok(pattern) => pattern,
            Err(e) => return self.project.say(e.to_string()),
        };
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let Some(count) = tab.select_matches(&pattern) else {
            return self.project.say(format!("no matches for {typed}"));
        };
        self.project.say(format!("{count} cursor{}: , goes back to one", if count == 1 { "" } else { "s" }));
    }
}
