//! What the language server does beyond `gd` and `space k` (in
//! [`super::language`]): `gr` goes through the references to what's under
//! the cursor (like `:grep`'s matches), `space r` renames it everywhere,
//! `space .` offers the server's code actions here, `space s` picks one of
//! the file's symbols, `:format` formats the file (and saving does, unless
//! `format_on_save` is off), and while a call is typed its signature shows.

use ghostwrite_ai::Item;
use ghostwrite_ai::protocol::Note;
use ghostwrite_core::Range;
use ghostwrite_lsp::{CodeAction, TextEdit, convert};
use gpui::{AnyElement, Context, Window, anchored, deferred, div, point, prelude::*};
use serde_json::{Value, json};

use super::Editor;
use super::picker::{Pick, PickerKind};
use super::prompt::PromptKind;
use crate::theme::{Theme, pt};

/// The signature of the call being typed, and which bytes of it are the parameter at the cursor.
pub(super) struct Signature {
    pub(super) label: String,
    pub(super) active: Option<std::ops::Range<usize>>,
}

impl Editor {
    /// The active file's server, if it does `provider`; else why not, said.
    fn server_for(&mut self, provider: &str, what: &str) -> Option<()> {
        let lsp = self.tab().and_then(|t| t.doc.lsp.as_ref());
        match lsp {
            Some(lsp) if lsp.supports(provider) => Some(()),
            Some(lsp) => {
                let server = lsp.server();
                self.project.say(format!("{server} doesn't do {what}"));
                None
            }
            None => {
                self.project.say("no language server for this file");
                None
            }
        }
    }

    /// Make `edits` to the file at `path` (opening it if it isn't), as one undo step.
    fn edit_file(&mut self, path: &std::path::Path, edits: &[TextEdit]) -> bool {
        let Some(ix) = self.project.tabs.index_of_path(path).or_else(|| self.project.open(path)) else { return false };
        let Some(tab) = self.project.tabs.get_mut(ix) else { return false };
        let placed = convert::apply(tab.doc.text(), edits);
        if placed.is_empty() {
            return false;
        }
        let selection = tab.view.selection;
        tab.doc.buffer.begin(selection);
        let mut cursor = selection.head;
        for (range, text) in placed {
            // The cursor moves with the text around it.
            if range.end <= cursor {
                cursor = (cursor + text.len()).saturating_sub(range.len());
            }
            tab.doc.buffer.edit(range, &text);
        }
        tab.view.selection = Range::point(cursor).within(tab.doc.text());
        tab.doc.buffer.commit(tab.view.selection);
        tab.sync();
        true
    }

    /// Make a workspace edit (a rename, a code action): every file's edits.
    pub(super) fn apply_workspace_edit(&mut self, edit: &Value, cx: &mut Context<Self>) {
        let files = convert::workspace_edit(edit);
        let changed = files.iter().filter(|(path, edits)| self.edit_file(path, edits)).count();
        self.project.say(match changed {
            0 => "nothing to change".into(),
            1 => "changed 1 file".into(),
            n => format!("changed {n} files (:wa writes them)"),
        });
        cx.notify();
    }

