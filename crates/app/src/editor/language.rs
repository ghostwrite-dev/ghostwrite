//! Language servers in the editor: `gd`, `space k` (docs as a note under
//! the line), and the completion menu while typing. Diagnostics go onto
//! documents as they arrive (see [`Project::apply_diagnostics`]).

use ghostwrite_core::{Key, MenuKey, Mode, Range};
use ghostwrite_lsp::{Completion, convert};
use gpui::{AnyElement, Bounds, Context, Pixels, Window, anchored, deferred, div, point, prelude::*, px};

use super::{Autoscroll, DocId, Editor, Project};
use crate::theme::Theme;

const MENU_ROWS: usize = 10;

/// The completion menu: what the server offered at the cursor, and which is chosen.
pub(super) struct CompletionMenu {
    doc: DocId,
    items: Vec<Completion>,
    selected: usize,
}

impl CompletionMenu {
    fn select(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
    }

    /// Put the chosen completion in, replacing what was typed since the request.
    fn accept(&self, project: &mut Project) {
        let Some(item) = self.items.get(self.selected) else { return };
        let Some(tab) = project.tabs.by_id_mut(self.doc) else { return };
        let end = tab.view.selection.head.max(item.range.end).min(tab.doc.buffer.len());
        let start = item.range.start.min(end);
        tab.doc.buffer.begin(tab.view.selection);
        tab.doc.buffer.edit(start..end, &item.text);
        tab.select(Range::point(start + item.text.len()), Autoscroll::Fit);
        tab.sync();
    }

    /// The menu, under the cursor.
    fn render(&self, cursor: Bounds<Pixels>, theme: &Theme) -> AnyElement {
        let first = self.selected.saturating_sub(MENU_ROWS - 1);
        let rows = self.items.iter().enumerate().skip(first).take(MENU_ROWS).map(|(i, item)| {
            div()
                .px_2()
                .flex()
                .gap_3()
                .justify_between()
                .when(i == self.selected, |d| d.bg(theme.accent).text_color(theme.accent_foreground))
                .child(div().whitespace_nowrap().child(item.label.clone()))
                .children(item.detail.clone().map(|d| div().max_w(px(260.)).overflow_hidden().whitespace_nowrap().text_color(theme.muted_foreground).child(d)))
        });
        deferred(
            anchored().position(point(cursor.left(), cursor.bottom())).child(
                div()
                    .min_w(px(180.))
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_md()
                    .font_family(theme.mono_font.clone())
                    .text_size(theme.mono_size)
                    .children(rows),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl Editor {
    /// Ask the active document's server about the cursor.
    fn ask_server(&self, method: &str) -> Option<(ghostwrite_lsp::document::Answer, &'static str)> {
        let tab = self.tab()?;
        let lsp = tab.doc.lsp.as_ref()?;
        // In normal mode the block sits on a char; ask about that char.
        Some((lsp.ask(method, tab.doc.text(), tab.cursor()), lsp.server()))
    }

    /// `gd`: jump to the definition of what's under the cursor.
    pub(super) fn goto_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((answer, _)) = self.ask_server("textDocument/definition") else {
            return self.project.say("no language server for this file");
        };
        cx.spawn_in(window, async move |this, cx| {
            let found = answer.await.ok().and_then(Result::ok);
            this.update(cx, |this, cx| {
                let Some((path, pos)) = found.as_ref().and_then(convert::first_location) else {
                    this.project.say("no definition found");
                    return cx.notify();
                };
                let Some(ix) = this.project.open(&path) else { return };
                if let Some(tab) = this.project.tabs.get_mut(ix) {
                    let at = convert::offset(tab.doc.text(), pos);
                    let line = tab.doc.buffer.line_of(at);
                    tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
                }
                this.activate(ix, cx);
            })
            .ok();
        })
        .detach();
    }

    /// `space k`: the docs of what's under the cursor, as a note under the
    /// line; Enter asks Claude about them.
    pub(super) fn show_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((answer, server)) = self.ask_server("textDocument/hover") else {
            return self.project.say("no language server for this file");
        };
        cx.spawn_in(window, async move |this, cx| {
            let docs = answer.await.ok().and_then(Result::ok);
            this.update(cx, |this, cx| {
                match docs.as_ref().and_then(convert::hover_text) {
                    Some(docs) => this.show_note(&docs, cx),
                    None => this.project.say(format!("{server}: nothing to show here")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After typing in insert mode: open (or refresh) the completion menu
    /// as a word is typed or after a trigger character; close it otherwise.
    pub(super) fn after_typing(&mut self, typed: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let Some(last) = typed.chars().last() else { return };
        let word = last.is_alphanumeric() || last == '_';
        if !word && !lsp.trigger_characters().iter().any(|t| typed.ends_with(t.as_str())) {
            self.completion = None;
            return;
        }
        let (text, at, doc) = (tab.doc.text().to_string(), tab.view.selection.head, tab.doc.id);
        let answer = lsp.ask("textDocument/completion", &text, at);
        cx.spawn(async move |this, cx| {
            let Some(value) = answer.await.ok().and_then(Result::ok) else { return };
            this.update(cx, |this, cx| {
                // Only if we're still typing there.
                let still_there = this.tab().is_some_and(|t| t.doc.id == doc && t.view.selection.head >= at) && this.modal.mode == Mode::Insert;
                let items = convert::completions(value, &text, at);
                this.completion = (still_there && !items.is_empty()).then_some(CompletionMenu { doc, items, selected: 0 });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A key while the menu is open. Esc closes it and goes on to leave insert mode.
    pub(super) fn menu_key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.completion.as_mut() else { return false };
        match self.modal.menu_key(key) {
            Some(MenuKey::Next) => menu.select(1),
            Some(MenuKey::Prev) => menu.select(-1),
            Some(MenuKey::Accept) => {
                if let Some(menu) = self.completion.take() {
                    menu.accept(&mut self.project);
                }
            }
            Some(MenuKey::Close) => {
                self.completion = None;
                return false;
            }
            None => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_completion(&self, theme: &Theme) -> Option<AnyElement> {
        let menu = self.completion.as_ref().filter(|m| self.tab().is_some_and(|t| t.doc.id == m.doc))?;
        Some(menu.render(self.measured.cursor?, theme))
    }
}
