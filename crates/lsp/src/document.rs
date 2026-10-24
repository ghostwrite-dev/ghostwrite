//! One open file as its language server knows it. Changes are sent whole;
//! dropping the document closes it on the server.

use std::path::Path;
use std::sync::Arc;

use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::client::Client;
use crate::convert::{position, uri};

pub struct Document {
    client: Arc<Client>,
    language: &'static str,
    uri: String,
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

    pub fn trigger_characters(&self) -> Vec<String> {
        self.client.trigger_characters()
    }

    fn did_open(&self, text: &str) {
        let doc = json!({ "uri": self.uri, "languageId": self.language, "version": self.version, "text": text });
        self.client.notify("textDocument/didOpen", json!({ "textDocument": doc }));
    }

    pub fn changed(&mut self, text: &str) {
        self.version += 1;
        self.client
            .notify("textDocument/didChange", json!({ "textDocument": { "uri": self.uri, "version": self.version }, "contentChanges": [{ "text": text }] }));
    }

    pub fn saved(&self) {
        self.client.notify("textDocument/didSave", json!({ "textDocument": { "uri": self.uri } }));
    }

    /// The file was renamed: reopen it under its new name.
    pub fn moved(&mut self, path: &Path, text: &str) {
        self.close();
        self.uri = uri(path);
        self.did_open(text);
    }

    /// Ask about `offset` in `text` (`textDocument/hover`, `…/definition`, `…/completion`).
    pub fn ask(&self, method: &str, text: &str, offset: usize) -> Answer {
        self.client.request(method, json!({ "textDocument": { "uri": self.uri }, "position": position(text, offset) }))
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
