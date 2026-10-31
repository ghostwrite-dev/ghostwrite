//! A PDF in its tab: its pages one under another, with the selection and
//! the cursor drawn where that text is on them. Clicks go to the text under
//! them, so moving and selecting (and copying) work as they do in code. It's
//! read-only, and Claude doesn't comment on it.
//!
//! Pages are drawn in the background, as wide as they're shown, and the
//! last few are kept ([`Pages`]).

use std::ops::Range as Bytes;
use std::path::PathBuf;
use std::sync::Arc;

use ghostwrite_core::{Buffer, Mode};
use ghostwrite_pdf::Pdf;
use gpui::{
    App, Bounds, ContentMask, Corners, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Global, GlobalElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, RenderImage, ScrollWheelEvent, Style, Window, fill, point, px, relative, size,
};

use super::element::Metrics;
use super::tabs::Autoscroll;
use super::{Editor, Measured};
use crate::theme::theme;
use crate::ui::page_cache::{self, PageCache};

/// Between pages, and above the first.
const GAP: f32 = 12.;
/// The widest a page is shown, before zooming.
const MAX_WIDTH: f32 = 900.;

pub struct PdfView {
    editor: Entity<Editor>,
    focus: FocusHandle,
}

impl PdfView {
    /// The active tab, a PDF.
    pub fn new(editor: Entity<Editor>, focus: FocusHandle) -> Self {
        Self { editor, focus }
    }
}

/// Where the pages are, in the view, before scrolling.
#[derive(Clone)]
struct Layout {
    left: Pixels,
    width: Pixels,
    /// Pixels to a point.
    scale: f32,
    tops: Vec<Pixels>,
    heights: Vec<Pixels>,
    bottom: Pixels,
}

impl Layout {
    fn new(pdf: &Pdf, view: Bounds<Pixels>, zoom: f32) -> Self {
        let width = (view.size.width - px(GAP * 2.)).min(px(MAX_WIDTH * zoom)).max(px(100.));
        let left = (view.size.width - width) / 2.;
        let (mut tops, mut heights, mut y) = (vec![], vec![], px(GAP));
        let scale = |page: &ghostwrite_pdf::Size| f32::from(width) / page.width.max(1.);
        for page in &pdf.pages {
            let height = px(page.height * scale(page));
            tops.push(y);
            heights.push(height);
            y += height + px(GAP);
        }
        let scale = pdf.pages.first().map_or(1., scale);
        Self { left, width, scale, tops, heights, bottom: y }
    }

    /// Characters `cols` of line `line`, in the view before scrolling.
    fn rect(&self, pdf: &Pdf, line: usize, cols: Bytes<usize>) -> Option<Bounds<Pixels>> {
        let l = pdf.lines.get(line)?;
        let x = |col: usize| self.left + px(l.xs[col.min(l.xs.len() - 1)] * self.scale);
        let top = self.tops[l.page] + px(l.top * self.scale);
        Some(Bounds::from_corners(point(x(cols.start), top), point(x(cols.end), top + px((l.bottom - l.top) * self.scale))))
    }

    /// The parts of lines that `range` (bytes of `buffer`) covers.
    fn spans(&self, pdf: &Pdf, buffer: &Buffer, range: Bytes<usize>) -> Vec<Bounds<Pixels>> {
        let text = buffer.text();
        let (first, last) = (buffer.line_of(range.start), buffer.line_of(range.end.max(range.start)));
        let col = |line: usize, at: usize| text[buffer.line_start(line)..at.clamp(buffer.line_start(line), buffer.line_end(line))].chars().count();
        (first..=last)
            .filter_map(|line| {
                let from = if line == first { col(line, range.start) } else { 0 };
                let to = if line == last { col(line, range.end) } else { usize::MAX };
                self.rect(pdf, line, from..to).filter(|r| r.size.width > px(0.) && r.size.height > px(0.))
            })
            .collect()
    }

    /// The line and character under `at` (in the view before scrolling):
    /// the nearest character on the page there, or the page's first line.
    fn hit(&self, pdf: &Pdf, at: Point<Pixels>) -> (usize, usize) {
        let page = self.tops.iter().rposition(|top| *top <= at.y).unwrap_or(0);
        let (x, y) = (f32::from(at.x - self.left) / self.scale, f32::from(at.y - self.tops[page]) / self.scale);
        pdf.hit(page, x, y).unwrap_or_else(|| (pdf.lines.iter().position(|l| l.page == page).unwrap_or(0), 0))
    }

