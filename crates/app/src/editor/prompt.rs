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
    /// Searching the file, forward or back.
    Find {
        backward: bool,
    },
    /// What's typed into a picker (`space f`).
    Pick,
    /// A new name for what's under the cursor (`space r`).
    Rename,
    /// A regex: a cursor on each match in the selection (`s`).
    SelectIn,
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
            PromptKind::Ask => format!("Ask {}, or tell it what to change…", self.backend.as_ref().map_or("Claude", |b| b.name())),
            PromptKind::Command => String::new(),
            PromptKind::TreeAdd(_) => "name, or name/ for a folder".into(),
            PromptKind::TreeRename(_) => "new name".into(),
            PromptKind::Find { .. } => String::new(),
            PromptKind::Pick => "type to narrow down".into(),
            PromptKind::Rename => "new name".into(),
            PromptKind::SelectIn => "select what, in the selection (or the file)".into(),
        };
        let field = cx.new(|cx| TextField::new(text, &placeholder, cx));
        let subscription = cx.subscribe_in(&field, window, |this, field, event, window, cx| {
            let text = field.read(cx).text().to_string();
            this.prompt_event(*event, text, window, cx);
        });
        field.read(cx).focus(window);
        // Saying something during a review: the model's started on it while it's typed.
        if kind == PromptKind::Ask
            && let (Some(backend), Some(state)) = (&self.backend, &self.story)
            && let Some(session) = state.conversation_session().filter(|_| !state.story.as_ref().is_some_and(|s| s.is_writing()))
        {
            backend.warm(Some(&self.project.root), Some(session));
        }
        if kind == PromptKind::Command {
            self.history.rewind();
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
                if !super::commands::is_secret(&text) {
                    self.history.remember(&text);
                }
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
            (TextFieldEvent::Submit, PromptKind::Rename) => {
                self.close_prompt(window, cx);
                self.rename_to(text.trim(), cx);
            }
            (TextFieldEvent::Submit, PromptKind::SelectIn) => {
                self.close_prompt(window, cx);
                self.select_matches(&text);
            }
            (TextFieldEvent::Changed, PromptKind::Command) => self.suggest_model(&text, cx),
            (TextFieldEvent::Changed, PromptKind::Pick) => self.picker_typed(&text, cx),
            (TextFieldEvent::Up | TextFieldEvent::Down, PromptKind::Pick) => self.picker_move(event == TextFieldEvent::Down, cx),
            (TextFieldEvent::Submit, PromptKind::Pick) => self.picker_open(window, cx),
            (TextFieldEvent::Cancel, PromptKind::Pick) => self.close_picker(window, cx),
            (TextFieldEvent::Changed, PromptKind::Find { .. }) => self.find_typed(&text, cx),
            (TextFieldEvent::Submit, PromptKind::Find { .. }) => self.end_find(window, cx),
            (TextFieldEvent::Cancel, PromptKind::Find { .. }) => self.cancel_find(window, cx),
            (TextFieldEvent::Cancel, PromptKind::Ask) => self.cancel_ask(window, cx),
            (TextFieldEvent::Cancel, _) => self.close_prompt(window, cx),
            (TextFieldEvent::Up | TextFieldEvent::Down, PromptKind::Command) => self.history_step(event == TextFieldEvent::Up, cx),
            (TextFieldEvent::Complete, PromptKind::Command) => self.complete_command(&text, cx),
            _ => {}
        }
        cx.notify();
    }
}
