//! Colors from the language server (its semantic tokens), over the
//! grammar's: the server knows what each name is (a local, a constant, a
//! type, a language's own notation), which a grammar can't. While the text
//! changes, the last ones it sent stay, moved past the edit, until it sends
//! new ones. Only the active file's are asked for.

use ghostwrite_core::Rope;
use ghostwrite_core::syntax::{Span, scope_named};
use ghostwrite_lsp::convert;
use serde_json::Value;

use crate::document::Document;

#[derive(Default)]
pub struct Semantic {
    /// The server's spans, and the buffer version they're of.
    pub spans: Vec<Span>,
    pub version: u64,
    /// The id of the server's last answer and its tokens, encoded, for it
    /// to send only what's changed next time.
    last: Option<(String, Vec<u32>)>,
    /// The buffer version asked about last.
    asked: Option<u64>,
    asking: bool,
    /// The server has none.
    unsupported: bool,
}

impl Semantic {
    /// Ask again, even if the text hasn't changed (the server's tokens did).
    pub fn stale(&mut self) {
        self.asked = None;
    }

    /// Whether to ask about `version` now (one question at a time): if so, it's
    /// asked, and this is the id of the server's last answer, to send only what's changed.
    pub fn ask(&mut self, version: u64) -> Option<Option<&str>> {
        if self.unsupported || self.asking || self.asked == Some(version) {
            return None;
        }
        (self.asking, self.asked) = (true, Some(version));
        Some(self.last.as_ref().map(|(id, _)| id.as_str()))
    }
}

impl Document {
    /// The server's answer about `text` (the buffer's version `version`), if it has
    /// one: its colors, from now on. Whether they're new.
    pub fn semantic_answered(&mut self, value: Option<Value>, text: Rope, version: u64) -> bool {
        self.semantic.asking = false;
        // Asked before the server was ready, it says only now whether it has them.
        let Some(legend) = self.lsp.as_ref().and_then(|l| l.semantic_legend()) else {
            self.semantic.unsupported = true;
            return false;
        };
        // An error (some servers' when the text changed meanwhile) keeps the last ones.
        let previous = self.semantic.last.take().map(|(_, data)| data);
        let Some((result, data)) = value.and_then(|v| convert::token_data(&v, previous.as_deref())) else { return false };
        let tokens = convert::semantic_tokens(&data, &text, &legend);
        self.semantic.last = result.map(|id| (id, data));
        self.semantic.spans = tokens.into_iter().filter_map(|(range, name)| Some(Span { range, scope: scope_named(name)? })).collect();
        self.semantic.version = version;
        true
    }
}
