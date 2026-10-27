//! tinymist's live preview of a Typst file, served to the browser by the
//! server itself: it follows the text as it's typed (not just saved), and
//! scrolls to where the cursor is. Clicking in it comes back as
//! [`Event::Show`](crate::Event::Show). Dropping it stops it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use crate::client::Client;
use crate::convert::{path_of, position};
use crate::document::Document;

pub struct Preview {
    client: Arc<Client>,
    task: String,
    path: PathBuf,
    /// The line last scrolled to, so moving along a line doesn't scroll.
    line: Option<u32>,
}

impl Preview {
    /// Preview `doc` if its server is tinymist; `served` gets the page's
    /// URL once it's up.
    pub fn start(doc: &Document, served: impl FnOnce(String) + Send + 'static) -> Option<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        if doc.server() != "tinymist" {
            return None;
        }
        let path = path_of(&doc.uri)?;
        let task = format!("ghostwrite-{}", NEXT.fetch_add(1, Ordering::SeqCst));
        // Port 0: each preview gets its own.
        let args = ["--task-id", &task, "--data-plane-host", "127.0.0.1:0", &path.to_string_lossy()].map(str::to_string);
        let answer = doc.client.execute("tinymist.doStartPreview", json!([args]));
        std::thread::spawn(move || {
            let Ok(Ok(result)) = futures::executor::block_on(answer) else { return };
            if let Some(addr) = result["staticServerAddr"].as_str() {
                served(format!("http://{addr}"));
            }
        });
        Some(Self { client: doc.client.clone(), task, path, line: None })
    }

    /// Scroll to `offset` in the file's `text`, if that's another line.
    pub fn scroll(&mut self, text: &str, offset: usize) {
        let at = position(text, offset);
        if self.line.replace(at.line) == Some(at.line) {
            return;
        }
        let request = json!({ "event": "panelScrollTo", "filepath": self.path, "line": at.line, "character": at.character });
        drop(self.client.execute("tinymist.scrollPreview", json!([self.task, request])));
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        drop(self.client.execute("tinymist.doKillPreview", json!([self.task])));
    }
}
