//! The editor on screen: the file tree beside the tabs and the code (and
//! what's beside a file, beside that: see [`super::aside`]), the terminal
//! and the `:` line under them, and helix's status line at the bottom.
//! Also the panes' edges being dragged, and the menu actions, which act on
//! the active document.

use ghostwrite_core::{Mode, Range};
use gpui::{AnyElement, ClipboardItem, Context, FontWeight, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, SharedString, Window, div, prelude::*};

use super::element::{CodeView, Decoration};
use super::{Autoscroll, Editor, PromptKind, Tab};
use crate::actions::*;
use crate::theme::{pt, theme};

/// A tab being dragged in the tab bar, and how it looks on the way.
#[derive(Clone)]
struct DraggedTab {
    ix: usize,
    name: gpui::SharedString,
}

impl gpui::Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .px_3()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tab_active)
            .text_color(theme.tab_active_foreground)
            .text_sm()
            .child(self.name.clone())
    }
}

/// The `space ?` cheatsheet: every key. Helix's keys are as in helix;
/// Ghostwrite's own are under `space`; ⌘ is for the app and its windows.
pub(super) const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("h j k l", "left down up right"),
            ("w b e", "next word, back, word end (W B E: WORD)"),
            ("f t ‹c›  F T", "find / till a char (F T: back)"),
            ("gg ge  G 10G", "file start, last line; line 10"),
            ("gh gl gs", "line start, end, first non-blank"),
            ("gt gc gb", "top, middle, bottom line on screen"),
            ("zz zt zb", "scroll: this line to the middle, top, bottom"),
            ("]d [d", "next, previous diagnostic"),
            ("ctrl-d ctrl-u", "half page down, up"),
            ("gn gp", "next, previous tab (4gn: four along)"),
            ("gN gP", "move the tab along (4gN: four places)"),
            ("gd  gr", "go to definition; through the references"),
            ("ctrl-o ctrl-i", "back where a jump came from, forward"),
        ],
    ),
    (
        "Select",
        &[
            ("v", "select mode (motions extend)"),
            ("C  alt-C", "another cursor below, above"),
            ("s", "a cursor on each match in the selection"),
            (",  esc", "back to one cursor"),
            ("click  ⇧click", "move there; select to there (or click after v)"),
            ("drag  2×  3× click", "select; word; line"),
            ("x X  %", "select line (again: next line), out to whole lines; all"),
            (";  alt-;", "collapse, flip selection"),
        ],
    ),
    (
        "Edit",
        &[
            ("i a I A", "insert before, after, line start, end"),
            ("o O", "open line below, above"),
            ("d c", "delete, change selection"),
            ("y p P  R", "yank, paste after, before; replace with yanked"),
            ("\"‹r›", "use register r for the next y d c p (+: clipboard)"),
            ("r‹c›  ~", "replace with char, switch case"),
            ("> <  J", "indent, unindent; join lines"),
            ("ctrl-a ctrl-x  ctrl-c", "number up, down; comment out / in"),
            (".  Q q", "repeat the last change; record, play a macro"),
            ("u U", "undo, redo"),
            ("ctrl-w  ⌥⌫  ctrl-u", "(typing) delete the word, back to line start"),
        ],
    ),
    (
        "Match  m",
        &[
            ("mm", "matching bracket"),
            ("ms‹c›", "surround with a pair"),
            ("mr‹a›‹b›", "replace surrounding pair"),
            ("md‹c›", "delete surrounding pair"),
            ("mi‹o›  ma‹o›", "select inside / around: w W p ( [ { < ' \" ` m"),
        ],
    ),
    ("Search", &[("/  ?", "search the file, forward, back (esc goes back)"), ("n N", "next match, previous"), ("*", "search for the selection")]),
    (
        "Space",
        &[
            ("space a", "ask about the line or selection"),
            ("space e  ⌘B", "file tree"),
            ("space t  ⌘J", "terminal"),
            ("space f  space b", "pick a file, an open file (fuzzy)"),
            ("space /", "search the project"),
            ("space k  space d", "docs here; this file's diagnostics"),
            ("space r  space .", "rename; the server's code actions here"),
            ("space s", "pick one of the file's symbols"),
            ("space j", "next error (or warning); a file's pages follow"),
            ("space y  space p", "clipboard yank, paste"),
            ("space ?", "this, every key"),
            ("ctrl-w v", "two panes: this file's pages, or the next tab"),
            ("ctrl-w w  ctrl-w q", "the other pane; only this one"),
        ],
    ),
    (
        "Reviewing",
        &[
            ("tab", "accept the step · next note or match"),
            ("tab  tab", "a command: run it in the terminal; it's done"),
            ("enter  space a", "redirect, or ask about it"),
            ("esc esc", "stop, back where you were"),
            ("motions", "look around; no editing"),
        ],
    ),
    (
        "Command line  :",
        &[
            (":e ‹path›  :w  :wa", "open, write, write all"),
            (":w!  :wq  :x  :q", "write over changes on disk; write and close; close"),
            (":n :bn :bp :bc", "new, next, previous, close tab"),
            (":rl  :rm  :42", "reload, trash file, go to line"),
            (":format", "format the file (saving does too)"),
            (":tree :term :preview", "file tree, terminal, a file's pages or panel"),
            (":tab-left :tab-right", "move the tab (⌘⌥← ⌘⌥→, gN gP, or drag it)"),
            (":model opus-5.5", "ask that model from now on (tab completes)"),
            (":api-key ‹key›", "keep a Claude API key safe on this machine"),
            (":undo-story", "take back all of the last story's steps"),
            (":replace old new", "go through the project's matches as steps (! takes all)"),
            (":tutor  :update", "learn the keys; is there a newer version?"),
            (":vsplit  :only", "a second pane beside this one; only this one"),
            (":break  :debug ‹program›", "a breakpoint here; run it in the debugger"),
            (":over :step :finish", "a line, into a call, out of it"),
            (":continue  :stop", "on to the next breakpoint; stop debugging"),
            (":project  :qa", "projects, quit"),
            ("↑ ↓  tab", "history, complete"),
        ],
    ),
    (
        "Tree  space e",
        &[("j k  gg G", "move"), ("l h  enter", "open / preview, close, open file"), ("a r d", "add, rename, trash"), ("esc  space e", "back to the text")],
    ),
    (
        "Terminal  space t",
        &[
            ("", "your shell has the keys; the model reads it, and runs only what you accept"),
            ("esc esc", "back where you were (in vim & co, click the code)"),
            ("drag  ⌘C", "select, copy"),
            ("⌘J", "hide it"),
            ("⌘T  ⌘W", "another shell in a tab, close it"),
            ("⌘⇧[  ⌘⇧]", "previous, next tab"),
            ("⌘⌥←  ⌘⌥→", "move the tab, or drag it"),
        ],
    ),
    (
        "Window  ⌘",
        &[
            ("⌘N ⌘W", "new, close tab"),
            ("⌘⇧[ ⌘⇧]", "previous, next tab"),
            ("⌘O ⌘⇧O ⌘⇧N", "open file, folder; new window"),
            ("⌘S ⌘⇧S", "save, save as"),
            ("⌘= ⌘- ⌘0", "zoom in, out, actual size"),
        ],
    ),
];

