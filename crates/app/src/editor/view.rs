//! The editor on screen: the file tree beside the tabs and the code, a tour
//! panel and the `:` line under them, and helix's status line at the bottom.
//! Also the menu actions, which act on the active document.

use ghostwrite_core::{Mode, Range};
use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, SharedString, Window, div, prelude::*, px,
};

use super::element::CodeView;
use super::{Autoscroll, Editor, PromptKind, Tab};
use crate::actions::*;
use crate::theme::theme;

/// The `?` cheatsheet: every key, grouped like helix's keymap.
const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("h j k l", "left down up right"),
            ("w b e", "next word, back, word end (W B E: WORD)"),
            ("t F T ‹c›", "till a char, find / till back"),
            ("gg ge", "file start, last line"),
            ("gh gl gs", "line start, end, first non-blank"),
            ("G  10G", "last line, line 10"),
            ("ctrl-d ctrl-u", "half page down, up"),
            ("gn gp", "next, previous tab"),
            ("gd", "go to definition"),
        ],
    ),
    ("Select", &[("v", "select mode (motions extend)"), ("x", "select line, again: next line"), ("%", "select all"), (";  alt-;", "collapse, flip selection")]),
    (
        "Edit",
        &[
            ("i a I A", "insert before, after, line start, end"),
            ("o O", "open line below, above"),
            ("d c", "delete, change selection"),
            ("y p P", "yank, paste after, before"),
            ("space y / p", "clipboard yank, paste"),
            ("r‹c›  ~", "replace with char, switch case"),
            ("> <", "indent, unindent"),
            ("u U", "undo, redo"),
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
    (
        "Language server",
        &[
            ("space k", "docs for what's under the cursor"),
            ("space d", "tour this file's diagnostics"),
            ("space /", "search the project (:grep)"),
            ("typing", "completion menu (enter picks)"),
        ],
    ),
    (
        "AI mode  space i",
        &[
            ("space i", "ask about the line or selection"),
            ("tab", "accept step · next note"),
            ("backspace", "skip"),
            ("enter", "redirect, or ask about it"),
            ("esc", "stop, back where you were"),
            ("motions", "look around; no editing"),
        ],
    ),
    (
        "Command line  :",
        &[
            (":e ‹path›  :w  :wa", "open, write, write all"),
            (":n :bn :bp :bc", "new, next, previous, close tab"),
            (":rl  :rm  :42", "reload, trash file, go to line"),
            (":sh ‹cmd›  :stop", "run in the background; Claude sees it"),
            (":output", "tour its errors or output"),
            (":tree  :project  :qa", "file tree, projects, quit"),
            ("↑ ↓  tab", "history, complete"),
        ],
    ),
    (
        "Tree mode  f",
        &[("j k  gg G", "move"), ("l h  enter", "open / preview, close, open file"), ("a r d", "add, rename, trash"), ("f  esc", "back to the buffer")],
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
                .child(div().w(px(96.)).text_right().font_family(theme.mono_font.clone()).text_color(theme.foreground).child(keys))
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
            .child(hint("f", "browse the file tree"))
            .child(hint(":e <path>", "open a file"))
            .child(hint(":n", "new buffer"))
            .child(hint(":project", "switch project"))
            .child(hint("?", "every key"))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx).clone();
        div()
            .id("tabs")
            .flex_none()
            .flex()
            .h(px(30.))
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
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx)))
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
                                this.project.tabs.set_active(ix);
                                this.close_tab(window, cx);
                            })),
                    )
            }))
            .into_any_element()
    }

    /// The `:` prompt: its own line above the status line, in the editor font.
    fn render_command_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let prompt = self.prompt.as_ref().filter(|p| p.kind == PromptKind::Command)?;
        let theme = theme(cx);
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .h(px(26.))
                .px_3()
                .font_family(theme.mono_font.clone())
                .text_size(theme.mono_size)
                .child(":")
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
            .h(px(24.))
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
                                Mode::Tour => theme.hint,
                            })
                            .child(mode.label()),
                    )
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().child(message)),
            )
            .child(div().flex().flex_none().items_center().gap_3().child(self.modal.pending_label()).children(server).children(position))
            .into_any_element()
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
                        .child(div().w(px(132.)).flex_none().font_family(theme.mono_font.clone()).text_color(theme.foreground).child(k.to_string()))
                        .child(div().text_color(theme.muted_foreground).child(what.to_string()))
                }))
        };
        let columns: [&[usize]; 3] = [&[0, 1, 2], &[3, 4, 5], &[6, 7]];
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background.opacity(0.6))
            .child(
                div()
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
                            .child(div().text_xs().text_color(theme.muted_foreground).child("any key closes")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .children(columns.iter().map(|col| div().flex().flex_col().gap_4().children(col.iter().map(|&i| section(&CHEATSHEET[i]))))),
                    ),
            )
            .into_any_element()
    }

    // Menu actions.

    fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
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
        if let Some(tab) = self.tab_mut() {
            tab.doc.refresh_highlights();
        }
        let body = match self.tab() {
            Some(tab) => {
                let (mut decorations, block) = match &self.story {
                    Some(story) => story.overlay(&self.project, tab, self.ask_field(), &theme),
                    None => (vec![], None),
                };
                decorations.extend(self.tour.as_ref().map(|t| t.decorations(&tab.doc, &theme)).unwrap_or_default());
                div().flex_1().min_h_0().child(CodeView::new(cx.entity(), self.focus.clone(), decorations, block)).into_any_element()
            }
            None => self.render_empty(cx),
        };
        let tree = self.tree_pane.shown.then(|| self.render_tree(cx));
        let tabs = (!self.project.tabs.is_empty()).then(|| self.render_tabs(cx));
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
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_tab(window, cx)))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_buffer(cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle_tab(true, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| this.cycle_tab(false, cx)))
            .on_action(cx.listener(|this, _: &ToggleTree, _, cx| this.toggle_tree(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_redo(false, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.undo_redo(true, cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(false, cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.copy(true, cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if let Some(tab) = this.tab_mut() {
                    tab.view.selection = Range::new(0, tab.doc.buffer.len());
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.drag_tree_edge(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.end_tree_drag()))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.ui_font.clone())
            .child(
                div().flex_1().min_h_0().flex().children(tree).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(tabs)
                        .child(body)
                        .children(self.tour.as_ref().and_then(|t| t.panel(&theme)))
                        .children(self.render_command_line(cx)),
                ),
            )
            .child(self.render_status(cx))
            .children(self.cheatsheet.then(|| self.render_cheatsheet(cx)))
            .children(self.render_completion(&theme))
    }
}
