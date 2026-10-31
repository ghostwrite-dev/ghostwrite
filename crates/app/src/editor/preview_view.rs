//! A Typst file's pages ([`super::preview`]), one under another beside its
//! code, with the cursor and the selection drawn on the glyphs they're on. A
//! click puts the cursor on the glyph clicked; the wheel scrolls.
//!
//! Pages are drawn in the background, as wide as they're shown. A page
//! that's the same as before isn't drawn again, and while one that changed
//! is, the old drawing stays up ([`Drawn`]).

use std::collections::HashMap;
use std::sync::Arc;

use ghostwrite_core::Mode;
use ghostwrite_typst::{Document, Glyph};
use gpui::{
    App, Bounds, ContentMask, Corners, DispatchPhase, Element, ElementId, Entity, FocusHandle, Global, GlobalElementId, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, RenderImage, ScrollWheelEvent, Style, Window, fill, point, px, relative, size,
};

use super::Editor;
use super::document::DocId;
use crate::theme::theme;
use crate::ui::page_cache::{self, PageCache};

/// Between pages, and around them.
const GAP: f32 = 12.;

pub struct PreviewView {
    editor: Entity<Editor>,
    focus: FocusHandle,
}

impl PreviewView {
    pub fn new(editor: Entity<Editor>, focus: FocusHandle) -> Self {
        Self { editor, focus }
    }
}

/// Where the pages are, in the view before scrolling.
#[derive(Clone)]
struct Layout {
    left: Pixels,
    width: Pixels,
    tops: Vec<Pixels>,
    /// Pixels to a point, for each page.
    scales: Vec<f32>,
    bottom: Pixels,
}

impl Layout {
    fn new(document: &Document, view: Bounds<Pixels>) -> Self {
        let width = (view.size.width - px(GAP * 2.)).max(px(100.));
        let (mut tops, mut scales, mut y) = (vec![], vec![], px(GAP));
        for page in &document.pages {
            let scale = f32::from(width) / page.width.max(1.);
            tops.push(y);
            scales.push(scale);
            y += px(page.height * scale + GAP);
        }
        Self { left: px(GAP), width, tops, scales, bottom: y }
    }

    fn page(&self, document: &Document, page: usize) -> Bounds<Pixels> {
        Bounds::new(point(self.left, self.tops[page]), size(self.width, px(document.pages[page].height * self.scales[page])))
    }

    fn rect(&self, page: usize, glyph: &Glyph) -> Bounds<Pixels> {
        let s = self.scales[page];
        Bounds::new(point(self.left + px(glyph.x * s), self.tops[page] + px(glyph.y * s)), size(px(glyph.width * s), px(glyph.height * s)))
    }

    /// The byte of the text under `at`: the glyph there, or the nearest
    /// on that page (lines count more than columns).
    fn hit(&self, document: &Document, at: Point<Pixels>) -> Option<usize> {
        let page = self.tops.iter().rposition(|top| *top <= at.y)?;
        let distance = |glyph: &Glyph| {
            let r = self.rect(page, glyph);
            let dx = (r.left() - at.x).max(at.x - r.right()).max(px(0.));
            let dy = (r.top() - at.y).max(at.y - r.bottom()).max(px(0.));
            f32::from(dy) * 4. + f32::from(dx)
        };
        let glyphs = document.pages[page].glyphs.iter().filter(|g| g.source.is_some());
        let glyph = glyphs.min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        let source = glyph.source.clone()?;
        // On its right half: after it.
        let r = self.rect(page, glyph);
        Some(if at.x > r.left() + r.size.width / 2. { source.end } else { source.start })
    }
}

/// The glyph made from byte `at` of the text, and its page; or, for a byte
/// nothing on the pages comes from (markup), the next one that is if `near`.
fn glyph_at(document: &Document, at: usize, near: bool) -> Option<(usize, &Glyph)> {
    let glyphs = || document.pages.iter().enumerate().flat_map(|(ix, page)| page.glyphs.iter().map(move |g| (ix, g)));
    let exact = glyphs().find(|(_, g)| g.source.as_ref().is_some_and(|s| s.contains(&at)));
    exact.or_else(|| {
        near.then(|| glyphs().filter(|(_, g)| g.source.as_ref().is_some_and(|s| s.start >= at)).min_by_key(|(_, g)| g.source.as_ref().map(|s| s.start)))
            .flatten()
    })
}

