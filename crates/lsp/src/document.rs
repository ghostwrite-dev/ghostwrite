//! One open file as its language server knows it. Changes are sent whole;
//! dropping the document closes it on the server.

use std::path::Path;
use std::sync::Arc;

use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::client::Client;
use crate::convert::{position, uri};

pub struct Document {
    pub(crate) client: Arc<Client>,
    language: &'static str,
    pub(crate) uri: String,
    version: i32,
}

/// A request's answer, once the server gives it.
pub type Answer = oneshot::Receiver<Result<Value, String>>;

impl Document {
    pub fn open(client: Arc<Client>, language: &'static str, path: &Path, text: &str) -> Self {
        let doc = Self { client, language, uri: uri(path), version: 1 };
        doc.did_open(text);
        doc
    }

    pub fn server(&self) -> &'static str {
        self.client.name
    }

    /// The version of the text the server was last sent.
    pub fn version(&self) -> i32 {
        self.version
    }

    /// Whether its server is still running.
    pub fn alive(&self) -> bool {
        self.client.alive()
    }

    pub fn trigger_characters(&self) -> Vec<String> {
        self.client.trigger_characters()
    }

    fn did_open(&self, text: &str) {
        let doc = json!({ "uri": self.uri, "languageId": self.language, "version": self.version, "text": text });
        self.client.notify("textDocument/didOpen", json!({ "textDocument": doc }));
    }

    /// The text changed: by `edits` (as [`ghostwrite_core::Edit`]s record
    /// them), if there are and the server takes edits; else it's sent `text` whole.
    pub fn changed(&mut self, text: &str, edits: Option<Vec<ghostwrite_core::Edit>>) {
        self.version += 1;
        let changes = match edits.filter(|_| self.client.incremental_sync()) {
            Some(edits) => edits
                .into_iter()
                .map(|e| {
                    let range = json!({ "start": { "line": e.start.0, "character": e.start.1 }, "end": { "line": e.end.0, "character": e.end.1 } });
                    json!({ "range": range, "text": e.text })
                })
                .collect(),
            None => vec![json!({ "text": text })],
        };
        self.client.notify("textDocument/didChange", json!({ "textDocument": { "uri": self.uri, "version": self.version }, "contentChanges": changes }));
    }

    pub fn saved(&self) {
        self.client.notify("textDocument/didSave", json!({ "textDocument": { "uri": self.uri } }));
    }

    /// Ask about `offset` in `text` (`textDocument/hover`, `…/definition`, `…/completion`).
    pub fn ask(&self, method: &str, text: &str, offset: usize) -> Answer {
        self.ask_cancelable(method, text, offset).1
    }

    /// [`Document::ask`], and the question's id, for [`Document::cancel`].
    pub fn ask_cancelable(&self, method: &str, text: &str, offset: usize) -> (i64, Answer) {
        self.client.request_cancelable(method, json!({ "textDocument": { "uri": self.uri }, "position": position(text, offset) }))
    }

    /// Ask `method` about the document, with `params` (it's named in them).
    pub fn request(&self, method: &str, mut params: Value) -> Answer {
        params["textDocument"] = json!({ "uri": self.uri });
        self.client.request(method, params)
    }

    /// Ask `method` with `params` just as they are (`codeAction/resolve`).
    pub fn request_raw(&self, method: &str, params: Value) -> Answer {
        self.client.request(method, params)
    }

    /// Run a server command (a code action's).
    pub fn execute(&self, command: &Value) -> Answer {
        self.client.request("workspace/executeCommand", json!({ "command": command["command"], "arguments": command["arguments"] }))
    }

    /// Whether its server does `provider` (see [`Client::supports`]).
    pub fn supports(&self, provider: &str) -> bool {
        self.client.supports(provider)
    }

    pub fn signature_triggers(&self) -> Vec<String> {
        self.client.signature_triggers()
    }

    /// The answer to question `id` isn't wanted any more.
    pub fn cancel(&self, id: i64) {
        self.client.cancel(id);
    }

    /// The file's semantic tokens (`textDocument/semanticTokens/full`); or,
    /// given the id of the last ones it sent, only what's changed since, if
    /// the server can (`…/full/delta`). [`convert::token_data`](crate::convert::token_data) reads either.
    pub fn semantic_tokens(&self, previous: Option<&str>) -> Answer {
        let document = json!({ "uri": self.uri });
        match previous.filter(|_| self.client.semantic_delta()) {
            Some(id) => self.client.request("textDocument/semanticTokens/full/delta", json!({ "textDocument": document, "previousResultId": id })),
            None => self.client.request("textDocument/semanticTokens/full", json!({ "textDocument": document })),
        }
    }

    pub fn semantic_legend(&self) -> Option<(Vec<String>, Vec<String>)> {
        self.client.semantic_legend()
    }

    fn close(&self) {
        self.client.notify("textDocument/didClose", json!({ "textDocument": { "uri": self.uri } }));
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.close();
    }
}