    /// `gr`: the references to what's under the cursor, gone through one at a time.
    pub(super) fn references(&mut self, cx: &mut Context<Self>) {
        if self.server_for("referencesProvider", "references").is_none() {
            return;
        }
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let position = convert::position(tab.doc.text(), tab.cursor());
        let answer = lsp.request("textDocument/references", json!({ "position": position, "context": { "includeDeclaration": true } }));
        cx.spawn(async move |this, cx| {
            let found = answer.await.ok().and_then(Result::ok).map(|v| convert::locations(&v)).unwrap_or_default();
            this.update(cx, |this, cx| {
                if found.is_empty() {
                    return this.project.say("no references found");
                }
                let root = this.project.root.clone();
                let n = found.len();
                let what = format!("{n} reference{}", if n == 1 { "" } else { "s" });
                let items: Vec<Item> = found
                    .iter()
                    .map(|(path, range)| {
                        let line = range.start.line as usize;
                        let text = this.project.tabs.text_of(path).map(str::to_string).or_else(|| std::fs::read_to_string(path).ok()).unwrap_or_default();
                        let file = path.strip_prefix(&root).unwrap_or(path).display().to_string();
                        Item::Note(Note {
                            file: Some(file),
                            at: vec![text.lines().nth(line).unwrap_or_default().to_string()],
                            text: what.clone(),
                            line: Some(line),
                        })
                    })
                    .collect();
                this.open_given(format!("go through the {what}"), items, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `space r`: rename what's under the cursor; the prompt starts with its name.
    pub(super) fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_for("renameProvider", "renaming").is_none() {
            return;
        }
        let Some(tab) = self.tab() else { return };
        let word = ghostwrite_core::textobject::text_object(&tab.doc.buffer, Range::point(tab.cursor()), 'w', false)
            .map(|r| tab.doc.text()[r.bytes()].to_string())
            .unwrap_or_default();
        self.open_prompt(PromptKind::Rename, &word, window, cx);
    }

    /// The new name, typed: renamed everywhere the server says.
    pub(super) fn rename_to(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let position = convert::position(tab.doc.text(), tab.cursor());
        let answer = lsp.request("textDocument/rename", json!({ "position": position, "newName": name }));
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            this.update(cx, |this, cx| match answer {
                Some(Ok(edit)) if !edit.is_null() => this.apply_workspace_edit(&edit, cx),
                Some(Err(e)) => this.project.say(format!("couldn't rename: {e}")),
                _ => this.project.say("nothing to rename here"),
            })
            .ok();
        })
        .detach();
    }

    /// `space .`: the server's code actions for the selection, and the diagnostics on it.
    pub(super) fn code_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_for("codeActionProvider", "code actions").is_none() {
            return;
        }
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let text = tab.doc.text();
        let bytes = tab.view.selection.bytes();
        let range = json!({ "start": convert::position(text, bytes.start), "end": convert::position(text, bytes.end) });
        // The diagnostics there, as the server sent them back would be best; their ranges and messages do.
        let line = tab.doc.buffer.line_of(tab.cursor());
        let diagnostics: Vec<Value> = tab
            .doc
            .diagnostics
            .iter()
            .filter(|d| tab.doc.buffer.line_of(d.range.start) <= line && line <= tab.doc.buffer.line_of(d.range.end))
            .map(|d| json!({ "range": { "start": convert::position(text, d.range.start), "end": convert::position(text, d.range.end) }, "message": d.message }))
            .collect();
        let answer = lsp.request("textDocument/codeAction", json!({ "range": range, "context": { "diagnostics": diagnostics } }));
        cx.spawn_in(window, async move |this, cx| {
            let actions = answer.await.ok().and_then(Result::ok).map(|v| convert::code_actions(&v)).unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                if actions.is_empty() {
                    return this.project.say("no code actions here");
                }
                let items = actions.into_iter().map(|a| (a.title.clone(), Pick::Action(Box::new(a)))).collect();
                this.show_picker(PickerKind::Actions, items, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Carry out a code action: its edit (resolved first, if the server
    /// left it out), then its command.
    pub(super) fn run_code_action(&mut self, action: CodeAction, cx: &mut Context<Self>) {
        let Some(lsp) = self.tab().and_then(|t| t.doc.lsp.as_ref()) else { return };
        if action.edit.is_none() && action.command.is_none() && lsp.supports("codeActionProvider") {
            let resolving = lsp.request_raw("codeAction/resolve", action.raw.clone());
            cx.spawn(async move |this, cx| {
                let resolved = resolving.await.ok().and_then(Result::ok);
                this.update(cx, |this, cx| match resolved.map(|v| convert::code_actions(&json!([v]))).and_then(|mut a| a.pop()) {
                    Some(action) if action.edit.is_some() || action.command.is_some() => this.run_code_action(action, cx),
                    _ => this.project.say("the server didn't say what that action does"),
                })
                .ok();
            })
            .detach();
            return;
        }
        if let Some(edit) = &action.edit {
            self.apply_workspace_edit(edit, cx);
        }
        let command = action.command.as_ref().map(|c| if c["command"].is_object() { c["command"].clone() } else { c.clone() });
        if let Some(command) = command.filter(|c| c["command"].is_string())
            && let Some(lsp) = self.tab().and_then(|t| t.doc.lsp.as_ref())
        {
            // What it changes comes back as `workspace/applyEdit`; a failure is said.
            let running = lsp.execute(&command);
            cx.spawn(async move |this, cx| {
                if let Ok(Err(e)) = running.await {
                    this.update(cx, |this, cx| {
                        this.project.say(format!("the action failed: {e}"));
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// `space s`: a symbol of the file, picked.
    pub(super) fn symbols(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_for("documentSymbolProvider", "symbols").is_none() {
            return;
        }
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref() else { return };
        let (answer, doc) = (lsp.request("textDocument/documentSymbol", json!({})), tab.doc.id);
        cx.spawn_in(window, async move |this, cx| {
            let found = answer.await.ok().and_then(Result::ok).map(|v| convert::symbols(&v)).unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                let Some(text) = this.project.tabs.by_id_mut(doc).map(|t| t.doc.text().to_string()) else { return };
                if found.is_empty() {
                    return this.project.say("no symbols in this file");
                }
                let items = found
                    .into_iter()
                    .map(|s| {
                        let within = if s.within.is_empty() { String::new() } else { format!(" in {}", s.within.join("::")) };
                        (format!("{}\t{}{within}", s.name, s.kind), Pick::At(doc, convert::offset(&text, s.range.start)))
                    })
                    .collect();
                this.show_picker(PickerKind::Symbols, items, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Whether saving the active file formats it first.
    pub(super) fn formats_on_save(&self) -> bool {
        crate::config::get().format_on_save && self.tab().and_then(|t| t.doc.lsp.as_ref()).is_some_and(|l| l.supports("documentFormattingProvider"))
    }

    /// `:format`, and saving (`then` writes it): format the file as its
    /// server does, as one undo step. If the text changes meanwhile, the
    /// formatting is dropped (it's of the old text).
    pub(super) fn format(&mut self, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) {
        let Some(tab) = self.tab() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref().filter(|l| l.supports("documentFormattingProvider")) else {
            self.project.say("no language server formats this file");
            return then(self, cx);
        };
        let indent = &tab.doc.indent;
        let options = json!({ "tabSize": indent.width, "insertSpaces": !indent.hard_tabs, "trimTrailingWhitespace": true, "insertFinalNewline": true });
        let (answer, doc, version) = (lsp.request("textDocument/formatting", json!({ "options": options })), tab.doc.id, tab.doc.buffer.version());
        cx.spawn(async move |this, cx| {
            // A server that's slow to format doesn't hold up saving for long.
            let timeout = cx.background_executor().timer(std::time::Duration::from_secs(3));
            let answer = futures::future::select(answer, Box::pin(timeout)).await;
            this.update(cx, |this, cx| {
                let edits = match answer {
                    futures::future::Either::Left((Ok(Ok(value)), _)) => convert::text_edits(&value),
                    _ => vec![],
                };
                let same = this.project.tabs.by_id_mut(doc).is_some_and(|t| t.doc.buffer.version() == version);
                if same && let Some(path) = this.project.tabs.by_id_mut(doc).and_then(|t| t.doc.path.clone()) {
                    this.edit_file(&path, &edits);
                }
                then(this, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After typing: the signature of the call being typed, after `(` or `,`
    /// (as the server says); gone after `)`.
    pub(super) fn signature_after_typing(&mut self, typed: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active() else { return };
        let Some(lsp) = tab.doc.lsp.as_ref().filter(|l| l.supports("signatureHelpProvider")) else { return };
        if typed.ends_with(')') {
            self.signature = None;
            return;
        }
        if !lsp.signature_triggers().iter().any(|t| typed.ends_with(t.as_str())) && self.signature.is_none() {
            return;
        }
        let (answer, asked) = self.signature_asked.ask(lsp, "textDocument/signatureHelp", tab.doc.text(), tab.view.selection.head);
        cx.spawn(async move |this, cx| {
            let found = answer.await.ok().and_then(Result::ok).and_then(|v| convert::signature(&v));
            this.update(cx, |this, cx| {
                if !this.signature_asked.answered(asked) {
                    return;
                }
                this.signature = found.filter(|_| this.modal.mode == ghostwrite_core::Mode::Insert).map(|(label, active)| Signature { label, active });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The signature being typed, above the cursor.
    pub(super) fn render_signature(&self, theme: &Theme) -> Option<AnyElement> {
        let signature = self.signature.as_ref().filter(|_| self.modal.mode == ghostwrite_core::Mode::Insert)?;
        let cursor = self.measured.cursor?;
        let label = &signature.label;
        let (before, active, after) = match signature
            .active
            .clone()
            .filter(|r| r.start <= r.end && r.end <= label.len() && label.is_char_boundary(r.start) && label.is_char_boundary(r.end))
        {
            Some(r) => (&label[..r.start], &label[r.clone()], &label[r.end..]),
            None => (label.as_str(), "", ""),
        };
        Some(
            deferred(
                anchored().position(point(cursor.left(), cursor.top() - theme.mono_size * 2.)).child(
                    div()
                        .occlude()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .font_family(theme.mono_font.clone())
                        .text_size(theme.mono_size)
                        .max_w(pt(640.))
                        .flex()
                        .child(before.to_string())
                        .child(div().font_weight(gpui::FontWeight::BOLD).text_color(theme.accent).child(active.to_string()))
                        .child(after.to_string()),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