pub struct Prepainted {
    layout: Layout,
    scroll_top: Pixels,
    pages: Vec<(Bounds<Pixels>, Option<Arc<RenderImage>>)>,
    quads: Vec<PaintQuad>,
    document: Arc<Document>,
}

impl IntoElement for PreviewView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PreviewView {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepainted>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Prepainted> {
        let theme = theme(cx).clone();
        let focused = self.focus.is_focused(window);
        let (id, document, selection, cursor, mode, scroll_top, reveal) = {
            let editor = self.editor.read(cx);
            let tab = editor.project.tabs.active()?;
            let preview = tab.doc.preview.as_ref()?;
            (tab.doc.id, preview.document.clone()?, tab.view.selection, tab.cursor(), editor.modal.mode, preview.scroll_top, preview.reveal)
        };
        let layout = Layout::new(&document, bounds);
        let view_height = bounds.size.height;

        // Bring what was asked for into view, a third of the way down.
        let mut top = scroll_top;
        if let Some((at, always)) = reveal
            && let Some((page, glyph)) = glyph_at(&document, at, true)
        {
            let r = layout.rect(page, glyph);
            if always || r.top() < top || r.bottom() > top + view_height {
                top = r.top() - view_height / 3.;
            }
        }
        let scroll_top = top.clamp(px(0.), (layout.bottom - view_height).max(px(0.)));
        let shift = |r: Bounds<Pixels>| Bounds::new(point(bounds.left() + r.left(), bounds.top() + r.top() - scroll_top), r.size);

        // The pages on screen, and their drawings when they're ready.
        let pixels = (f32::from(layout.width) * window.scale_factor()).round() as usize;
        let mut pages = vec![];
        for page in 0..document.pages.len() {
            let on_screen = shift(layout.page(&document, page));
            if on_screen.bottom() >= bounds.top() && on_screen.top() <= bounds.bottom() {
                pages.push((on_screen, drawn(id, &document, page, pixels, window, cx)));
            }
        }

        // The selection and the cursor, on the glyphs they're on.
        let mut quads = vec![];
        if !matches!(mode, Mode::Tree | Mode::Terminal) {
            let text = self.editor.read(cx).project.tabs.active().map(|t| t.doc.text().len()).unwrap_or(0);
            let range = selection.bytes();
            if selection.from() != selection.to() && range.end <= text {
                for (ix, page) in document.pages.iter().enumerate() {
                    for glyph in page.glyphs.iter().filter(|g| g.source.as_ref().is_some_and(|s| s.start < range.end && range.start < s.end)) {
                        quads.push(fill(shift(layout.rect(ix, glyph)), theme.selection));
                    }
                }
            }
            if let Some((page, glyph)) = glyph_at(&document, cursor, false) {
                // Pages are white whatever the theme: a box that reads on paper, and lets the letter through.
                let color = if mode == Mode::Select { theme.cursor_select } else { gpui::rgb(0x2563eb).into() };
                let r = layout.rect(page, glyph);
                let r = Bounds::new(r.origin, size(r.size.width.max(px(3.)), r.size.height));
                quads.push(fill(shift(r), color.opacity(if focused { 0.4 } else { 0.15 })));
            }
        }

        self.editor.update(cx, |editor, _| {
            if let Some(preview) = editor.tab_mut().and_then(|t| t.doc.preview.as_mut()) {
                (preview.scroll_top, preview.reveal) = (scroll_top, None);
            }
        });
        Some(Prepainted { layout, scroll_top, pages, quads, document })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Option<Prepainted>,
        window: &mut Window,
        _: &mut App,
    ) {
        let Some(state) = state else { return };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (at, image) in &state.pages {
                window.paint_quad(fill(*at, gpui::white()));
                if let Some(image) = image {
                    window.paint_image(*at, Corners::default(), image.clone(), 0, false).ok();
                }
            }
            for quad in state.quads.drain(..) {
                window.paint_quad(quad);
            }
        });
        self.listen(bounds, state, window);
    }
}

