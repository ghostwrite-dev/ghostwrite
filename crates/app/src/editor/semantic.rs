//! Asking the language server for its colors (see [`ghostwrite_view::semantic`]).
//! Only the active file's are asked for.

use gpui::Context;

use super::Editor;

impl Editor {
    /// Keep the active file's server colors up to date: one question at a time.
    pub(super) fn refresh_semantic(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let doc = &mut tab.doc;
        let Some(lsp) = doc.lsp.as_ref() else { return };
        let version = doc.buffer.version();
        let Some(previous) = doc.semantic.ask(version) else { return };
        let (answer, text, id) = (lsp.semantic_tokens(previous), doc.buffer.text().to_string(), doc.id);
        cx.spawn(async move |this, cx| {
            let value = answer.await.ok().and_then(Result::ok);
            this.update(cx, |this, cx| {
                let Some(doc) = this.project.tabs.by_id_mut(id).map(|t| &mut t.doc) else { return };
                if doc.semantic_answered(value, text) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
