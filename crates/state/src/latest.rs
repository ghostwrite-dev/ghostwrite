//! Questions to a language server where only the latest answer counts.

/// Questions to a server where only the latest answer counts (completions,
/// the signature, a panel's): a newer question cancels the ones still
/// out, and an older one's answer, if it comes anyway, is dropped.
#[derive(Default)]
pub struct Latest {
    asked: u64,
    /// The questions still out, to cancel.
    pending: Vec<i64>,
}

impl Latest {
    /// Ask `method` about `offset` in `text`, cancelling what's still out:
    /// the answer, and the question's number, to check it's still the latest.
    pub fn ask(
        &mut self,
        lsp: &ghostwrite_lsp::Document,
        method: &str,
        text: &ghostwrite_core::Rope,
        offset: usize,
    ) -> (ghostwrite_lsp::document::Answer, u64) {
        self.cancel(lsp);
        self.asked += 1;
        let (id, answer) = lsp.ask_cancelable(method, text, offset);
        self.pending.push(id);
        (answer, self.asked)
    }

    /// Another question with the latest (asked together, both answers wanted).
    pub fn also(&mut self, lsp: &ghostwrite_lsp::Document, method: &str, text: &ghostwrite_core::Rope, offset: usize) -> ghostwrite_lsp::document::Answer {
        let (id, answer) = lsp.ask_cancelable(method, text, offset);
        self.pending.push(id);
        answer
    }

    /// Whether question `n`'s answer is the one wanted; if it is, nothing's out.
    pub fn answered(&mut self, n: u64) -> bool {
        let latest = n == self.asked;
        if latest {
            self.pending.clear();
        }
        latest
    }

    /// Cancel what's still out: its answers aren't wanted.
    pub fn cancel(&mut self, lsp: &ghostwrite_lsp::Document) {
        for id in self.pending.drain(..) {
            lsp.cancel(id);
        }
    }
}
