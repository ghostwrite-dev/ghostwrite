//! Views: the places on screen that take the keys. The code (either pane of a split),
//! Files (the tree, left of it), the Terminal (under it), and a plugin's views: a tab
//! of pages, which is in a pane as code is, and what's beside a file, right of the
//! code. The modes are the same in each; which view has the keys is
//! [`Modal::view`](ghostwrite_core::Modal::view), and its layer of the keymap comes
//! first.
//!
//! `ctrl-w h j k l` go from view to view by where they are on screen; `space e` and
//! `space t` go to Files and the terminal, and again, back to the code; a click
//! puts the keys where it is. Whatever was going on in a view ends as the keys
//! leave it: typing (insert mode) and selecting (select mode) in the code, typing
//! in the shell, naming a file. A story's AI mode goes with the keys.

use ghostwrite_core::{Direction, Mode, ViewKind};
use gpui::{Context, ScrollStrategy};

use super::Editor;
use super::prompt::PromptKind;

/// How far a key scrolls pages, or what's beside a file, in points.
pub(super) const LINE: f32 = 40.;

impl Editor {
    /// Whether the keys are on the code: a file's text, or a tab of pages.
    pub(super) fn in_code(&self) -> bool {
        matches!(self.modal.view, ViewKind::Code | ViewKind::Pages)
    }

    /// Typing in the terminal's shell: every key but ⌘ ones goes there.
    pub(super) fn typing_in_terminal(&self) -> bool {
        self.modal.view == ViewKind::Terminal && self.modal.mode == Mode::Insert
    }

    /// Whether a plugin shows something beside the file in front.
    pub(super) fn aside_showing(&self) -> bool {
        self.project.aside_shown && self.tab().is_some_and(|tab| tab.doc.aside.is_some())
    }