impl PreviewView {
    /// A click puts the cursor on the glyph clicked; the wheel scrolls.
    fn listen(&self, bounds: Bounds<Pixels>, state: &Prepainted, window: &mut Window) {
        let (layout, scroll_top, document) = (state.layout.clone(), state.scroll_top, state.document.clone());
        let (editor, focus) = (self.editor.clone(), self.focus.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !bounds.contains(&event.position) {
                return;
            }
            let at = point(event.position.x - bounds.left(), event.position.y - bounds.top() + scroll_top);
            let Some(byte) = layout.hit(&document, at) else { return };
            window.focus(&focus);
            editor.update(cx, |editor, cx| editor.click(byte, event.modifiers.shift, event.click_count, cx));
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(px(20.));
            editor.update(cx, |editor, cx| editor.scroll_preview(delta, cx));
        });
    }
}

/// A page drawn: what's on it, and how many pixels wide.
type Key = (u128, usize);

/// Drawn pages, and the last drawing of each page of each file, shown while it's drawn again.
#[derive(Default)]
struct Drawn {
    cache: PageCache<Key>,
    latest: HashMap<(DocId, usize), Key>,
}

impl Global for Drawn {}

/// Page `page` of `document` drawn `pixels` wide (or a little wider), if
/// it's ready; if it isn't, it's started, and its last drawing is shown meanwhile.
fn drawn(id: DocId, document: &Document, page: usize, pixels: usize, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
    let pixels = page_cache::width(pixels);
    let key = (document.pages[page].hash, pixels);
    let drawn = cx.default_global::<Drawn>();
    let meanwhile = |drawn: &mut Drawn| drawn.latest.get(&(id, page)).copied().and_then(|old| drawn.cache.get(&old)).flatten();
    match drawn.cache.get(&key) {
        Some(Some(image)) => {
            drawn.latest.insert((id, page), key);
            return Some(image);
        }
        Some(None) => return meanwhile(drawn),
        None => {}
    }
    let shown = meanwhile(drawn);
    for image in drawn.cache.drawing(key) {
        cx.drop_image(image, Some(window));
    }
    let source = document.pages[page].clone();
    let width = source.width;
    let drawing = cx.background_executor().spawn(async move {
        let bitmap = source.render(pixels as f32 / width.max(1.));
        let frame = image::Frame::new(image::RgbaImage::from_raw(bitmap.width as u32, bitmap.height as u32, bitmap.bgra)?);
        Some(Arc::new(RenderImage::new(vec![frame])))
    });
    cx.spawn(async move |cx| {
        let image = drawing.await;
        drop(cx.update(|cx| {
            let drawn = cx.default_global::<Drawn>();
            // Unless it was dropped while it was being drawn.
            if drawn.cache.drawn(&key, image) {
                drawn.latest.insert((id, page), key);
                cx.refresh_windows();
            }
        }));
    })
    .detach();
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_lands_on_the_text_of_the_glyph_clicked() {
        let root = std::env::temp_dir().join(format!("ghostwrite-hit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text = "#set page(width: 300pt, height: 200pt)\nThe cat sat.\n";
        let document = ghostwrite_typst::Compiler::new(&root, &root.join("a.typ")).unwrap().compile(text).unwrap();
        let layout = Layout::new(&document, Bounds::new(point(px(0.), px(0.)), size(px(624.), px(800.))));
        let cat = text.find("cat").unwrap();
        let (page, glyph) = glyph_at(&document, cat, false).expect("the c of cat");
        let r = layout.rect(page, glyph);
        assert_eq!(layout.hit(&document, point(r.left() + px(1.), r.top() + px(2.))), Some(cat), "its left half: before it");
        assert_eq!(layout.hit(&document, point(r.right() - px(1.), r.top() + px(2.))), Some(cat + 1), "its right half: after it");
        assert_eq!(layout.hit(&document, point(r.left(), r.top() - px(30.))), Some(cat), "above the line: the nearest glyph");
        assert_eq!(
            glyph_at(&document, 1, true).map(|(_, g)| g.source.clone()),
            Some(Some(text.find("The").unwrap()..text.find("The").unwrap() + 1)),
            "from markup, the text after it"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
