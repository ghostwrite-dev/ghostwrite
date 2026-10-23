//! Language servers in the editor: keeping documents in sync, the completion
//! menu and hover popup, `gd`, `space k` (docs as an inline note) and
//! `space d` (a tour of the file's diagnostics, like `:sh` errors).

use std::cell::{Cell, RefCell};

use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{CompletionProvider, Rope};
use lsp_types::{CompletionContext, CompletionResponse, CompletionTextEdit};
use serde_json::{Value, json};

use super::*;
use crate::lsp::{self, Client};
use crate::shell::Location;

/// A document as a language server knows it.
pub(super) struct Doc {
    pub(super) client: Arc<Client>,
    language: &'static str,
    uri: RefCell<String>,
    version: Cell<i32>,
}

impl Doc {
    pub(super) fn open(client: Arc<Client>, language: &'static str, path: &Path, text: &str) -> Rc<Self> {
        let uri = lsp::uri(path);
        client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": language, "version": 1, "text": text } }));
        Rc::new(Self { client, language, uri: RefCell::new(uri), version: Cell::new(1) })
    }

    /// The whole text, as sent after every change.
    pub(super) fn changed(&self, text: &str) {
        self.version.set(self.version.get() + 1);
        self.client.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": *self.uri.borrow(), "version": self.version.get() }, "contentChanges": [{ "text": text }] }),
        );
    }

    pub(super) fn saved(&self) {
        self.client.notify("textDocument/didSave", json!({ "textDocument": { "uri": *self.uri.borrow() } }));
    }

    pub(super) fn moved(&self, path: &Path, text: &str) {
        self.closed();
        *self.uri.borrow_mut() = lsp::uri(path);
        self.client.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": *self.uri.borrow(), "languageId": self.language, "version": self.version.get(), "text": text } }),
        );
    }

    pub(super) fn closed(&self) {
        self.client.notify("textDocument/didClose", json!({ "textDocument": { "uri": *self.uri.borrow() } }));
    }

    fn at(&self, text: &str, offset: usize) -> Value {
        json!({ "textDocument": { "uri": *self.uri.borrow() }, "position": lsp::position(text, offset) })
    }

    /// A request about `offset`, answered on a background thread.
    pub(super) fn ask(&self, method: &str, text: &str, offset: usize, cx: &App) -> Task<Result<Value, String>> {
        let rx = self.client.request(method, self.at(text, offset));
        cx.background_executor().spawn(async move { rx.await.map_err(|_| "the language server went away".to_string())? })
    }
}

/// LSP position (UTF-16 columns) → the editor's (char columns).
pub(super) fn to_editor(text: &str, pos: lsp_types::Position) -> lsp_types::Position {
    let offset = lsp::offset(text, pos);
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    lsp_types::Position::new(pos.line, text[line_start..offset].chars().count() as u32)
}

fn range_to_editor(text: &str, r: lsp_types::Range) -> lsp_types::Range {
    lsp_types::Range::new(to_editor(text, r.start), to_editor(text, r.end))
}

/// Hover contents as plain text, code fences turned into indented blocks.
pub(super) fn hover_text(value: &Value) -> Option<String> {
    let contents = &value["contents"];
    let raw = match contents {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or_default().to_string(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| p.as_str().map(str::to_string).or_else(|| p["value"].as_str().map(str::to_string)).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };
    let mut out = vec![];
    let mut in_code = false;
    for line in raw.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        out.push(if in_code { format!("    {line}") } else { line.to_string() });
    }
    let text = out.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The editor's providers, for one document.
pub(super) struct Provider(pub(super) Rc<Doc>);

impl CompletionProvider for Provider {
    fn completions(&self, text: &Rope, offset: usize, _: CompletionContext, _: &mut Window, cx: &mut App) -> Task<anyhow::Result<CompletionResponse>> {
        let text = text.to_string();
        let task = self.0.ask("textDocument/completion", &text, offset, cx);
        cx.background_executor().spawn(async move {
            let value = task.await.map_err(anyhow::Error::msg)?;
            if value.is_null() {
                return Ok(CompletionResponse::Array(vec![]));
            }
            let mut response: CompletionResponse = serde_json::from_value(value)?;
            let items = match &mut response {
                CompletionResponse::Array(items) => items,
                CompletionResponse::List(list) => &mut list.items,
            };
            // Edits come in UTF-16 columns; the editor counts chars.
            for item in items.iter_mut() {
                match &mut item.text_edit {
                    Some(CompletionTextEdit::Edit(edit)) => edit.range = range_to_editor(&text, edit.range),
                    Some(CompletionTextEdit::InsertAndReplace(edit)) => {
                        edit.insert = range_to_editor(&text, edit.insert);
                        edit.replace = range_to_editor(&text, edit.replace);
                    }
                    None => {}
                }
                for edit in item.additional_text_edits.iter_mut().flatten() {
                    edit.range = range_to_editor(&text, edit.range);
                }
            }
            Ok(response)
        })
    }

    /// The menu opens on the server's trigger characters, and as you type a word.
    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        let Some(last) = new_text.chars().last() else { return false };
        last.is_alphanumeric() || last == '_' || self.0.client.trigger_characters().iter().any(|t| new_text.ends_with(t.as_str()))
    }
}