    /// The byte of `buffer`, a PDF's text, under `at`.
    fn hit_text(&self, pdf: &Pdf, buffer: &Buffer, at: Point<Pixels>) -> usize {
        let (line, col) = self.hit(pdf, at);
        let start = buffer.line_start(line.min(buffer.last_line()));
        let text = &buffer.text()[start..buffer.line_end(line.min(buffer.last_line()))];
        start + text.char_indices().nth(col).map_or(text.len(), |(i, _)| i)
    }
}

pub struct Prepainted {
    layout: Layout,
    scroll_top: Pixels,
    pages: Vec<(Bounds<Pixels>, Option<Arc<RenderImage>>)>,
    quads: Vec<PaintQuad>,
    line_height: Pixels,
}

impl IntoElement for PdfView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PdfView {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

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
    ) -> Prepainted {
        let theme = theme(cx).clone();
        let metrics = Metrics::measure(&theme, window);
        let focused = self.focus.is_focused(window);
        let (pdf, path, mode, selection, autoscroll, scroll_top) = {
            let editor = self.editor.read(cx);
            let tab = editor.project.tabs.active().expect("a PDF view shows the active tab");
            let view = &tab.view;
            let pdf = tab.doc.pdf.clone().expect("a PDF view shows a PDF");
            (pdf, tab.doc.path.clone().unwrap_or_default(), editor.modal.mode, view.selection, view.autoscroll.clone(), view.scroll_top)
        };
        let layout = Layout::new(&pdf, bounds, theme.zoom);

        // Scroll to what was asked for.
        let (cursor_line, line_box) = {
            let editor = self.editor.read(cx);
            let buffer = &editor.project.tabs.active().expect("checked above").doc.buffer;
            (buffer.line_of(selection.cursor(buffer.text())), |line: usize| layout.rect(&pdf, line, 0..usize::MAX))
        };
        let view_height = bounds.size.height;
        let mut top = scroll_top;
        match &autoscroll {
            Some(Autoscroll::Fit) => {
                if let Some(r) = line_box(cursor_line) {
                    let margin = (metrics.line_height * crate::config::get().margin_lines).min(view_height / 3.);
                    if r.top() - margin < top {
                        top = r.top() - margin;
                    } else if r.bottom() + margin > top + view_height {
                        top = r.bottom() + margin - view_height;
                    }
                }
            }
            Some(Autoscroll::Top) => top = line_box(cursor_line).map_or(top, |r| r.top()),
            Some(Autoscroll::Bottom) => top = line_box(cursor_line).map_or(top, |r| r.bottom() - view_height),
            Some(Autoscroll::Center(lines)) => {
                let last = lines.end.max(lines.start + 1) - 1;
                if let (Some(from), Some(to)) = (line_box(lines.start), line_box(last)) {
                    let (from, to) = (from.top(), to.bottom());
                    top = if to - from > view_height { from - metrics.line_height } else { (from + to) / 2. - view_height / 2. };
                }
            }
            None => {}
        }
        let scroll_top = top.clamp(px(0.), (layout.bottom - view_height).max(px(0.)));
        let shift = |r: Bounds<Pixels>| Bounds::new(point(bounds.left() + r.left(), bounds.top() + r.top() - scroll_top), r.size);

        // The pages on screen, and their drawings when they're ready.
        let pixels = (f32::from(layout.width) * window.scale_factor()).round() as usize;
        let mut pages = vec![];
        for (page, (&page_top, &height)) in layout.tops.iter().zip(&layout.heights).enumerate() {
            let on_screen = shift(Bounds::new(point(layout.left, page_top), size(layout.width, height)));
            if on_screen.bottom() < bounds.top() || on_screen.top() > bounds.bottom() {
                continue;
            }
            pages.push((on_screen, drawn(&path, page, pixels, window, cx)));
        }

        // The selection and the cursor.
        let mut quads = vec![];
        let rows = (f32::from(view_height) / f32::from(metrics.line_height)).floor() as usize;
        self.editor.update(cx, |editor, _| {
            if !matches!(mode, Mode::Tree | Mode::Terminal) {
                let buffer = &editor.project.tabs.active().expect("checked above").doc.buffer;
                // Pages are white whatever the theme: a box that reads on paper, and lets the letter through.
                let cursor = (if mode == Mode::Select { theme.cursor_select } else { gpui::rgb(0x2563eb).into() }).opacity(if focused { 0.4 } else { 0.15 });
                let marks = selection_marks(&layout, &pdf, buffer, selection, cursor, &theme, metrics.line_height);
                quads.extend(marks.into_iter().map(|(r, color)| fill(shift(r), color)));
            }
            let Some(tab) = editor.project.tabs.active_mut() else { return };
            (tab.view.scroll_top, tab.view.autoscroll) = (scroll_top, None);
            editor.measured = Measured { rows, cursor: None, top_line: None };
        });
        Prepainted { layout, scroll_top, pages, quads, line_height: metrics.line_height }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
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

impl PdfView {
    /// Clicks place the cursor in the text; the wheel scrolls.
    fn listen(&self, bounds: Bounds<Pixels>, state: &Prepainted, window: &mut Window) {
        let (layout, scroll_top, line_height) = (state.layout.clone(), state.scroll_top, state.line_height);
        let hit = move |editor: &Editor, position: Point<Pixels>| -> Option<usize> {
            let doc = &editor.project.tabs.active()?.doc;
            let at = point(position.x - bounds.left(), position.y - bounds.top() + scroll_top);
            Some(layout.hit_text(doc.pdf.as_ref()?, &doc.buffer, at))
        };
        let (editor, focus) = (self.editor.clone(), self.focus.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !bounds.contains(&event.position) {
                return;
            }
            window.focus(&focus);
            editor.update(cx, |editor, cx| {
                if let Some(at) = hit(editor, event.position) {
                    editor.click(at, event.modifiers.shift, event.click_count, cx);
                }
            });
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(line_height);
            editor.update(cx, |editor, cx| editor.scroll_by(delta, cx));
        });
    }
}

