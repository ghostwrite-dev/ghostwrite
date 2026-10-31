//! The editor on screen: the file tree beside the tabs and the code (and a
//! Typst file's preview beside that), the `:` line under them, and helix's
//! status line at the bottom.
//! Also the menu actions, which act on the active document.

use ghostwrite_core::{Mode, Range};
use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString, Window, div,
    prelude::*,
};

use super::element::{CodeView, Decoration};
use super::pdf_view::PdfView;
use super::preview::Preview;
use super::preview_view::PreviewView;
use super::{Autoscroll, Editor, PromptKind, Tab};
use crate::actions::*;
use crate::theme::{pt, theme};

/// The `space ?` cheatsheet: every key. Helix's keys are as in helix;
/// Ghostwrite's own are under `space`; ⌘ is for the app and its windows.
const CHEATSHEET: &[(&str, &[(&str, &str)])] = &[
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
            ("gn gp", "next, previous tab"),
            ("gd  gr", "go to definition; through the references"),
            ("ctrl-o ctrl-i", "back where a jump came from, forward"),
        ],
    ),
    (
        "Select",
        &[
            ("v", "select mode (motions extend)"),
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
    ("Search", &[("/  ?", "search the file, forward, back (as typed; esc goes back)"), ("n N", "next match, previous"), ("*", "search for the selection")]),
    (
        "Space",
        &[
            ("space a", "ask Claude about the line or selection"),
            ("space e  ⌘B", "file tree"),
            ("space t  ⌘J", "terminal"),
            ("space f  space b", "pick a file, an open file (fuzzy)"),
            ("space /", "search the project"),
            ("space k  space d", "docs here; this file's diagnostics"),
            ("space r  space .", "rename; the server's code actions here"),
            ("space s", "pick one of the file's symbols"),
            ("space j", "next error (or warning); Typst pages follow"),
            ("space y  space p", "clipboard yank, paste"),
            ("space ?", "this, every key"),
        ],
    ),
    (
        "Reviewing",
        &[
            ("tab", "accept the step · next note or match"),
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
            (":sh ‹cmd›  :stop", "run in the background; Claude sees it"),
            (":output", "go through its errors or output"),
            (":tree :term :preview", "file tree, terminal, Typst pages / Lean goals"),
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
            ("", "your shell has the keys; Claude reads it, never types"),
            ("esc esc", "back where you were (in vim & co, click the code)"),
            ("drag  ⌘C", "select, copy"),
            ("⌘J", "hide it"),
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

    /// A Typst file's pages, under why it doesn't compile now, if it doesn't.
    fn render_preview(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let error = preview
            .error
            .clone()
            .map(|error| div().flex_none().px_3().py_1().border_b_1().border_color(theme.border).text_xs().text_color(theme.danger).child(error));
        let pages = div().flex_1().min_h_0().child(PreviewView::new(cx.entity(), self.focus.clone()));
        self.side_pane(div().size_full().flex().flex_col().children(error).child(pages).into_any_element(), cx)
    }

    /// What's beside the code (a Typst file's pages, a Lean file's goals),
    /// with an edge that drags to resize it.
    fn side_pane(&self, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        div()
            .relative()
            .min_w_0()
            .map(|d| match self.preview_pane.width {
                Some(width) => d.flex_none().w(pt(width)),
                None => d.flex_1(),
            })
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(div().flex_1().min_h_0().child(content))
            // The edge drags to resize.
            .child(div().id("preview-edge").absolute().top_0().left(pt(-3.)).w(pt(6.)).h_full().cursor_col_resize().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.preview_pane.resizing = true;
                    cx.stop_propagation();
                }),
            ))
            .into_any_element()
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
                                this.activate(ix, cx);
                                this.close_tab(window, cx);
                            })),
                    )
            }))
            .into_any_element()
    }

    /// The `:` prompt: its own line above the status line, in the editor font.
    fn render_command_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let prompt = self.prompt.as_ref().filter(|p| matches!(p.kind, PromptKind::Command | PromptKind::Find { .. } | PromptKind::Rename))?;
        let lead = match prompt.kind {
            PromptKind::Find { backward: true } => "?",
            PromptKind::Find { backward: false } => "/",
            PromptKind::Rename => "rename to ",
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
                        .child(div().w(pt(132.)).flex_none().font_family(theme.mono_font.clone()).text_color(theme.foreground).child(k.to_string()))
                        .child(div().text_color(theme.muted_foreground).child(what.to_string()))
                }))
        };
        let columns: [&[usize]; 3] = [&[0, 1, 2], &[3, 4, 5], &[6, 7, 8]];
        div()
            .absolute()
            .inset_0()
            // Over the code: clicks and the wheel stay here.
            .occlude()
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
        self.refresh_preview(cx);
        self.refresh_goals(cx);
        self.refresh_semantic(cx);
        if self.find.shown {
            // Found again for the text as it is now, to mark.
            self.matches();
        }
        if let Some((language, version, text)) = self.tab_mut().and_then(|tab| tab.doc.refresh_highlights()) {
            let id = self.tab().map(|t| t.doc.id);
            let work = cx.background_spawn(async move {
                let spans = language.highlight(&text);
                (text, spans)
            });
            cx.spawn(async move |this, cx| {
                let (text, spans) = work.await;
                this.update(cx, |this, cx| {
                    if let Some(tab) = id.and_then(|id| this.project.tabs.by_id_mut(id)) {
                        tab.doc.highlighted(language, version, text, spans);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
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
        let body = match self.tab() {
            Some(tab) => {
                let view = if tab.doc.pdf.is_some() {
                    PdfView::new(cx.entity(), self.focus.clone()).into_any_element()
                } else {
                    let (mut decorations, block) = match &self.story {
                        Some(story) => story.overlay(&self.project, tab, self.ask_field(), &theme),
                        None => (vec![], None),
                    };
                    // While searching, every match on screen.
                    if self.find.shown {
                        let marked = theme.warning.opacity(0.25);
                        decorations.extend(self.find.near(tab).into_iter().map(|m| Decoration::background(m, marked)));
                    }
                    // The abbreviation being typed (`\alp`), until it's a symbol.
                    if let Some(range) = self.modal.abbreviation().filter(|r| r.end <= tab.doc.buffer.len()) {
                        decorations.push(Decoration { range, underline: Some(theme.accent), ..Decoration::default() });
                    }
                    CodeView::new(cx.entity(), self.focus.clone(), decorations, block).into_any_element()
                };
                let preview = tab.doc.preview.as_ref().filter(|p| self.project.previews && (p.document.is_some() || p.error.is_some()));
                let goals = tab.doc.goals.as_ref().filter(|_| self.project.previews && tab.doc.lsp.is_some());
                let preview = preview.map(|p| self.render_preview(p, cx)).or_else(|| {
                    goals.map(|g| {
                        let goals = self.render_goals(g, cx);
                        self.side_pane(goals, cx)
                    })
                });
                div().flex_1().min_h_0().flex().child(div().flex_1().min_w_0().child(view)).children(preview).into_any_element()
            }
            None => self.render_empty(cx),
        };
        let tree = self.tree_pane.shown.then(|| self.render_tree(cx));
        let tabs = (!self.project.tabs.is_empty()).then(|| self.render_tabs(cx));
        let terminal = self.render_terminal(cx);
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
                    this.end_tree_drag();
                    this.end_terminal_drag();
                    this.end_preview_drag();
                    return;
                }
                this.drag_tree_edge(event, cx);
                this.drag_terminal_edge(event, window, cx);
                this.drag_preview_edge(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.end_tree_drag();
                    this.end_terminal_drag();
                    this.end_preview_drag();
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.ui_font.clone())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(tree)
                    .child(div().flex_1().min_w_0().flex().flex_col().children(tabs).child(body).children(terminal).children(self.render_command_line(cx))),
            )
            .child(self.render_status(cx))
            .children(self.cheatsheet.then(|| self.render_cheatsheet(cx)))
            .children(self.render_picker(cx))
            .children(self.render_completion(&theme))
            .children(self.render_signature(&theme))
    }
}
