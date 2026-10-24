//! The one prompt under the code: the `:` line, asking Claude, and naming
//! a file in the tree. While it's open it has the keyboard.

use std::path::PathBuf;

use gpui::{AppContext as _, Context, Entity, Subscription, Window};

use super::Editor;
use crate::ui::text_field::{TextField, TextFieldEvent};

/// What a prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PromptKind {
    Command,
    /// Asking Claude, or saying something about the item under review.
    Ask,
    /// Naming a new file or folder in this folder.
    TreeAdd(PathBuf),
    TreeRename(PathBuf),
}

/// The one open prompt: it has the keyboard until it closes.
pub(super) struct Prompt {
    pub(super) kind: PromptKind,
    pub(super) field: Entity<TextField>,
    _subscription: Subscription,
}

impl Editor {
    pub(super) fn open_prompt(&mut self, kind: PromptKind, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match kind {
            PromptKind::Ask => "Ask Claude, or tell it what to change…",
            PromptKind::Command => "",
            PromptKind::TreeAdd(_) => "name, or name/ for a folder",
            PromptKind::TreeRename(_) => "new name",
        };
        let field = cx.new(|cx| TextField::new(text, placeholder, cx));
        let subscription = cx.subscribe_in(&field, window, |this, field, event, window, cx| {
            let text = field.read(cx).text().to_string();
            this.prompt_event(*event, text, window, cx);
        });
        field.read(cx).focus(window);
        if kind == PromptKind::Command {
            self.history_at = None;
            self.project.status = None;
        }
        self.prompt = Some(Prompt { kind, field, _subscription: subscription });
        cx.notify();
    }

    pub(super) fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn asking(&self) -> bool {
        self.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Ask)
    }

    /// The prompt's field while asking Claude.
    pub(super) fn ask_field(&self) -> Option<Entity<TextField>> {
        self.prompt.as_ref().filter(|p| p.kind == PromptKind::Ask).map(|p| p.field.clone())
    }

    fn prompt_event(&mut self, event: TextFieldEvent, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.prompt.as_ref().map(|p| p.kind.clone()) else { return };
        match (event, kind) {
            (TextFieldEvent::Submit, PromptKind::Command) => {
                self.close_prompt(window, cx);
                self.remember_command(&text);
                if let Err(e) = self.run_command(&text, window, cx) {
                    self.project.status = (!e.is_empty()).then(|| e.into());
                }
            }
            (TextFieldEvent::Submit, PromptKind::Ask) => self.ask(text, window, cx),
            (TextFieldEvent::Submit, PromptKind::TreeAdd(dir)) => {
                self.close_prompt(window, cx);
                self.tree_add(&dir, &text, cx);
            }
            (TextFieldEvent::Submit, PromptKind::TreeRename(from)) => {
                self.close_prompt(window, cx);
                self.tree_rename(&from, &text);
            }
            (TextFieldEvent::Cancel, PromptKind::Ask) => self.cancel_ask(window, cx),
            (TextFieldEvent::Cancel, _) => self.close_prompt(window, cx),
            (TextFieldEvent::Up | TextFieldEvent::Down, PromptKind::Command) => self.history_step(event == TextFieldEvent::Up, cx),
            (TextFieldEvent::Complete, PromptKind::Command) => self.complete_command(&text, cx),
            _ => {}
        }
        cx.notify();
    }
}