/// The first location in a definition response: a path and its LSP position.
fn first_location(value: &Value) -> Option<(PathBuf, lsp_types::Position)> {
    let one = if value.is_array() { value.get(0)? } else { value };
    let uri = one["uri"].as_str().or_else(|| one["targetUri"].as_str())?;
    let range = if one["targetSelectionRange"].is_object() { &one["targetSelectionRange"] } else { &one["range"] };
    let start = serde_json::from_value(range["start"].clone()).ok()?;
    Some((lsp::path_of(uri)?, start))
}

impl TextEditor {
    /// Start (or reuse) the file's language server and open the document.
    pub(super) fn attach_lsp(&mut self, path: Option<&Path>, text: &str) -> Option<Rc<Doc>> {
        let path = path?;
        let (client, language) = self.servers.for_path(path)?;
        Some(Doc::open(client, language, path, text))
    }

    /// Diagnostics from a server: onto the buffer they're for.
    pub(super) fn apply_diagnostics(&mut self, path: &Path, diagnostics: Vec<lsp_types::Diagnostic>, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path.as_deref() == Some(path)) else { return };
        buffer.state.update(cx, |st, cx| {
            let text = st.text().clone();
            let plain = text.to_string();
            if let Some(set) = st.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics.into_iter().map(|mut d| {
                    d.range = range_to_editor(&plain, d.range);
                    Diagnostic::from(d)
                }));
            }
            cx.notify();
        });
    }

    fn cursor_request(&self, method: &str, cx: &App) -> Option<(Task<Result<Value, String>>, &'static str)> {
        let buffer = self.buffers.get(self.active)?;
        let doc = buffer.doc.as_ref()?;
        let st = buffer.state.read(cx);
        let text = st.text().to_string();
        // In normal mode the block sits on a char; ask about that char.
        let offset = st.selected_range().start;
        Some((doc.ask(method, &text, offset, cx), doc.client.name))
    }

    /// `gd`: jump to the definition of what's under the cursor.
    pub(super) fn goto_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((task, _)) = self.cursor_request("textDocument/definition", cx) else {
            self.status = Some("no language server for this file".into());
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some((path, pos)) = result.ok().as_ref().and_then(first_location) else {
                    this.status = Some("no definition found".into());
                    return cx.notify();
                };
                this.open_paths(vec![path], window, cx);
                let Some(buffer) = this.buffers.get(this.active) else { return };
                let state = buffer.state.clone();
                state.update(cx, |st, cx| {
                    let text = st.text().to_string();
                    let at = lsp::offset(&text, pos);
                    this.modal.select_bytes(at..at, st, cx);
                    let line = text[..at].matches('\n').count();
                    st.center_on_lines(line..line + 1, px(0.), cx);
                });
            })
            .ok();
        })
        .detach();
    }

    /// `space k`: the docs of what's under the cursor, as a note folded out
    /// under the line; Enter asks Claude about them.
    pub(super) fn show_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((task, server)) = self.cursor_request("textDocument/hover", cx) else {
            self.status = Some("no language server for this file".into());
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result.ok().as_ref().and_then(hover_text) {
                Some(docs) => this.show_note(&docs, window, cx),
                None => {
                    this.status = Some(format!("{server}: nothing to show here").into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// `space d`: tour this file's diagnostics, like `:sh` errors.
    pub(super) fn diagnostics_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.get(self.active) else { return };
        let Some(path) = buffer.path.clone() else { return };
        let st = buffer.state.read(cx);
        let text = st.text().to_string();
        let mut stops: Vec<(usize, Location)> = st
            .diagnostics()
            .map(|set| {
                set.iter()
                    .map(|entry| {
                        let severity = match entry.diagnostic.severity {
                            DiagnosticSeverity::Error => "error",
                            DiagnosticSeverity::Warning => "warning",
                            DiagnosticSeverity::Info => "info",
                            DiagnosticSeverity::Hint => "hint",
                        };
                        let line = text[..entry.range.start.min(text.len())].matches('\n').count();
                        let message = format!("{severity}: {}", entry.diagnostic.message);
                        (entry.range.start, Location { path: path.clone(), line, message: message.lines().map(str::to_string).collect() })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if stops.is_empty() {
            self.status = Some("no diagnostics in this file".into());
            return cx.notify();
        }
        stops.sort_by_key(|(at, _)| *at);
        let count = stops.len();
        self.start_tour(format!("{count} diagnostic{}", if count == 1 { "" } else { "s" }), stops.into_iter().map(|(_, l)| l).collect(), window, cx);
    }
}
