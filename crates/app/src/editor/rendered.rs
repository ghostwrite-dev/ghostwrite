//! Pages of a document, whatever makes them ([`ghostwrite_page::Pages`]): a plugin
//! makes a file's from its text in the background, saved or not, as it changes
//! ([`ghostwrite_page::Renderer`]), or if the file is only pages, from the file.
//!
//! A document whose pages are made from its text shows them in a tab of its own,
//! which `:preview` opens: a view of that document, with no text of its own
//! ([`Document::view_of`]). The cursor and the selection show on them too, and a
//! click puts the file's cursor on what's clicked, for going back to it. They stay where they are while the
//! cursor moves around; an edit that leaves it on another line, or `space j` (to
//! the next problem), brings them to it. [`super::page_view`] draws them; where
//! they're scrolled to is the document's tab's ([`PagesScroll`]).
//!
//! [`Document::view_of`]: super::document::Document::view_of

use std::sync::{Arc, Mutex};

use ghostwrite_core::{Key, Range};
use ghostwrite_page::{OnlyPages, Pages, Renderer};
use gpui::{AppContext as _, Context, Pixels, Point, px};

use super::Editor;
use super::document::{DocId, Document};
use super::tabs::Autoscroll;

/// How far a key scrolls pages.
const LINE: f32 = 40.;

/// A document's pages, and what makes them.
#[derive(Default)]
pub struct Rendered {
    /// The pages, once there are any.
    pub pages: Option<Arc<dyn Pages>>,
    /// Why the text as it is now can't be made into pages.
    pub error: Option<String>,
    renderer: Option<Arc<Mutex<Box<dyn Renderer>>>>,
    /// The buffer version made into pages last, or being made.
    pub(super) made: Option<u64>,
    making: bool,
}

impl Rendered {
    /// Whether pages are being made right now.
    #[cfg(test)]
    pub(super) fn is_making(&self) -> bool {
        self.making
    }

    fn made_by(renderer: Result<Box<dyn Renderer>, String>) -> Self {
        match renderer {
            Ok(renderer) => Self { renderer: Some(Arc::new(Mutex::new(renderer))), ..Self::default() },
            Err(error) => Self { error: Some(error), ..Self::default() },
        }
    }
}

/// A file that's only pages has them: its text is its lines for them, as it's now made.
fn tab_of_only_pages(tab: &mut super::tabs::Tab, done: Result<Arc<dyn Pages>, String>, cx: &mut Context<Editor>) {
    let Some(rendered) = tab.doc.rendered.as_mut() else { return };
    rendered.making = false;
    match done {
        Ok(pages) => {
            let text = pages.text().unwrap_or_default().to_string();
            (rendered.pages, rendered.error) = (Some(pages), None);
            if tab.doc.buffer.text() != text {
                tab.reload(text);
            }
            let version = tab.doc.buffer.version();
            if let Some(rendered) = tab.doc.rendered.as_mut() {
                rendered.made = Some(version);
            }
        }
        Err(why) => rendered.error = Some(why),
    }
    cx.notify();
}

/// Where a document's pages are scrolled to.
#[derive(Default)]
pub struct PagesScroll {
    pub top: Pixels,
    /// A byte of the text to bring into view next time they're drawn, and
    /// whether to even if it's in view (`space j`).
    pub reveal: Option<(usize, bool)>,
    /// The line they were last brought to.
    pub(super) line: Option<usize>,
}