    /// What the view with the keys is called in the mode line, if it isn't its kind:
    /// a plugin's view, by its title.
    pub(super) fn view_title(&self) -> Option<&'static str> {
        match self.modal.view {
            ViewKind::Aside => Some(self.tab()?.doc.aside.as_ref()?.panel.title()),
            _ => None,
        }
    }

    /// The view the keys are on, as things are now: a tab of pages is a view of its own,
    /// and a view that's gone (hidden, closed) leaves them on the code.
    pub(super) fn view_now(&self) -> ViewKind {
        let pages = self.tab().is_some_and(|tab| tab.doc.view_of.is_some());
        match self.modal.view {
            ViewKind::Code | ViewKind::Pages if pages => ViewKind::Pages,
            ViewKind::Code | ViewKind::Pages => ViewKind::Code,
            ViewKind::Files if !self.tree_pane.shown => ViewKind::Code,
            ViewKind::Terminal if !self.terminal.shown || self.terminal.shells.is_empty() => ViewKind::Code,
            ViewKind::Aside if !self.aside_showing() => ViewKind::Code,
            view => view,
        }
    }

    /// Before a key: the keys are where [`Self::view_now`] says, and on the terminal,
    /// out of typing, there's its text to move through.
    pub(super) fn settle_view(&mut self, cx: &mut Context<Self>) {
        let now = self.view_now();
        // The view the keys were on went away.
        if now == ViewKind::Code && !self.in_code() {
            return self.focus_on(ViewKind::Code, cx);
        }
        self.modal.view = now;
        if now == ViewKind::Terminal && self.modal.mode != Mode::Insert && self.terminal.reading.is_none() {
            self.start_reading(false, cx);
        }
    }

    /// The keys go to `to`, in the mode they're in. What was going on where they were
    /// ends: typing, selecting, naming a file. (Into the terminal, there's nothing to
    /// read yet: see [`Self::start_reading`] and [`Self::type_in_terminal`].)
    pub(super) fn focus_on(&mut self, to: ViewKind, cx: &mut Context<Self>) {
        let to = if to == ViewKind::Pages { ViewKind::Code } else { to };
        if self.modal.view == to || to == ViewKind::Code && self.in_code() {
            return;
        }
        match self.modal.view {
            ViewKind::Code | ViewKind::Pages => self.leave_insert(),
            ViewKind::Files => self.stop_naming(),
            ViewKind::Terminal => {
                if self.modal.mode == Mode::Insert {
                    self.terminal.held_escape = None;
                    self.terminal.held_quit = None;
                    self.modal.leave(Mode::Insert);
                }
                self.stop_reading();
            }
            ViewKind::Aside => {}
        }
        self.stop_selecting();
        self.modal.cancel_pending();
        self.modal.view = to;
        self.modal.view = self.view_now();
        cx.notify();
    }

    /// A file being named in Files (added or renamed) isn't any more.
    fn stop_naming(&mut self) {
        if self.prompt.as_ref().is_some_and(|p| matches!(p.kind, PromptKind::TreeAdd(_) | PromptKind::TreeRename(_))) {
            self.prompt = None;
            self.modal.leave(Mode::Insert);
            self.refocus = true;
        }
    }

    /// `space e`, ⌘B: Files has the keys, on the file in front.
    pub(super) fn enter_files(&mut self, cx: &mut Context<Self>) {
        self.tree_pane.shown = true;
        self.focus_on(ViewKind::Files, cx);
        self.project.tree.set_active(self.project.active_path());
        if let Some(path) = self.project.active_path() {
            self.project.tree.reveal(&path);
        }
        self.scroll_tree_to_cursor(ScrollStrategy::Center);
        cx.notify();
    }

    /// `ctrl-w h j k l`: the keys to the view that way on screen, if there's one there.
    /// Files is left of the code, the terminal under it (and what's beside it), and
    /// what a plugin shows beside the file right of it; the panes of a split are side by side.
    pub(super) fn focus_towards(&mut self, towards: Direction, cx: &mut Context<Self>) {
        enum To {
            View(ViewKind),
            OtherPane,
        }
        let split = self.other_pane_doc().and(self.split.as_ref());
        // Which side the pane with the keys is on, if there are two.
        let (keys_left, keys_right) = (split.is_some_and(|s| s.other_on_right), split.is_some_and(|s| !s.other_on_right));
        let (files, aside) = (self.tree_pane.shown, self.aside_showing());
        let terminal = self.terminal.shown && !self.terminal.shells.is_empty();
        let to = match (self.in_code(), self.modal.view, towards) {
            (true, _, Direction::Left) if keys_right => To::OtherPane,
            (true, _, Direction::Left) if files => To::View(ViewKind::Files),
            (true, _, Direction::Right) if keys_left => To::OtherPane,
            (true, _, Direction::Right) if aside => To::View(ViewKind::Aside),
            (true, _, Direction::Down) if terminal => To::View(ViewKind::Terminal),
            // From Files, the pane next to it; from beside the file, the one next to that.
            (_, ViewKind::Files, Direction::Right) if keys_right => To::OtherPane,
            (_, ViewKind::Files, Direction::Right) => To::View(ViewKind::Code),
            (_, ViewKind::Aside, Direction::Left) if keys_left => To::OtherPane,
            (_, ViewKind::Aside, Direction::Left) => To::View(ViewKind::Code),
            (_, ViewKind::Aside, Direction::Down) if terminal => To::View(ViewKind::Terminal),
            (_, ViewKind::Terminal, Direction::Up) => To::View(ViewKind::Code),
            (_, ViewKind::Terminal, Direction::Left) if files => To::View(ViewKind::Files),
            _ => return,
        };
        match to {
            To::OtherPane => {
                self.focus_on(ViewKind::Code, cx);
                self.other_pane(cx);
            }
            To::View(ViewKind::Terminal) => self.start_reading(false, cx),
            To::View(view) => self.focus_on(view, cx),
        }
        cx.notify();
    }

    /// `j`, `k`, `ctrl-d`, `ctrl-u` on pages or beside the file: `by` lines (or with
    /// `pages`, screens) down, or up if it's less than none.
    pub(super) fn scroll_view(&mut self, by: i32, pages: bool, cx: &mut Context<Self>) {
        let rows = if pages { self.measured.rows.max(2) as f32 } else { 1. };
        let down = LINE * rows * by as f32;
        match self.modal.view {
            ViewKind::Aside => {
                let handle = &self.aside_scroll;
                let (offset, most) = (handle.offset(), handle.max_offset().height);
                handle.set_offset(gpui::point(offset.x, (offset.y - gpui::px(down)).clamp(-most.max(gpui::px(0.)), gpui::px(0.))));
                cx.notify();
            }
            _ => self.scroll_pages(None, gpui::Point::new(gpui::px(0.), gpui::px(-down)), cx),
        }
    }
}
