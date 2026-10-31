//! Language servers in the editor: `gd`, `space k` (docs as a note under
//! the line), `space j` (the next problem), the completion menu while
//! typing, and going where a server says (`window/showDocument`). Diagnostics go onto
//! documents as they arrive (see [`Project::apply_diagnostics`]).

use ghostwrite_core::{Key, MenuKey, Mode, Range};
use ghostwrite_lsp::{Completion, convert};
use gpui::{AnyElement, Bounds, Context, Pixels, Window, anchored, deferred, div, point, prelude::*};

use super::{Autoscroll, DocId, Editor, Project};
use crate::theme::{Theme, pt};

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
        let mut cursor = start + item.text.len();
        // What comes with it (an import), elsewhere: from the last, in the text as it was.
        let mut more = item.more.clone();
        more.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, text) in more.into_iter().filter(|(r, _)| r.end <= start || r.start >= end) {
            let shift = if range.start >= end { cursor as isize - end as isize } else { 0 };
            let range = (range.start as isize + shift) as usize..(range.end as isize + shift) as usize;
            if range.end <= start {
                cursor = (cursor + text.len()).saturating_sub(range.len());
            }
            tab.doc.buffer.edit(range, &text);
        }
        tab.select(Range::point(cursor), Autoscroll::Fit);
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
                .children(item.detail.clone().map(|d| div().max_w(pt(260.)).overflow_hidden().whitespace_nowrap().text_color(theme.muted_foreground).child(d)))
        });
        deferred(
            anchored().position(point(cursor.left(), cursor.bottom())).child(
                div()
                    .occlude()
                    .min_w(pt(180.))
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

/// The problem `space j` goes to from `cursor`, and which of how many of
/// its kind it is: the next error after it (round to the first), or with
/// no errors, the next warning.
fn next_problem(diagnostics: &[ghostwrite_lsp::Diagnostic], cursor: usize) -> Option<(&ghostwrite_lsp::Diagnostic, usize, usize)> {
    use ghostwrite_lsp::Severity;
    let worst = [Severity::Error, Severity::Warning].into_iter().find(|s| diagnostics.iter().any(|d| d.severity == *s))?;
    let mut kind: Vec<&ghostwrite_lsp::Diagnostic> = diagnostics.iter().filter(|d| d.severity == worst).collect();
    kind.sort_by_key(|d| d.range.start);
    kind.dedup_by_key(|d| d.range.start);
    let ix = kind.iter().position(|d| d.range.start > cursor).unwrap_or(0);
    Some((kind[ix], ix + 1, kind.len()))
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
        let from = self.before_jump();
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
                this.note_jump(from);
            })
            .ok();
        })
        .detach();
    }

    /// A server asked for `path` to be shown (tinymist, when its preview is
    /// clicked): open it at `selection`, and bring the window to the front.
    pub(super) fn show_document(&mut self, path: &std::path::Path, selection: Option<ghostwrite_lsp::RawRange>, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_insert();
        let Some(ix) = self.project.open(path) else { return cx.notify() };
        if let Some((tab, selection)) = self.project.tabs.get_mut(ix).zip(selection) {
            let at = convert::range(tab.doc.text(), selection);
            let lines = tab.doc.buffer.line_of(at.start)..tab.doc.buffer.line_of(at.end) + 1;
            tab.select(Range::point(at.start), Autoscroll::Center(lines));
        }
        self.activate(ix, cx);
        window.activate_window();
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

    /// `space j`: the next error after the cursor, round to the first; or,
    /// with no errors, the next warning (Lean's unsolved goals are errors,
    /// its `sorry`s warnings). A Typst file's pages come along.
    pub(super) fn next_problem(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let cursor = tab.cursor();
        let checking = tab.doc.goals.as_ref().and_then(|g| g.checking);
        let found = next_problem(&tab.doc.diagnostics, cursor).map(|(d, nth, of)| {
            let first = d.message.lines().next().unwrap_or_default();
            (d.range.start, format!("{} {nth} of {of}: {first}", d.severity.label()))
        });
        match found {
            Some((at, message)) => {
                let line = tab.doc.buffer.line_of(at);
                tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
                self.project.say(message);
            }
            None => match checking {
                Some(line) => self.project.say(format!("no problems yet: Lean is still checking line {}", line + 1)),
                None => self.project.say("no problems in this file"),
            },
        }
        self.bring_pages(cx);
        cx.notify();
    }

    /// `]d`, `[d`: the next diagnostic (of any kind), or the one before, round the file.
    pub(super) fn next_diagnostic(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let cursor = tab.cursor();
        let mut starts: Vec<(usize, String)> =
            tab.doc.diagnostics.iter().map(|d| (d.range.start, format!("{}: {}", d.severity.label(), d.message.lines().next().unwrap_or_default()))).collect();
        starts.sort_by_key(|(at, _)| *at);
        starts.dedup_by_key(|(at, _)| *at);
        let found = if forward {
            starts.iter().find(|(at, _)| *at > cursor).or(starts.first())
        } else {
            starts.iter().rev().find(|(at, _)| *at < cursor).or(starts.last())
        };
        let Some((at, message)) = found.cloned() else { return self.project.say("no diagnostics in this file") };
        let line = tab.doc.buffer.line_of(at);
        tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
        self.project.say(message);
        cx.notify();
    }

    /// After typing in insert mode: open (or refresh) the completion menu
    /// as a word is typed or after a trigger character; close it otherwise.
    pub(super) fn after_typing(&mut self, typed: &str, cx: &mut Context<Self>) {
        self.signature_after_typing(typed, cx);
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let Some(last) = typed.chars().last() else { return };
        // `\alp` is a symbol on its way, not a name.
        if self.modal.abbreviation().is_some() {
            self.completion = None;
            return;
        }
        let word = last.is_alphanumeric() || last == '_';
        if !word && !lsp.trigger_characters().iter().any(|t| typed.ends_with(t.as_str())) {
            self.completion = None;
            return;
        }
        let (text, at, doc) = (tab.doc.text().to_string(), tab.view.selection.head, tab.doc.id);
        let answer = lsp.ask("textDocument/completion", &text, at);
        self.completions_asked += 1;
        let asked = self.completions_asked;
        cx.spawn(async move |this, cx| {
            let Some(value) = answer.await.ok().and_then(Result::ok) else { return };
            this.update(cx, |this, cx| {
                // Only the latest answer, and only if we're still typing there.
                if asked != this.completions_asked {
                    return;
                }
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

#[cfg(test)]
mod tests {
    use ghostwrite_lsp::{Diagnostic, Severity};

    use super::next_problem;

    #[test]
    fn space_j_goes_to_the_next_error_round_to_the_first_then_warnings() {
        let d = |at: usize, severity| Diagnostic { range: at..at + 1, severity, message: format!("at {at}") };
        let all = [d(30, Severity::Error), d(5, Severity::Warning), d(10, Severity::Error), d(20, Severity::Info)];
        let at = |cursor| next_problem(&all, cursor).map(|(d, nth, of)| (d.range.start, nth, of));
        assert_eq!(at(0), Some((10, 1, 2)));
        assert_eq!(at(10), Some((30, 2, 2)));
        assert_eq!(at(30), Some((10, 1, 2)), "round to the first");
        let warnings = [d(5, Severity::Warning), d(20, Severity::Info)];
        assert_eq!(next_problem(&warnings, 9).map(|(d, ..)| d.range.start), Some(5), "no errors: the warnings");
        assert!(next_problem(&[d(20, Severity::Info)], 0).is_none(), "info isn't a problem");
    }
}