impl Editor {
    /// Keep the pages of the document `id` up to date: make them again when its
    /// text has changed, once at a time.
    pub(super) fn refresh_rendered(&mut self, id: DocId, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let Some(tab) = self.project.tabs.by_id_mut(id) else { return };
        let doc = &mut tab.doc;
        let Some(path) = doc.path.clone() else { return };
        if doc.rendered.is_none() {
            let Some(renderer) = doc.language.and_then(|language| ghostwrite_plugin::renderer(language, &root, &path)) else { return };
            doc.rendered = Some(Rendered::made_by(renderer));
        }
        let only_pages = doc.is_only_pages();
        let at_own_size = doc.language.is_some_and(|language| language.pages_at_own_size);
        let version = doc.buffer.version();
        // What's only pages is made from the file: its text is only lines to move by.
        let text = if only_pages { String::new() } else { doc.buffer.text().to_string() };
        let Some(rendered) = doc.rendered.as_mut().filter(|r| r.renderer.is_some() && !r.making && r.made != Some(version)) else { return };
        // After an edit (not opening it), the pages go to it.
        let edited = rendered.made.is_some();
        (rendered.making, rendered.made) = (true, Some(version));
        let renderer = rendered.renderer.clone();
        let work = cx.background_spawn(async move {
            let renderer = renderer.ok_or("nothing to make them")?;
            // Locks are unwrapped elsewhere; this one guards a renderer's work, and if
            // it ever panics, that's said, and the file can still be edited.
            let mut renderer = renderer.lock().map_err(|_| "making the pages crashed".to_string())?;
            ghostwrite_project::contained("making the pages", || renderer.render(&text)).and_then(|pages| pages)
        });
        cx.spawn(async move |this, cx| {
            let done = work.await;
            this.update(cx, |this, cx| {
                let done = done.map(|pages| if only_pages { Arc::new(OnlyPages::new(pages, at_own_size)) as Arc<dyn Pages> } else { pages });
                if let (true, Err(why)) = (only_pages, &done) {
                    this.project.say(why.clone());
                }
                let Some(tab) = this.project.tabs.by_id_mut(id) else { return };
                if only_pages {
                    return tab_of_only_pages(tab, done, cx);
                }
                let cursor = tab.cursor();
                let line = tab.doc.buffer.line_of(cursor);
                let Some(rendered) = tab.doc.rendered.as_mut() else { return };
                rendered.making = false;
                match done {
                    Ok(pages) => (rendered.pages, rendered.error) = (Some(pages), None),
                    Err(e) => rendered.error = Some(e),
                }
                let scroll = &mut tab.view.pages;
                if edited && scroll.line.replace(line) != Some(line) {
                    scroll.reveal = Some((cursor, false));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `:preview`: from a file to the tab of its pages, and back. `false` if it
    /// has no pages to make.
    pub(super) fn preview_pages(&mut self, cx: &mut Context<Self>) -> Result<bool, String> {
        let tab = self.current()?;
        if let Some((file, _)) = &tab.doc.view_of {
            let ix = self.project.tabs.index_of_id(*file).ok_or("its file is closed")?;
            self.activate(ix, cx);
            return Ok(true);
        }
        if !tab.doc.language.is_some_and(ghostwrite_plugin::renders) {
            return Ok(false);
        }
        if tab.doc.path.is_none() {
            return Err("save it first: its pages are made from where it is".into());
        }
        let (id, name, cursor) = (tab.doc.id, tab.doc.name().to_string(), tab.cursor());
        let open = self.project.tabs.iter().find(|t| t.doc.is_view_of(id)).map(|t| t.doc.id);
        let ix = match open.and_then(|view| self.project.tabs.index_of_id(view)) {
            Some(ix) => ix,
            None => self.project.tabs.add(|view| Document::view(view, id, format!("{name} · pages"))),
        };
        // To where the cursor is in the file.
        if let Some(file) = self.project.tabs.by_id_mut(id) {
            file.view.pages.reveal = Some((cursor, true));
        }
        self.activate(ix, cx);
        Ok(true)
    }

    /// Bring the pages to the cursor, even if it's in view (after `space j`).
    pub(super) fn bring_pages(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.project.tabs.active_mut() else { return };
        if tab.doc.rendered.as_ref().is_none_or(|r| r.pages.is_none()) {
            return;
        }
        let cursor = tab.cursor();
        let line = tab.doc.buffer.line_of(cursor);
        (tab.view.pages.reveal, tab.view.pages.line) = (Some((cursor, true)), Some(line));
        cx.notify();
    }

    /// The document whose pages the active tab shows, if it shows any.
    pub(super) fn shown_pages(&self) -> Option<DocId> {
        self.pages_of(self.tab()?.doc.id)
    }

    /// The document whose pages the tab of `doc` shows, if it shows pages.
    pub(super) fn pages_of(&self, doc: DocId) -> Option<DocId> {
        let doc = &self.project.tabs.by_id(doc)?.doc;
        doc.view_of.as_ref().map(|(file, _)| *file).or_else(|| doc.pages_are_the_text().then_some(doc.id))
    }

    /// The wheel on pages: those of the tab of `doc`, or of the active tab.
    pub(super) fn scroll_pages(&mut self, doc: Option<DocId>, delta: Point<Pixels>, cx: &mut Context<Self>) {
        let shown = doc.map_or_else(|| self.shown_pages(), |doc| self.pages_of(doc));
        if let Some(file) = shown.and_then(|id| self.project.tabs.by_id_mut(id)) {
            file.view.pages.top = (file.view.pages.top - delta.y).max(px(0.));
            cx.notify();
        }
    }

    /// A click on pages, on the text at `byte` if it's on any: the cursor goes there,
    /// in the document they're of. A view stays where it is, and the file's tab has
    /// the cursor there when it's next shown; a double click goes to that tab.
    ///
    /// `doc`: they're in the other pane, in that tab. When they're the pages of the file
    /// that has the keys, the cursor moves there and the keys stay; pages of anything
    /// else get the keys first.
    pub(super) fn click_page_of(&mut self, doc: Option<DocId>, byte: Option<usize>, shift: bool, count: usize, cx: &mut Context<Self>) {
        if let Some(doc) = doc {
            let file = self.pages_of(doc);
            if file.is_some() && file == self.tab().map(|tab| tab.doc.id) {
                if let Some((byte, tab)) = byte.zip(self.project.tabs.active_mut()) {
                    let at = tab.doc.text().floor_char_boundary(byte);
                    let selection = if shift { Range::new(tab.view.selection.anchor, at) } else { Range::point(at) };
                    let line = tab.doc.buffer.line_of(at);
                    tab.select(selection, Autoscroll::Center(line..line + 1));
                    cx.notify();
                }
                return;
            }
            self.go_to_doc(doc, cx);
        }
        let view = self.tab().is_some_and(|t| t.doc.view_of.is_some());
        if !view {
            if let Some(byte) = byte {
                self.click(byte, shift, count, cx);
            }
            return;
        }
        if let Some((byte, file)) = byte.zip(self.shown_pages().and_then(|id| self.project.tabs.by_id_mut(id))) {
            let at = file.doc.text().floor_char_boundary(byte);
            let selection = if shift { Range::new(file.view.selection.anchor, at) } else { Range::point(at) };
            let line = file.doc.buffer.line_of(at);
            file.select(selection, Autoscroll::Center(line..line + 1));
            cx.notify();
        }
        if count >= 2
            && let Err(why) = self.preview_pages(cx)
        {
            self.project.say(why);
        }
    }

    /// A key on a tab of pages that has no cursor of its own: it moves them.
    pub(super) fn page_key(&mut self, key: &Key, cx: &mut Context<Self>) -> bool {
        if !self.tab().is_some_and(|t| t.doc.view_of.is_some()) {
            return false;
        }
        let rows = self.measured.rows.max(2) as f32;
        let delta = match key {
            Key::Char('j') | Key::Down => -LINE,
            Key::Char('k') | Key::Up => LINE,
            Key::Ctrl('d') => -LINE * rows,
            Key::Ctrl('u') => LINE * rows,
            _ => return false,
        };
        self.scroll_pages(None, Point::new(px(0.), px(delta)), cx);
        true
    }
}
