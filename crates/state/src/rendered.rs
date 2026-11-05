//! Pages of a document, whatever makes them ([`ghostwrite_page::Pages`]): a plugin
//! makes a file's from its text in the background, saved or not, as it changes
//! ([`ghostwrite_page::Renderer`]), or if the file is only pages, from the file.
//!
//! A document whose pages are made from its text shows them in a tab of its own,
//! which `:preview` opens: a view of that document, with no text of its own
//! ([`Document::view_of`]). Where they're scrolled to is the document's tab's
//! ([`PagesScroll`]); the app makes and draws them.
//!
//! [`Document::view_of`]: crate::document::Document::view_of

use std::sync::{Arc, Mutex};

use ghostwrite_page::{Pages, Renderer};

use crate::tabs::Tab;

/// A document's pages, and what makes them.
#[derive(Default)]
pub struct Rendered {
    /// The pages, once there are any.
    pub pages: Option<Arc<dyn Pages>>,
    /// Why the text as it is now can't be made into pages.
    pub error: Option<String>,
    pub renderer: Option<Arc<Mutex<Box<dyn Renderer>>>>,
    /// The buffer version made into pages last, or being made.
    pub made: Option<u64>,
    pub making: bool,
}

impl Rendered {
    /// Whether pages are being made right now.
    pub fn is_making(&self) -> bool {
        self.making
    }

    pub fn made_by(renderer: Result<Box<dyn Renderer>, String>) -> Self {
        match renderer {
            Ok(renderer) => Self { renderer: Some(Arc::new(Mutex::new(renderer))), ..Self::default() },
            Err(error) => Self { error: Some(error), ..Self::default() },
        }
    }
}

/// A file that's only pages has them: its text is its lines for them, as it's now made.
pub fn tab_of_only_pages(tab: &mut Tab, done: Result<Arc<dyn Pages>, String>) {
    let Some(rendered) = tab.doc.rendered.as_mut() else { return };
    rendered.making = false;
    match done {
        Ok(pages) => {
            let text = pages.text().unwrap_or_default().to_string();
            (rendered.pages, rendered.error) = (Some(pages), None);
            if *tab.doc.buffer.rope() != text {
                tab.reload(text);
            }
            let version = tab.doc.buffer.version();
            if let Some(rendered) = tab.doc.rendered.as_mut() {
                rendered.made = Some(version);
            }
        }
        Err(why) => rendered.error = Some(why),
    }
}

/// Where a document's pages are scrolled to, and how far they're zoomed: their own,
/// whatever the editor's zoom.
pub struct PagesScroll {
    /// In points, down and across.
    pub top: f32,
    pub left: f32,
    /// Times as wide as fits the view.
    pub zoom: f32,
    /// A byte of the text to bring into view next time they're drawn, and
    /// whether to even if it's in view (`space j`).
    pub reveal: Option<(usize, bool)>,
    /// The line they were last brought to.
    pub line: Option<usize>,
}

impl Default for PagesScroll {
    fn default() -> Self {
        Self { top: 0., left: 0., zoom: 1., reveal: None, line: None }
    }
}

/// The steps ⌘= and ⌘- zoom pages through.
const PAGE_ZOOMS: [f32; 15] = [0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.8, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];

impl PagesScroll {
    /// ⌘= ⌘- ⌘0: a step in or out (`by`), or back to fitting the view (0).
    pub fn zoom_step(&mut self, by: i32) {
        let now = PAGE_ZOOMS.iter().position(|&z| z >= self.zoom - 0.001).unwrap_or(PAGE_ZOOMS.len() - 1);
        let zoom = match by {
            0 => 1.,
            // Between steps, in goes to the next one up and out to the one below.
            _ if by < 0 && PAGE_ZOOMS[now] > self.zoom + 0.001 => PAGE_ZOOMS[now.saturating_sub(1)],
            _ => PAGE_ZOOMS[now.saturating_add_signed(by as isize).min(PAGE_ZOOMS.len() - 1)],
        };
        let ratio = zoom / self.zoom;
        (self.zoom, self.top, self.left) = (zoom, self.top * ratio, if by == 0 { 0. } else { self.left * ratio });
    }
}
