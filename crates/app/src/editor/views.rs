//! Views: the places on screen that take the keys. The code (either pane of a split),
//! Files (the tree, left of it), the Terminal (under it), the Debugger (right of the
//! code), and the views plugins declare for files of their languages: a tab of a file's
//! pages, which is in a pane as code is, or what's beside a file, right of the code.
//! The editor knows no plugin's view but by what it declares ([`PluginView`]). The modes
//! are the same in each; which view has the keys is
//! [`Modal::view`](ghostwrite_core::Modal::view), and its layer of the keymap comes
//! first.
//!
//! `space` and a letter is the one way from view to view: `space e` goes to Files,
//! `space t` to the terminal, `space x` to the debugger, and a plugin's view's key (or
//! yours, bound to `focus_view` and its name) to that view of the file in front,
//! opening it; each again, from its view, goes back to the code. A click puts the keys
//! where it is. Whatever was going on in a view ends as the keys leave it: typing
//! (insert mode) and selecting (select mode) in the code, typing in the shell, naming
//! a file. A story's AI mode goes with the keys.

use ghostwrite_core::{Mode, PluginView, Shows, ViewId, ViewKind, ViewPlace};
use gpui::{Context, ScrollStrategy};

use super::Editor;
use super::prompt::PromptKind;

/// How far a key scrolls pages, what's beside a file, or the debugger, in points.
pub(super) const LINE: f32 = 40.;

/// Whether a plugin's view `id` is in a tab, as a file's pages are.
fn in_a_tab(id: ViewId) -> bool {
    id.shows.place() == ViewPlace::Tab
}

impl Editor {
    /// Whether the keys are on the code: a file's text, or a tab of its pages.
    pub(super) fn in_code(&self) -> bool {
        match self.modal.view {
            ViewKind::Code => true,
            ViewKind::Plugin(id) => in_a_tab(id),
            _ => false,
        }
    }

    /// Typing in the terminal's shell: every key but ⌘ ones goes there.
    pub(super) fn typing_in_terminal(&self) -> bool {
        self.modal.view == ViewKind::Terminal && self.modal.mode == Mode::Insert
    }

    /// Whether the debugger's up: something's being debugged, beside a file.
    pub(super) fn debugger_showing(&self) -> bool {
        self.tab().is_some() && self.debug.session.is_some()
    }

    /// Whether there's something beside the code: a plugin's panel, or the debugger.
    #[cfg(test)]
    pub(super) fn aside_showing(&self) -> bool {
        self.shown_beside().is_some() || self.debugger_showing()
    }

    /// The plugin's view beside the file in front, if it's shown.
    fn shown_beside(&self) -> Option<PluginView> {
        self.tab()?.doc.aside.as_ref().filter(|_| self.project.aside_shown).map(|aside| aside.view)
    }

