//! What's beside the code: a plugin's view of blocks for a file's language, and the
//! debugger. A plugin's is a [`ghostwrite_plugin::Panel`], which says what to ask the
//! file's language server about the cursor and what to show of the answers. It's kept
//! on the document, kept up to date as the cursor moves and the file is edited. While
//! something's being debugged, the debugger is there too, on top of the panel if
//! there's one (see [`super::debug`]): each is a view of its own. The pane's left edge
//! drags to resize it. The plugin's view's key opens it and puts the keys there (again,
//! back to the code), and `:preview` shows or hides it. There `j` and `k` scroll it, a
//! click puts the keys there too, and the mode line calls it by its title.

use ghostwrite_core::{Language, Shows, ViewKind};
use ghostwrite_lsp::{Diagnostic, Severity};
use ghostwrite_plugin::{Block, Panel};
use gpui::{AnyElement, Context, Hsla, MouseButton, MouseDownEvent, div, prelude::*};
use serde_json::Value;

pub use ghostwrite_state::aside::Aside;

use super::document::DocId;
use super::page_view::PageView;
use super::{Editor, Tab};
use crate::theme::{Theme, pt, theme};
use crate::ui::pane::{Edge, Resizable};

/// The pane: half the room, until its edge is dragged.
pub(super) fn width() -> Resizable {
    Resizable::new("preview-width", Edge::Left, None)
}

/// What's beside a file of `language`: the panel of its plugin's view of blocks, if it has one.
pub(super) fn panel_for(language: &Language) -> Option<Aside> {
    let view = ghostwrite_plugin::view_showing(Shows::Blocks, Some(language))?;
    Some(Aside::new(view, ghostwrite_plugin::panel_of(view.id, language)?))
}

/// How wide it can be dragged, in a window `room` points wide.
pub(super) fn widths(room: f32) -> std::ops::RangeInclusive<f32> {
    200.0..=(room - 300.).max(200.)
}

impl Editor {
    /// Keep what goes with the active file up to date, if it's shown.
    pub(super) fn refresh_aside(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        // A tab of pages, or the file while it has one.
        let id = tab.doc.id;
        let viewed = tab.doc.view_of.as_ref().map(|(file, _)| *file).or_else(|| self.project.tabs.iter().any(|t| t.doc.is_view_of(id)).then_some(id));
        if let Some(file) = viewed {
            return self.refresh_rendered(file, cx);
        }
        if self.project.aside_shown {
            self.refresh_panel(cx);
        }
    }

    /// `:preview`: from a file to its pages and back, or show or hide what's beside it.
    pub(super) fn preview(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.preview_pages(cx)? {
            return Ok(());
        }
        if self.current()?.doc.language.and_then(panel_for).is_none() {
            return Err("this file has nothing to preview".into());
        }
        self.project.aside_shown = !self.project.aside_shown;
        self.project.say(if self.project.aside_shown { "shown beside the file" } else { "hidden" });
        Ok(())
    }

    /// A server's notification of its own: its file's panel may make something of it.
    pub(super) fn server_notification(&mut self, method: &str, params: &Value, cx: &mut Context<Self>) {
        let Some(path) = params["textDocument"]["uri"].as_str().and_then(ghostwrite_lsp::convert::path_of) else { return };
        let tab = self.project.tabs.index_of_path(&path).and_then(|ix| self.project.tabs.get_mut(ix));
        if let Some(aside) = tab.and_then(|t| t.doc.aside.as_mut()) {
            if aside.panel.notification(method, params) {
                aside.asked = None;
            }
            cx.notify();
        }
    }

