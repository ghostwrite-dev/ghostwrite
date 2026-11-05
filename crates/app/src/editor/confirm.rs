//! A question over the code before something's done: what it would do, a line each,
//! and Enter to do it or Esc not to. While one's up it has the keys; if there are
//! more, they're asked one after another.

use gpui::{AnyElement, Context, FontWeight, Keystroke, div, prelude::*};

use super::Editor;
use crate::theme::{pt, theme};

/// What's done when it's answered yes.
type Then = Box<dyn FnOnce(&mut Editor, &mut Context<Editor>)>;

pub(super) struct Question {
    /// What's asked, as "Install ‹plugin› ‹version›?"
    pub(super) asked: String,
    /// What it would do, a line each, and whether the line is a warning.
    pub(super) lines: Vec<(String, bool)>,
    /// What Enter does, as the keys at the bottom say it.
    pub(super) yes: &'static str,
    /// Done on Enter. On Esc it's dropped, with whatever it holds.
    then: Then,
}

impl Question {
    pub(super) fn new(asked: String, lines: Vec<(String, bool)>, yes: &'static str, then: impl FnOnce(&mut Editor, &mut Context<Editor>) + 'static) -> Self {
        Self { asked, lines, yes, then: Box::new(then) }
    }
}

impl Editor {
    /// Ask `question`, after any that are up already.
    pub(super) fn put_question(&mut self, question: Question, cx: &mut Context<Self>) {
        self.questions.push(question);
        cx.notify();
    }

    /// A key while a question's up: Enter says yes, Esc no, and any other is ignored.
    /// Whether it was taken (one with ⌘ goes on to the menus).
    pub(super) fn answer_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        if self.questions.is_empty() || keystroke.modifiers.platform {
            return false;
        }
        match keystroke.key.as_str() {
            "enter" => {
                let question = self.questions.remove(0);
                (question.then)(self, cx);
            }
            "escape" => {
                self.questions.remove(0);
            }
            _ => {}
        }
        cx.notify();
        true
    }

    /// The question being asked, over the code, as a picker is.
    pub(super) fn render_question(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let question = self.questions.first()?;
        let theme = theme(cx);
        let lines = question.lines.iter().map(|(line, warning)| {
            let said = if *warning { format!("! {line}") } else { format!("  {line}") };
            div().px_3().text_color(if *warning { theme.warning } else { theme.foreground }).child(said)
        });
        let more = if self.questions.len() > 1 { format!(" · {} more", self.questions.len() - 1) } else { String::new() };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .justify_center()
                .pt(pt(60.))
                .child(
                    div()
                        .w(pt(560.))
                        .max_w_full()
                        .h_auto()
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .shadow_lg()
                        .font_family(theme.mono_font.clone())
                        .text_size(theme.mono_size)
                        .child(div().px_3().py_2().border_b_1().border_color(theme.border).font_weight(FontWeight::SEMIBOLD).child(question.asked.clone()))
                        .child(div().py_1().children(lines))
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(theme.border)
                                .text_color(theme.muted_foreground)
                                .child(format!("⏎ {} · esc cancel{more}", question.yes)),
                        ),
                )
                .into_any_element(),
        )
    }
}
