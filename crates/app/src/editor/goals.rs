//! A Lean file's goals beside it, as Lean's infoview shows them (`:preview`
//! hides them, as it does Typst's pages): the proof state at the cursor
//! (`$/lean/plainGoal`), or else the type expected there
//! (`$/lean/plainTermGoal`); the messages on the cursor's line, `#eval`'s
//! output among them; and how far Lean has got checking the file. They're
//! asked for again as the cursor moves and the text changes.

use ghostwrite_lsp::{Diagnostic, Severity};
use gpui::{AnyElement, Context, Hsla, div, prelude::*};
use serde_json::Value;

use super::Editor;
use crate::theme::{Theme, pt, theme};

#[derive(Default)]
pub struct Goals {
    /// The buffer version and cursor asked about last.
    asked: Option<(u64, usize)>,
    /// How many times it's been asked: only the latest answer counts (Lean
    /// answers only once it's checked up to the cursor, so a cursor that's
    /// moved on doesn't wait for the old answer).
    questions: u64,
    /// The questions still unanswered, to cancel when there's a newer one.
    pending: Vec<i64>,
    /// The goals at the cursor, when it's in a proof (none left: it's done).
    pub goals: Option<Vec<String>>,
    /// The type expected at the cursor, when it's in a term.
    pub expected: Option<String>,
    /// The first line Lean is still checking.
    pub checking: Option<u32>,
}

impl Goals {
    /// Lean's progress: once it's done, the goals are asked for again.
    pub fn checked(&mut self, checking: Option<u32>) {
        if checking.is_none() && self.checking.is_some() {
            self.asked = None;
        }
        self.checking = checking;
    }
}

impl Editor {
    /// Keep the active tab's goals, if it's Lean, up to date with its
    /// cursor and text.
    pub(super) fn refresh_goals(&mut self, cx: &mut Context<Self>) {
        if !self.project.previews {
            return;
        }
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let cursor = tab.cursor();
        let doc = &mut tab.doc;
        let Some(lsp) = doc.lsp.as_ref().filter(|_| doc.language.is_some_and(|l| l.name == "lean")) else { return };
        let goals = doc.goals.get_or_insert_default();
        let now = (doc.buffer.version(), cursor);
        if goals.asked == Some(now) {
            return;
        }
        goals.asked = Some(now);
        goals.questions += 1;
        let question = goals.questions;
        for old in goals.pending.drain(..) {
            lsp.cancel(old);
        }
        let ((proof_id, proof), (term_id, term)) =
            (lsp.ask_cancelable("$/lean/plainGoal", doc.buffer.text(), cursor), lsp.ask_cancelable("$/lean/plainTermGoal", doc.buffer.text(), cursor));
        goals.pending = vec![proof_id, term_id];
        let id = doc.id;
        cx.spawn(async move |this, cx| {
            let answer = |a: Result<Result<Value, String>, _>| a.ok().and_then(Result::ok).filter(|v| !v.is_null());
            let (proof, term) = (answer(proof.await), answer(term.await));
            this.update(cx, |this, cx| {
                let Some(goals) = this.project.tabs.by_id_mut(id).and_then(|t| t.doc.goals.as_mut()).filter(|g| g.questions == question) else {
                    return;
                };
                goals.pending.clear();
                goals.goals = proof.map(|p| p["goals"].as_array().into_iter().flatten().filter_map(|g| Some(g.as_str()?.to_string())).collect());
                goals.expected = term.and_then(|t| Some(t["goal"].as_str()?.to_string()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The goals pane, for the active tab.
    pub(super) fn render_goals(&self, goals: &Goals, cx: &mut Context<Self>) -> AnyElement {
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
        let checking = goals.checking.map(|line| div().text_color(theme.muted_foreground).child(format!("checking line {}…", line + 1)));
        let heading = |text: &str| div().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(text.to_string());
        let mut body: Vec<AnyElement> = vec![];
        match (&goals.goals, &goals.expected) {
            (Some(list), _) if list.is_empty() => body.push(div().text_color(theme.success).child("No goals").into_any_element()),
            (Some(list), _) => {
                body.push(heading(&if list.len() == 1 { "1 goal".into() } else { format!("{} goals", list.len()) }).into_any_element());
                body.extend(list.iter().map(|goal| code(goal, &theme)));
            }
            (None, Some(expected)) => {
                body.push(heading("Expected type").into_any_element());
                body.push(code(expected, &theme));
            }
            (None, None) => body.push(div().text_color(theme.muted_foreground).child("No goals here: put the cursor in a proof").into_any_element()),
        }
        if !messages.is_empty() {
            body.push(heading("Messages").mt_2().into_any_element());
            body.extend(messages.iter().map(|d| {
                let color = severity_color(d.severity, &theme);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(color).child(d.severity.label()))
                    .child(code(&d.message, &theme))
                    .into_any_element()
            }));
        }
        div()
            .id("goals")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .text_color(theme.foreground)
            .child(div().flex().justify_between().child(heading("Lean")).children(checking))
            .children(body)
            .into_any_element()
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

/// Lean's text, in the code's font: a goal's `⊢` line stands out.
fn code(text: &str, theme: &Theme) -> AnyElement {
    let lines = text.lines().map(|line| {
        let goal = line.starts_with('⊢');
        div().min_h(theme.mono_size * 1.3).when(goal, |d| d.text_color(theme.accent).font_weight(gpui::FontWeight::SEMIBOLD)).child(line.to_string())
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