    /// What `view` is called in the mode line, if it isn't its name: a plugin's, by its title.
    pub(super) fn view_title(&self, view: ViewKind) -> Option<&'static str> {
        let ViewKind::Plugin(id) = view else { return None };
        let beside = self.tab().and_then(|tab| tab.doc.aside.as_ref()).map(|aside| aside.view).filter(|view| view.id == id);
        Some(beside.or_else(|| ghostwrite_plugin::view(id))?.title)
    }

    /// The view a tab of pages is: the view of pages of the file they're of, from its plugin.
    fn pages_view(&self) -> ViewKind {
        let view = ghostwrite_plugin::view_showing(Shows::Pages, self.active_language()).map(|view| view.id);
        // A tab of pages no plugin's view has any more is still pages.
        ViewKind::Plugin(view.unwrap_or(ViewId { plugin: "", name: "pages", shows: Shows::Pages }))
    }

    /// The view the keys are on, as things are now: a tab of pages is a view of its own,
    /// and a view that's gone (hidden, closed) leaves them on the code.
    pub(super) fn view_now(&self) -> ViewKind {
        if self.in_code() {
            let pages = self.tab().is_some_and(|tab| tab.doc.view_of.is_some());
            return if pages { self.pages_view() } else { ViewKind::Code };
        }
        match self.modal.view {
            ViewKind::Files if !self.tree_pane.shown => ViewKind::Code,
            ViewKind::Terminal if !self.terminal.shown || self.terminal.shells.is_empty() => ViewKind::Code,
            ViewKind::Debugger if !self.debugger_showing() => ViewKind::Code,
            ViewKind::Plugin(id) if self.shown_beside().is_none_or(|beside| beside.id != id) => ViewKind::Code,
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
        // A tab of pages is the code's pane's: which tab's in front says which it is.
        let to = if matches!(to, ViewKind::Plugin(id) if in_a_tab(id)) { ViewKind::Code } else { to };
        if self.modal.view == to || to == ViewKind::Code && self.in_code() {
            return;
        }
        match self.modal.view {
            _ if self.in_code() => self.leave_insert(),
            ViewKind::Files => self.stop_naming(),
            ViewKind::Terminal => {
                if self.modal.mode == Mode::Insert {
                    self.terminal.held_escape = None;
                    self.terminal.held_quit = None;
                    self.modal.leave(Mode::Insert);
                }
                self.stop_reading();
            }
            _ => {}
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

    /// `focus_view name`, and the key a plugin's view declares: the keys to the view of
    /// the file in front called `name`, opened if it isn't shown yet; from there, back to
    /// the code. A file with no such view says whose it is.
    pub(super) fn focus_view(&mut self, name: &str, cx: &mut Context<Self>) {
        let now = self.view_now();
        if let ViewKind::Plugin(id) = now
            && id.name == name
            && !in_a_tab(id)
        {
            return self.focus_on(ViewKind::Code, cx);
        }
        // What's beside the file already, or a view its plugin has of it.
        let beside = self.tab().and_then(|tab| tab.doc.aside.as_ref()).map(|aside| aside.view).filter(|view| view.id.name == name);
        let pages = match now {
            ViewKind::Plugin(id) if id.name == name => Some(PluginView { id, title: id.name, place: ViewPlace::Tab, key: None }),
            _ => None,
        };
        let Some(view) = beside.or(pages).or_else(|| ghostwrite_plugin::view_named(name, self.active_language())) else {
            let mut whose: Vec<&str> = ghostwrite_plugin::views().iter().filter(|view| view.id.name == name).map(|view| view.id.plugin).collect();
            whose.dedup();
            let said = match whose.as_slice() {
                [] => format!("there's no {name} view: no plugin has one"),
                [one] => format!("this file has no {name} view: {one} has one, for its own files"),
                [many @ .., last] => format!("this file has no {name} view: {} and {last} have one, for their own files", many.join(", ")),
            };
            return self.project.say(said);
        };
        match view.place {
            ViewPlace::Tab => self.go_to_pages(view, cx),
            ViewPlace::Beside => self.go_beside(view, cx),
        }
    }

    /// To the tab of the file's pages, which opens if it isn't shown yet; from the pages,
    /// back to the file.
    fn go_to_pages(&mut self, view: PluginView, cx: &mut Context<Self>) {
        let elsewhere = !self.in_code();
        self.focus_on(ViewKind::Code, cx);
        // From another view to a tab of pages that's in front already: there.
        if elsewhere && self.view_now() == view.kind() {
            return;
        }
        if self.tab().is_some_and(|tab| tab.doc.pages_are_the_text()) {
            return self.project.say("this file is its pages");
        }
        match self.preview_pages(cx) {
            Ok(true) => {}
            Ok(false) => self.project.say(format!("this file has no {}: its plugin makes none of it", view.id.name)),
            Err(why) => self.project.say(why),
        }
        self.modal.view = self.view_now();
        cx.notify();
    }

    /// To what the plugin shows beside the file, which shows if it isn't yet.
    fn go_beside(&mut self, view: PluginView, cx: &mut Context<Self>) {
        if let Some(tab) = self.project.tabs.active_mut() {
            if tab.doc.aside.as_ref().is_none_or(|aside| aside.view.id != view.id)
                && let Some(panel) = tab.doc.language.and_then(|language| ghostwrite_plugin::panel_of(view.id, language))
            {
                tab.doc.aside = Some(super::aside::Aside::new(view, panel));
            }
            if tab.doc.aside.is_some() {
                self.project.aside_shown = true;
                self.refresh_aside(cx);
            }
        }
        if self.shown_beside().is_none_or(|beside| beside.id != view.id) {
            return self.project.say(format!("this file has nothing in its {} view: its plugin shows nothing of it", view.id.name));
        }
        self.focus_on(view.kind(), cx);
    }

    /// `space x`: the keys to the debugger, while something's being debugged; from
    /// there, back to the code.
    pub(super) fn toggle_debugger(&mut self, cx: &mut Context<Self>) {
        if self.modal.view == ViewKind::Debugger {
            return self.focus_on(ViewKind::Code, cx);
        }
        if !self.debugger_showing() {
            return self.project.say("nothing's being debugged: :debug path/to/program");
        }
        self.focus_on(ViewKind::Debugger, cx);
    }

    /// `j`, `k`, `ctrl-d`, `ctrl-u` on pages, beside the file or in the debugger: `by`
    /// lines (or with `pages`, screens) down, or up if it's less than none.
    pub(super) fn scroll_view(&mut self, by: i32, pages: bool, cx: &mut Context<Self>) {
        let rows = if pages { self.measured.rows.max(2) as f32 } else { 1. };
        let down = LINE * rows * by as f32;
        let handle = match self.modal.view {
            ViewKind::Debugger => &self.debug.scroll,
            ViewKind::Plugin(id) if !in_a_tab(id) => &self.aside_scroll,
            _ => return self.scroll_pages(None, gpui::Point::new(gpui::px(0.), gpui::px(-down)), cx),
        };
        let (offset, most) = (handle.offset(), handle.max_offset().height);
        handle.set_offset(gpui::point(offset.x, (offset.y - gpui::px(down)).clamp(-most.max(gpui::px(0.)), gpui::px(0.))));
        cx.notify();
    }
}
