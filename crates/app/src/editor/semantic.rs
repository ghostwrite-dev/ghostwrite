//! Colors from the language server (its semantic tokens), over the
//! grammar's: the server knows what each name is (a local, a constant, a
//! type, Lean's own notation), which a grammar can't. While the text
//! changes, the last ones it sent stay, moved past the edit, until it sends
//! new ones. Only the active file's are asked for.

use ghostwrite_core::syntax::{Span, scope_named};
use ghostwrite_lsp::convert;
use gpui::Context;

use super::Editor;

#[derive(Default)]
pub struct Semantic {
    /// The server's spans, and the text they're of.
    pub spans: Vec<Span>,
    pub text: String,
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
}

impl Editor {
    /// Keep the active file's server colors up to date: one question at a time.
    pub(super) fn refresh_semantic(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let doc = &mut tab.doc;
        let Some(lsp) = doc.lsp.as_ref() else { return };
        let version = doc.buffer.version();
        let semantic = &mut doc.semantic;
        if semantic.unsupported || semantic.asking || semantic.asked == Some(version) {
            return;
        }
        (semantic.asking, semantic.asked) = (true, Some(version));
        let previous = semantic.last.as_ref().map(|(id, _)| id.as_str());
        let (answer, text, id) = (lsp.semantic_tokens(previous), doc.buffer.text().to_string(), doc.id);
        cx.spawn(async move |this, cx| {
            let value = answer.await.ok().and_then(Result::ok);
            this.update(cx, |this, cx| {
                let Some(doc) = this.project.tabs.by_id_mut(id).map(|t| &mut t.doc) else { return };
                doc.semantic.asking = false;
                // Asked before the server was ready, it says only now whether it has them.
                let Some(legend) = doc.lsp.as_ref().and_then(|l| l.semantic_legend()) else {
                    doc.semantic.unsupported = true;
                    return;
                };
                // An error (Lean's when the text changed meanwhile) keeps the last ones.
                let previous = doc.semantic.last.take().map(|(_, data)| data);
                let Some((result, data)) = value.and_then(|v| convert::token_data(&v, previous.as_deref())) else { return };
                let tokens = convert::semantic_tokens(&data, &text, &legend);
                doc.semantic.last = result.map(|id| (id, data));
                doc.semantic.spans = tokens.into_iter().filter_map(|(range, name)| Some(Span { range, scope: scope_named(name)? })).collect();
                doc.semantic.text = text;
                doc.recolor();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