    /// Keep the active tab's panel up to date with its cursor and text.
    fn refresh_panel(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let cursor = tab.cursor();
        let doc = &mut tab.doc;
        if doc.lsp.is_none() {
            return;
        }
        if doc.aside.is_none() {
            let Some(aside) = doc.language.and_then(panel_for) else { return };
            doc.aside = Some(aside);
        }
        let (Some(lsp), Some(aside)) = (doc.lsp.as_ref(), doc.aside.as_mut()) else { return };
        let now = (doc.buffer.version(), cursor);
        if aside.asked == Some(now) {
            return;
        }
        aside.asked = Some(now);
        let text = doc.buffer.rope();
        let mut methods = aside.panel.requests().iter();
        let Some(first) = methods.next() else { return };
        let (first, question) = aside.latest.ask(lsp, first, text, cursor);
        let mut waiting = vec![first];
        waiting.extend(methods.map(|method| aside.latest.also(lsp, method, text, cursor)));
        let id = doc.id;
        cx.spawn(async move |this, cx| {
            let mut answers = vec![];
            for answer in waiting {
                answers.push(answer.await.ok().and_then(Result::ok).filter(|v: &Value| !v.is_null()));
            }
            this.update(cx, |this, cx| {
                let Some(aside) = this.project.tabs.by_id_mut(id).and_then(|t| t.doc.aside.as_mut()) else { return };
                if aside.latest.answered(question) {
                    aside.panel.answered(answers);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// What's beside `tab`'s code, if there's something to show: the debugger, while
    /// something's being debugged, over the file's panel, if it's shown. Each is a view,
    /// and a click on one puts the keys there.
    pub(super) fn render_aside(&self, tab: &Tab, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = theme(cx).clone();
        let keyed = |view: ViewKind| if self.modal.view == view { theme.success } else { theme.border };
        let panel = tab.doc.aside.as_ref().filter(|_| self.project.aside_shown).map(|aside| {
            let view = aside.view.kind();
            div()
                .flex_1()
                .min_h_0()
                .border_l_1()
                .border_color(keyed(view))
                .debug_selector(|| "panel".to_string())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.focus_on(view, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(self.render_panel(aside.panel.as_ref(), cx))
        });
        // The debugger on top, while it's running; alone, it has the whole height.
        let debug = self.render_debug(panel.is_none(), cx).map(|debug| {
            div()
                .flex()
                .flex_col()
                .when(panel.is_none(), |d| d.flex_1().min_h_0())
                .border_l_1()
                .border_color(keyed(ViewKind::Debugger))
                .child(debug)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.focus_on(ViewKind::Debugger, cx);
                        cx.stop_propagation();
                    }),
                )
        });
        if panel.is_none() && debug.is_none() {
            return None;
        }
        Some(
            div()
                .relative()
                .min_w_0()
                .map(|d| match self.aside_width.size {
                    Some(width) => d.flex_none().w(pt(width)),
                    None => d.flex_1(),
                })
                .flex()
                .flex_col()
                .bg(theme.tab_bar)
                .debug_selector(|| "aside".to_string())
                .children(debug)
                .children(panel)
                .child(self.aside_width.handle("aside-edge").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        // It's at the window's right: as wide as from here to there.
                        let width = f32::from(window.viewport_size().width - event.position.x) / crate::theme::theme(cx).zoom;
                        this.aside_width.grab(event.position, width);
                        cx.stop_propagation();
                    }),
                ))
                .into_any_element(),
        )
    }

    /// A panel: what it says, then the messages on the cursor's line, and how far its
    /// server has got.
    fn render_panel(&self, panel: &dyn Panel, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx).clone();
        let tab = self.tab();
        let line = tab.map(|t| t.doc.buffer.line_of(t.cursor()));
        let messages: Vec<&Diagnostic> = tab
            .zip(line)
            .map(|(t, line)| {
                let buffer = &t.doc.buffer;
                t.doc.diagnostics.iter().filter(|d| (buffer.line_of(d.range.start)..=buffer.line_of(d.range.end)).contains(&line)).collect()
            })
            .unwrap_or_default();
        let working = panel.working_on().map(|line| div().text_color(theme.muted_foreground).child(format!("checking line {}…", line + 1)));
        let heading = |text: &str| div().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(text.to_string());
        let mut body: Vec<AnyElement> = panel
            .blocks()
            .into_iter()
            .map(|block| match block {
                Block::Heading(text) => heading(&text).into_any_element(),
                Block::Code { text, stands_out } => code(&text, stands_out, &theme),
                Block::Good(text) => div().text_color(theme.success).child(text).into_any_element(),
                Block::Muted(text) => div().text_color(theme.muted_foreground).child(text).into_any_element(),
            })
            .collect();
        if !messages.is_empty() {
            body.push(heading("Messages").mt_2().into_any_element());
            body.extend(messages.iter().map(|d| {
                let color = severity_color(d.severity, &theme);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(color).child(d.severity.label()))
                    .child(code(&d.message, None, &theme))
                    .into_any_element()
            }));
        }
        div()
            .id("panel")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.aside_scroll)
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .text_color(theme.foreground)
            .child(div().flex().justify_between().child(heading(panel.title())).children(working))
            .children(body)
            .into_any_element()
    }

    /// A tab of the pages of `of`, under why they can't be made now, if they can't.
    pub(super) fn render_pages(&self, of: DocId, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let error = self
            .project
            .tabs
            .by_id(of)
            .and_then(|file| file.doc.rendered.as_ref())
            .and_then(|rendered| rendered.error.clone())
            .map(|error| div().flex_none().px_3().py_1().border_b_1().border_color(theme.border).text_xs().text_color(theme.danger).child(error));
        let view = div().flex_1().min_h_0().debug_selector(|| "page-view".to_string()).child(PageView::new(cx.entity(), self.focus.clone()));
        div().size_full().flex().flex_col().children(error).child(view).into_any_element()
    }
}

fn severity_color(severity: Severity, theme: &Theme) -> Hsla {
    match severity {
        Severity::Error => theme.danger,
        Severity::Warning => theme.warning,
        Severity::Info => theme.info,
        Severity::Hint => theme.hint,
    }
}

/// Text in the code's font, lines starting `stands_out` standing out.
fn code(text: &str, stands_out: Option<char>, theme: &Theme) -> AnyElement {
    let lines = text.lines().map(|line| {
        let out = stands_out.is_some_and(|c| line.starts_with(c));
        div().min_h(theme.mono_size * 1.3).when(out, |d| d.text_color(theme.accent).font_weight(gpui::FontWeight::SEMIBOLD)).child(line.to_string())
    });
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .font_family(theme.mono_font.clone())
        .text_size(theme.mono_size)
        .children(lines)
        .min_w(pt(0.))
        .into_any_element()
}