impl Editor {
    /// With no file open: a few ways in.
    fn render_empty(&self, cx: &Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let hint = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .gap_3()
                .child(div().w(pt(96.)).text_right().font_family(theme.mono_font.clone()).text_color(theme.foreground).child(keys))
                .child(div().text_color(theme.muted_foreground).child(what))
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .child(hint("space e", "browse the file tree"))
            .child(hint(":e <path>", "open a file"))
            .child(hint(":n", "new buffer"))
            .child(hint(":project", "switch project"))
            .child(hint("space ?", "every key"))
            .into_any_element()
    }

    /// A pane's edge is being dragged: the mouse isn't selecting.
    pub(super) fn resizing(&self) -> bool {
        self.tree_pane.width.dragging() || self.terminal.height.dragging() || self.aside_width.dragging()
    }

    /// The mouse moved to `at`, maybe dragging a pane's edge.
    fn drag_edges(&mut self, at: gpui::Point<gpui::Pixels>, window: &Window, cx: &mut Context<Self>) {
        let zoom = theme(cx).zoom;
        let room = window.viewport_size().map(|d| f32::from(d) / zoom);
        let moved = self.tree_pane.width.drag(at, zoom, super::tree::WIDTHS)
            | self.aside_width.drag(at, zoom, super::aside::widths(room.width))
            | self.terminal.height.drag(at, zoom, super::terminal::TerminalPane::heights(room.height));
        if moved {
            cx.notify();
        }
    }