/// The selection over `buffer` (a PDF's text), and the cursor in `cursor`:
/// a box over its char.
fn selection_marks(
    layout: &Layout,
    pdf: &Pdf,
    buffer: &Buffer,
    selection: ghostwrite_core::Range,
    cursor: gpui::Hsla,
    theme: &crate::theme::Theme,
    line_height: Pixels,
) -> Vec<(Bounds<Pixels>, gpui::Hsla)> {
    let mut marks = vec![];
    let text = buffer.text();
    if selection.is_wide(text) {
        marks.extend(layout.spans(pdf, buffer, selection.bytes()).into_iter().map(|r| (r, theme.selection)));
    }
    let at = selection.cursor(text);
    let line = buffer.line_of(at);
    let start = buffer.line_start(line);
    let col = text[start..at.max(start)].chars().count();
    if let Some(r) = layout.rect(pdf, line, col..col + 1) {
        marks.push((Bounds::new(r.origin, size(r.size.width.max(px(4.)), r.size.height.max(line_height * 0.8))), cursor));
    }
    marks
}

/// A drawn page: the file, the page, and how many pixels wide.
/// A drawn page: the file (as of when it last changed), the page, and how many pixels wide.
type Key = (PathBuf, Option<std::time::SystemTime>, usize, usize);

/// Drawn pages.
#[derive(Default)]
struct Pages(PageCache<Key>);

impl Global for Pages {}

/// Page `page` of `path` drawn `pixels` wide (or a little wider), if it's
/// ready (or, while it's being drawn, at another width). If it isn't, it's
/// started, and the windows redraw when it's done.
fn drawn(path: &std::path::Path, page: usize, pixels: usize, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
    let pixels = page_cache::width(pixels);
    // A file that's changed is drawn again (the old drawing shown meanwhile).
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key = (path.to_path_buf(), modified, page, pixels);
    let Pages(cache) = cx.default_global::<Pages>();
    let meanwhile = |cache: &mut PageCache<Key>| cache.any(|(p, _, n, _)| p == path && *n == page);
    match cache.get(&key) {
        Some(Some(image)) => return Some(image),
        Some(None) => return meanwhile(cache),
        None => {}
    }
    let shown = meanwhile(cache);
    for image in cache.drawing(key.clone()) {
        cx.drop_image(image, Some(window));
    }
    let drawing = cx.background_executor().spawn(async move { draw(&key) });
    let key = (path.to_path_buf(), modified, page, pixels);
    cx.spawn(async move |cx| {
        let image = drawing.await;
        drop(cx.update(|cx| {
            // Unless it was dropped while it was being drawn.
            if cx.default_global::<Pages>().0.drawn(&key, image) {
                cx.refresh_windows();
            }
        }));
    })
    .detach();
    shown
}

fn draw((path, _, page, pixels): &Key) -> Option<Arc<RenderImage>> {
    let bitmap = ghostwrite_pdf::render(path, *page, *pixels).ok()?;
    // gpui draws images from BGRA, which is what PDFKit drew.
    let frame = image::Frame::new(image::RgbaImage::from_raw(bitmap.width as u32, bitmap.height as u32, bitmap.bgra)?);
    Some(Arc::new(RenderImage::new(vec![frame])))
}
