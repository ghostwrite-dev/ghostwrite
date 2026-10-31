//! A Typst file's pages, beside its code (`:preview` hides them), compiled
//! in-process ([`ghostwrite_typst`]) as it's typed, saved or not. Each
//! glyph knows the text it came from, so the cursor and the selection show
//! on the page too, and a click on the page puts the cursor on what was
//! clicked. The pages stay where they are while the cursor moves around;
//! an edit that leaves it on another line, or `space j` (to the next problem), brings them to it.
//! [`super::preview_view`] draws them.

use std::sync::{Arc, Mutex};

use ghostwrite_project::store;
use ghostwrite_typst::{Compiler, Document};
use gpui::{AppContext as _, Context, MouseMoveEvent, Pixels, Point, Window, px};

use super::Editor;

pub struct Preview {
    compiler: Arc<Mutex<Compiler>>,
    /// The pages, once it's compiled.
    pub document: Option<Arc<Document>>,
    /// Why the text as it is now doesn't compile.
    pub error: Option<String>,
    pub scroll_top: Pixels,
    /// A byte of the text to bring into view next time it's drawn, and
    /// whether to even if it's in view (`space j`).
    pub reveal: Option<(usize, bool)>,
    /// The buffer version compiled last, or being compiled.
    compiled: Option<u64>,
    compiling: bool,
    /// The line the pages were last brought to.
    pub(super) line: Option<usize>,
}

/// How wide the pages are: half the room, until their edge is dragged.
pub struct PreviewPane {
    pub width: Option<f32>,
    pub resizing: bool,
}

impl PreviewPane {
    pub fn new() -> Self {
        Self { width: store::preview_width(), resizing: false }
    }
}

impl Editor {
    /// Keep the active tab's pages, if it's Typst, up to date: compile it
    /// again when its text has changed, one compile at a time.
    pub(super) fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        if !self.project.previews {
            return;
        }
        let root = self.project.root.clone();
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let doc = &mut tab.doc;
        let Some(path) = doc.path.clone().filter(|_| doc.language.is_some_and(|l| l.name == "typst")) else { return };
        if doc.preview.is_none() {
            let Some(compiler) = Compiler::new(&root, &path) else { return };
            let compiler = Arc::new(Mutex::new(compiler));
            doc.preview =
                Some(Preview { compiler, document: None, error: None, scroll_top: px(0.), reveal: None, compiled: None, compiling: false, line: None });
        }
        let version = doc.buffer.version();
        let Some(preview) = doc.preview.as_mut().filter(|p| !p.compiling && p.compiled != Some(version)) else { return };
        // After an edit (not opening it), the pages go to it.
        let edited = preview.compiled.is_some();
        (preview.compiling, preview.compiled) = (true, Some(version));
        let (compiler, text, id) = (preview.compiler.clone(), doc.buffer.text().to_string(), doc.id);
        let work = cx.background_spawn(async move { compiler.lock().map_err(|_| "the compiler crashed".to_string())?.compile(&text) });
        cx.spawn(async move |this, cx| {
            let done = work.await;
            this.update(cx, |this, cx| {
                let Some(tab) = this.project.tabs.by_id_mut(id) else { return };
                let cursor = tab.cursor();
                let line = tab.doc.buffer.line_of(cursor);
                let Some(preview) = tab.doc.preview.as_mut() else { return };
                preview.compiling = false;
                match done {
                    Ok(document) => (preview.document, preview.error) = (Some(Arc::new(document)), None),
                    Err(e) => preview.error = Some(e),
                }
                if edited && preview.line.replace(line) != Some(line) {
                    preview.reveal = Some((cursor, false));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Bring the pages to the cursor, even if it's in view (after `space j`).
    pub(super) fn bring_pages(&mut self, cx: &mut Context<Self>) {
        let previews = self.project.previews;
        let Some(tab) = self.project.tabs.active_mut() else { return };
        let cursor = tab.cursor();
        let line = tab.doc.buffer.line_of(cursor);
        if let Some(preview) = tab.doc.preview.as_mut().filter(|p| previews && p.document.is_some()) {
            (preview.reveal, preview.line) = (Some((cursor, true)), Some(line));
            cx.notify();
        }
    }

    /// `:preview`: show or hide the pages beside a Typst file, and the goals beside a Lean one.
    pub(super) fn toggle_preview(&mut self) {
        self.project.previews = !self.project.previews;
        self.project.say(if self.project.previews { "Typst pages and Lean goals shown beside their files" } else { "Typst pages and Lean goals hidden" });
    }

    pub(super) fn scroll_preview(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(preview) = self.tab_mut().and_then(|t| t.doc.preview.as_mut()) {
            preview.scroll_top = (preview.scroll_top - delta.y).max(px(0.));
            cx.notify();
        }
    }

    /// While dragging the pages' edge.
    pub(super) fn drag_preview_edge(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if self.preview_pane.resizing {
            let room = f32::from(window.viewport_size().width);
            let width = (room - f32::from(event.position.x)).clamp(200., (room - 300.).max(200.));
            self.preview_pane.width = Some(width / crate::theme::theme(cx).zoom);
            cx.notify();
        }
    }

    pub(super) fn end_preview_drag(&mut self) {
        if std::mem::take(&mut self.preview_pane.resizing) {
            self.preview_pane.width.inspect(|w| store::save_preview_width(*w));
        }
    }
}