    /// The mouse was let go: the edge dragged, if any, stays there.
    fn release_edges(&mut self) {
        self.tree_pane.width.release();
        self.aside_width.release();
        self.terminal.height.release();
    }

    /// Bring what's derived from the model up to date for the active file,
    /// before it's drawn: its highlights, its server's colors, what's
    /// beside it, the matches marked. Whatever changed the model, this
    /// follows; the work is in the background, and draws again when done.
    fn refresh_derived(&mut self, cx: &mut Context<Self>) {
        self.tell_plugins_what_happened(cx);
        self.refresh_aside(cx);
        self.refresh_semantic(cx);
        self.refresh_highlights(cx);
        self.refresh_git(cx);
        // Pages in the other pane keep up with the file they're of.
        if let Some(file) = self.other_pane_doc().and_then(|doc| self.pages_of(doc)) {
            self.refresh_rendered(file, cx);
        }
        // A file that's only pages has them as soon as it's in view.
        if let Some(file) = self.tab().filter(|tab| tab.doc.is_only_pages()).map(|tab| tab.doc.id) {
            self.refresh_rendered(file, cx);
        }
        if self.find.shown {
            // Found again for the text as it is now, to mark.
            self.matches();
        }
        // The thinking dots move every 400 ms: draw again then, not every frame.
        if self.story.as_ref().is_some_and(|s| s.thinking()) && self.ticking.is_none() {
            self.ticking = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                this.update(cx, |this, cx| {
                    this.ticking = None;
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Work out where the active file differs from what git has, if it's changed:
    /// what git has once, until something may have moved it, and the rest as it's edited.
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let id = tab.doc.id;
        let Some(path) = tab.doc.path.clone().filter(|_| !tab.doc.read_only()) else { return };
        let git = &mut tab.doc.git;
        if !git.asked {
            git.asked = true;
            let read = cx.background_spawn(async move { ghostwrite_project::git::committed(&root, &path) });
            cx.spawn(async move |this, cx| {
                let base = read.await;
                this.update(cx, |this, cx| {
                    if let Some(tab) = this.project.tabs.by_id_mut(id) {
                        (tab.doc.git.base, tab.doc.git.made) = (base.map(Into::into), None);
                        if tab.doc.git.base.is_none() {
                            tab.doc.git.changes.clear();
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        let version = tab.doc.buffer.version();
        let Some(base) = tab.doc.git.base.clone().filter(|_| !tab.doc.git.making && tab.doc.git.made != Some(version)) else { return };
        tab.doc.git.making = true;
        let text = tab.doc.buffer.text().to_string();
        let work = cx.background_spawn(async move {
            ghostwrite_project::contained("comparing with git", || ghostwrite_project::git::changes(&base, &text)).unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let changes = work.await;
            this.update(cx, |this, cx| {
                if let Some(tab) = this.project.tabs.by_id_mut(id) {
                    (tab.doc.git.changes, tab.doc.git.made, tab.doc.git.making) = (changes, Some(version), false);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Work out the active file's grammar colors again, if its text changed.
    fn refresh_highlights(&mut self, cx: &mut Context<Self>) {
        let other = self.other_pane_doc();
        let active = self.tab().map(|tab| tab.doc.id);
        for doc in active.into_iter().chain(other) {
            self.refresh_highlights_of(doc, cx);
        }
    }

    fn refresh_highlights_of(&mut self, doc: super::DocId, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.by_id_mut(doc) else { return };
        let id = tab.doc.id;
        let Some((language, version, text)) = tab.doc.refresh_highlights() else { return };
        let work = cx.background_spawn(async move {
            // Without its colors the file can still be edited.
            let spans = ghostwrite_project::contained("coloring the file", || language.highlight(&text)).unwrap_or_default();
            (text, spans)
        });
        cx.spawn(async move |this, cx| {
            let (text, spans) = work.await;
            this.update(cx, |this, cx| {
                if let Some(tab) = this.project.tabs.by_id_mut(id) {
                    tab.doc.highlighted(language, version, text, spans);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx).clone();
        div()
            .id("tabs")
            .flex_none()
            .flex()
            .h(pt(30.))
            .overflow_x_scroll()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .children(self.project.tabs.iter().enumerate().map(|(ix, tab)| {
                let (doc, active) = (&tab.doc, ix == self.project.tabs.active_index());
                div()
                    .id(("tab", ix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.tab_active).text_color(theme.tab_active_foreground))
                    .when(!active, |d| d.text_color(theme.tab_foreground))
                    .debug_selector(move || format!("tab-{ix}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx)))
                    .on_drag(DraggedTab { ix, name: doc.name() }, |tab, _, _, cx| cx.new(|_| tab.clone()))
                    .drag_over::<DraggedTab>({
                        let accent = theme.accent;
                        move |style, _, _, _| style.border_l_2().border_color(accent)
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| this.move_tab(dragged.ix, ix, cx)))
                    .child(doc.name())
                    .child(
                        div()
                            .id(("close", ix))
                            .px_1()
                            .rounded_sm()
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.muted).text_color(theme.foreground))
                            .child(if doc.dirty { "●" } else { "×" })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.activate(ix, cx);
                                this.close_tab(window, cx);
                            })),
                    )
            }))
            .into_any_element()
    }

    /// The `:` prompt: its own line above the status line, in the editor font.
    fn render_command_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let prompt =
            self.prompt.as_ref().filter(|p| matches!(p.kind, PromptKind::Command | PromptKind::Find { .. } | PromptKind::Rename | PromptKind::SelectIn))?;
        let lead = match prompt.kind {
            PromptKind::Find { backward: true } => "?",
            PromptKind::Find { backward: false } => "/",
            PromptKind::Rename => "rename to ",
            PromptKind::SelectIn => "select ",
            _ => ":",
        };
        let theme = theme(cx);
        Some(
            div()
                .flex_none()
                .flex()
                .items_start()
                .min_h((theme.mono_size * 2.).round())
                .px_3()
                .py((theme.mono_size * 0.5).round())
                .font_family(theme.mono_font.clone())
                .text_size(theme.mono_size)
                .child(lead)
                .child(div().flex_1().child(prompt.field.clone()))
                .into_any_element(),
        )
    }

    /// Helix's status line: mode and message on the left, keys typed so far
    /// and the position on the right, on one line that never wraps.
    fn render_status(&self, cx: &Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let mode = self.modal.mode;
        let tab = self.tab();
        let message: SharedString = self
            .project
            .status
            .clone()
            .or_else(|| {
                tab.and_then(Tab::diagnostic_at_cursor).map(|d| format!("{}: {}", d.severity.label(), d.message.lines().next().unwrap_or_default()).into())
            })
            .or_else(|| tab.map(|t| t.doc.name()))
            .unwrap_or_else(|| "no file open".into());
        let position = tab.map(|t| {
            let (line, column) = t.cursor_position();
            format!("{}:{}", line + 1, column + 1)
        });
        let server = tab.and_then(|t| t.doc.lsp.as_ref().map(|l| l.server()));
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .h(pt(24.))
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.status_bar)
            .text_xs()
            .whitespace_nowrap()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_color(match mode {
                                Mode::Normal => theme.foreground,
                                Mode::Insert => theme.success,
                                Mode::Select => theme.cursor_select,
                                Mode::Tree => theme.warning,
                                Mode::Ai => theme.info,
                                Mode::Terminal => theme.success,
                            })
                            .child(mode.label()),
                    )
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().child(message)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_3()
                    .children(self.recording_macro().then(|| div().text_color(theme.danger).child("recording")))
                    .child(self.modal.pending_label())
                    .children(server)
                    .children(position),
            )
            .into_any_element()
    }

    /// Scroll the cheatsheet if `keystroke` is one that does.
    pub(super) fn scroll_cheatsheet(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        const ROW: f32 = 32.;
        let ctrl = keystroke.modifiers.control;
        let rows = match keystroke.key.as_str() {
            "j" | "down" if !ctrl => 1.,
            "k" | "up" if !ctrl => -1.,
            "d" if ctrl => 12.,
            "u" if ctrl => -12.,
            _ => return false,
        };
        let handle = &self.cheatsheet_scroll;
        let (offset, most) = (handle.offset(), handle.max_offset().height);
        handle.set_offset(gpui::point(offset.x, (offset.y - gpui::px(rows * ROW)).clamp(-most.max(gpui::px(0.)), gpui::px(0.))));
        cx.notify();
        true
    }

    fn render_cheatsheet(&self, cx: &Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let section = |(title, keys): &(&str, &[(&str, &str)])| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.warning).child(title.to_string()))
                .children(keys.iter().map(|(k, what)| {
                    div()
                        .flex()
                        .gap_3()
                        .text_xs()
                        .child(div().w(pt(132.)).flex_none().font_family(theme.mono_font.clone()).text_color(theme.foreground).child(crate::keys::shown(k)))
                        .child(div().text_color(theme.muted_foreground).child(what.to_string()))
                }))
        };
        let columns: [&[usize]; 3] = [&[0, 1, 2], &[3, 4, 5], &[6, 7, 8]];
        // What plugins bind for this file, in a section of their own at the end.
        let bound: Vec<(&str, &str)> = ghostwrite_plugin::keys(self.active_language()).iter().map(|b| (b.keys, b.what)).collect();
        let mut this_file = (!bound.is_empty()).then(|| section(&("Plugin keys", bound.as_slice())));
        let last = columns.len() - 1;
        div()
            .absolute()
            .inset_0()
            // Over the code: clicks and the wheel stay here.
            .occlude()
            .flex()
            .justify_center()
            .p_4()
            .bg(theme.background.opacity(0.6))
            .child(
                div()
                    // As tall as the window, and never wider: a long line wraps.
                    .h_full()
                    .max_w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Keys"))
                            .child(div().text_xs().text_color(theme.muted_foreground).child("j k scroll · any other key closes")),
                    )
                    .child(div().id("cheatsheet").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.cheatsheet_scroll).min_w_0().child(
                        div().flex().items_start().gap_6().min_w_0().children(columns.iter().enumerate().map(|(n, col)| {
                            let extra = if n == last { this_file.take() } else { None };
                            div().flex_none().min_w_0().flex().flex_col().gap_4().children(col.iter().map(|&i| section(&CHEATSHEET[i]))).children(extra)
                        })),
                    )),
            )
            .into_any_element()
    }

    // Menu actions.

    // In terminal mode they're about the shell, where only paste means
    // something. While a story's being reviewed the buffer isn't edited
    // behind its back: copying is all they do.

    fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.modal.mode == Mode::Terminal || self.modal.mode.reviewing() {
            return;
        }
        self.leave_insert();
        if let Some(tab) = self.tab_mut() {
            let restored = if redo { tab.doc.buffer.redo() } else { tab.doc.buffer.undo() };
            if let Some(sel) = restored {
                tab.select(sel, Autoscroll::Fit);
            }
            tab.sync();
        }
        cx.notify();
    }

    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        if self.modal.mode == Mode::Terminal {
            return self.terminal_copy(cx);
        }
        let cut = cut && !self.modal.mode.reviewing();
        let Some(tab) = self.tab_mut() else { return };
        let range = tab.view.selection.min_width_1(tab.doc.text()).bytes();
        cx.write_to_clipboard(ClipboardItem::new_string(tab.doc.text()[range.clone()].to_string()));
        if cut {
            tab.doc.buffer.begin(tab.view.selection);
            tab.doc.buffer.edit(range.clone(), "");
            tab.view.selection = Range::point(range.start);
            tab.doc.buffer.commit(tab.view.selection);
            tab.sync();
        }
        cx.notify();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if self.modal.mode == Mode::Terminal {
            return self.terminal_paste(cx);
        }
        if self.modal.mode.reviewing() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let insert = self.modal.mode == Mode::Insert;
        let Some(tab) = self.tab_mut() else { return };
        let sel = tab.view.selection;
        let range = if insert { sel.bytes() } else { sel.min_width_1(tab.doc.text()).bytes() };
        tab.doc.buffer.begin(sel);
        tab.doc.buffer.edit(range.clone(), &text);
        let pasted = if insert { Range::point(range.start + text.len()) } else { Range::new(range.start, range.start + text.len()) };
        tab.select(pasted, Autoscroll::Fit);
        if !insert {
            tab.doc.buffer.commit(pasted);
        }
        tab.sync();
        cx.notify();
    }
}

impl gpui::Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx).clone();
        let project = self.project.root.file_name().map_or_else(|| self.project.root.display().to_string(), |n| n.to_string_lossy().into_owned());
        match self.tab().map(|t| &t.doc) {
            Some(doc) => window.set_window_title(&format!("{}{} — {project}", if doc.dirty { "• " } else { "" }, doc.name())),
            None => window.set_window_title(&project),
        }
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus);
        }
        self.refresh_derived(cx);
        let body = match self.tab() {
            Some(tab) => {
                let view = if let Some(pages) = self.shown_pages() {
                    self.render_pages(pages, cx)
                } else {
                    let (mut decorations, block) = match &self.story {
                        Some(story) => story.overlay(&self.project, tab, self.ask_field(), self.terminal.shown, self.command_proposed(), &theme),
                        None => (vec![], None),
                    };
                    // While searching, every match on screen.
                    if self.find.shown {
                        let marked = theme.warning.opacity(0.25);
                        decorations.extend(self.find.near(tab).into_iter().map(|m| Decoration::background(m, marked)));
                    }
                    // Where the program it's debugging is stopped.
                    if let Some(line) = tab.doc.path.as_deref().and_then(|path| self.stopped_line(path)) {
                        decorations.push(Decoration::background(tab.doc.buffer.lines_range(line..line + 1), theme.warning.opacity(0.25)));
                    }
                    // The abbreviation being typed (`\alp`), until it's a symbol.
                    if let Some(range) = self.modal.abbreviation().filter(|r| r.end <= tab.doc.buffer.len()) {
                        decorations.push(Decoration { range, underline: Some(theme.accent), ..Decoration::default() });
                    }
                    CodeView::new(cx.entity(), self.focus.clone(), decorations, block).into_any_element()
                };
                let aside = self.render_aside(tab, cx);
                let view = self.beside(view, cx);
                let lit = self.render_attention(super::attention::Panel::Code, cx);
                div().flex_1().min_h_0().flex().child(div().relative().flex_1().min_w_0().child(view).children(lit)).children(aside).into_any_element()
            }
            None => self.render_empty(cx),
        };
        let tree = self.tree_pane.shown.then(|| {
            let lit = self.render_attention(super::attention::Panel::Tree, cx);
            let tree = self.render_tree(cx);
            div().relative().flex_none().flex().child(tree).children(lit).into_any_element()
        });
        let tabs = (!self.project.tabs.is_empty()).then(|| self.render_tabs(cx));
        let terminal = self.render_terminal(cx);
        let debug = self.render_debug(cx);
        div()
            .key_context("Editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.focus.is_focused(window) && this.key_down(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Open, window, cx| this.open_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.save_as(window, cx)))
            .on_action(cx.listener(
                |this, _: &CloseTab, window, cx| {
                    if this.modal.mode == Mode::Terminal { this.close_shell(this.terminal.active, cx) } else { this.close_tab(window, cx) }
                },
            ))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| if this.modal.mode == Mode::Terminal { this.new_shell(cx) } else { this.new_buffer(cx) }))
            .on_action(cx.listener(
                |this, _: &MoveTabLeft, _, cx| if this.modal.mode == Mode::Terminal { this.nudge_shell(false, cx) } else { this.nudge_tab(false, 1, cx) },
            ))
            .on_action(cx.listener(
                |this, _: &MoveTabRight, _, cx| if this.modal.mode == Mode::Terminal { this.nudge_shell(true, cx) } else { this.nudge_tab(true, 1, cx) },
            ))
            .on_action(
                cx.listener(|this, _: &NextTab, _, cx| if this.modal.mode == Mode::Terminal { this.cycle_shell(true, cx) } else { this.cycle_tab(true, cx) }),
            )
            .on_action(
                cx.listener(|this, _: &PrevTab, _, cx| if this.modal.mode == Mode::Terminal { this.cycle_shell(false, cx) } else { this.cycle_tab(false, cx) }),
            )
            .on_action(cx.listener(|this, _: &ToggleTree, _, cx| this.toggle_tree(cx)))
            .on_action(cx.listener(|this, _: &ToggleTerminal, _, cx| this.toggle_terminal(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_redo(false, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.undo_redo(true, cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(false, cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.copy(true, cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                let terminal = this.modal.mode == Mode::Terminal;
                if let Some(tab) = this.tab_mut().filter(|_| !terminal) {
                    tab.view.selection = Range::new(0, tab.doc.buffer.len());
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                // Let go of outside the window: the drag is over.
                if event.pressed_button != Some(MouseButton::Left) {
                    return this.release_edges();
                }
                this.drag_edges(event.position, window, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.release_edges()))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.ui_font.clone())
            .child(div().flex_1().min_h_0().flex().children(tree).child(
                div().flex_1().min_w_0().flex().flex_col().children(tabs).child(body).children(debug).children(terminal).children(self.render_command_line(cx)),
            ))
            .child(self.render_status(cx))
            .children(self.cheatsheet.then(|| self.render_cheatsheet(cx)))
            .children(self.render_picker(cx))
            .children(self.render_completion(&theme))
            .children(self.render_signature(&theme))
    }
}
