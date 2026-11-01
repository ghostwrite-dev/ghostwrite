//! Pages, one under another, in their tab: how they're laid out and drawn,
//! and painted ([`paint`]). What's on a page and where its text is, is
//! each view's own.
//!
//! Drawn pages are kept as images so
//! scrolling doesn't draw them again. They're big (a page is ~10–20 MB on
//! a Retina screen), so there's a budget: past it, the pages used longest
//! ago go, never one used in the last couple of seconds (it's on screen).
//! Widths are rounded up, so dragging a pane's edge doesn't draw every page
//! again for every pixel; while a page is drawn again, its last drawing is
//! shown, scaled.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ghostwrite_page::{Bitmap, Size};
use gpui::{App, Bounds, ContentMask, Corners, Global, PaintQuad, Pixels, Point, RenderImage, Window, fill, point, px, size};

/// Between pages, and around them.
const GAP: f32 = 12.;

/// Bytes of drawn pages kept at most.
const BUDGET: usize = 256 << 20;
/// Pages used this recently stay, over budget or not.
const IN_USE: Duration = Duration::from_secs(2);
/// Widths are drawn at multiples of this many pixels.
const STEP: usize = 128;

/// A width to draw at: `pixels`, rounded up.
fn width(pixels: usize) -> usize {
    pixels.div_ceil(STEP).max(1) * STEP
}

struct Entry {
    /// `None` while it's being drawn, or if it couldn't be.
    image: Option<Arc<RenderImage>>,
    used: Instant,
}

/// The cache, by `K` (what's on a page, and its width).
struct PageCache<K> {
    entries: HashMap<K, Entry>,
}

impl<K> Default for PageCache<K> {
    fn default() -> Self {
        Self { entries: HashMap::new() }
    }
}

fn bytes(image: &RenderImage) -> usize {
    (0..image.frame_count()).map(|i| image.as_bytes(i).map_or(0, <[u8]>::len)).sum()
}

impl<K: Hash + Eq + Clone> PageCache<K> {
    /// The drawing of `key`, if it's ready; `None` while it's drawn (`Some(None)`) or if it's not asked for yet.
    fn get(&mut self, key: &K) -> Option<Option<Arc<RenderImage>>> {
        let entry = self.entries.get_mut(key)?;
        entry.used = Instant::now();
        Some(entry.image.clone())
    }

    /// `key` is being drawn: it's kept a place for, and the budget made
    /// room for. Returns the drawings dropped, for the caller to let go of
    /// on the GPU (`cx.drop_image`).
    fn drawing(&mut self, key: K) -> Vec<Arc<RenderImage>> {
        self.entries.insert(key, Entry { image: None, used: Instant::now() });
        self.make_room()
    }

    /// `key`'s drawing is done; `false` if it was dropped meanwhile.
    fn drawn(&mut self, key: &K, image: Option<Arc<RenderImage>>) -> bool {
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.image = image;
                true
            }
            None => false,
        }
    }

    /// Drop the pages used longest ago, past the budget, but none in use.
    fn make_room(&mut self) -> Vec<Arc<RenderImage>> {
        let mut total: usize = self.entries.values().filter_map(|e| e.image.as_deref()).map(bytes).sum();
        let mut dropped = vec![];
        if total <= BUDGET {
            return dropped;
        }
        let now = Instant::now();
        let mut old: Vec<(K, Instant)> = self.entries.iter().filter(|(_, e)| now - e.used > IN_USE).map(|(k, e)| (k.clone(), e.used)).collect();
        old.sort_by_key(|(_, used)| *used);
        for (key, _) in old {
            if total <= BUDGET {
                break;
            }
            if let Some(image) = self.entries.remove(&key).and_then(|e| e.image) {
                total -= bytes(&image);
                dropped.push(image);
            }
        }
        dropped
    }
}

/// Pages of `K`'s kind drawn, and the last drawing of each page of each
/// document, to show while it's drawn again.
struct Drawn<K> {
    cache: PageCache<K>,
    latest: HashMap<(usize, usize), K>,
}

impl<K> Default for Drawn<K> {
    fn default() -> Self {
        Self { cache: PageCache::default(), latest: HashMap::new() }
    }
}

impl<K: 'static> Global for Drawn<K> {}

/// A page to draw: `key` says what's on it (it's drawn again when that
/// changes), at `pixels` wide; `of` is the document and page it is, for its
/// last drawing to show meanwhile.
pub struct Page<K> {
    pub key: K,
    pub of: (usize, usize),
    pub pixels: usize,
}

/// The page drawn, if it's ready (or its last drawing, while it's drawn
/// again). If it isn't, `draw` draws it (as wide as it's given, or a
/// little wider) in the background, and the windows redraw when it's done.
pub fn drawn<K>(page: Page<K>, draw: impl FnOnce(usize) -> Option<Bitmap> + Send + 'static, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>>
where
    K: Hash + Eq + Clone + Send + 'static,
{
    let key = (page.key, width(page.pixels));
    let drawn = cx.default_global::<Drawn<(K, usize)>>();
    let meanwhile = |drawn: &mut Drawn<(K, usize)>| drawn.latest.get(&page.of).cloned().and_then(|old| drawn.cache.get(&old)).flatten();
    match drawn.cache.get(&key) {
        Some(Some(image)) => {
            drawn.latest.insert(page.of, key.clone());
            return Some(image);
        }
        Some(None) => return meanwhile(drawn),
        None => {}
    }
    let shown = meanwhile(drawn);
    let dropped = drawn.cache.drawing(key.clone());
    dropped.into_iter().for_each(|image| cx.drop_image(image, Some(&mut *window)));
    let pixels = key.1;
    let drawing = cx.background_executor().spawn(async move { draw(pixels).and_then(image) });
    cx.spawn(async move |cx| {
        let image = drawing.await;
        cx.update(|cx| {
            let drawn = cx.default_global::<Drawn<(K, usize)>>();
            // Unless it was dropped while it was being drawn.
            if drawn.cache.drawn(&key, image) {
                drawn.latest.insert(page.of, key);
                cx.refresh_windows();
            }
        })
        .ok();
    })
    .detach();
    shown
}

/// A drawn page as an image to paint: gpui takes BGRA, as pages are drawn, but
/// multiplies by alpha itself, so a page's edges would otherwise go dark and thin.
fn image(mut bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    straighten(&mut bitmap.bgra);
    let frame = image::Frame::new(image::RgbaImage::from_raw(bitmap.width as u32, bitmap.height as u32, bitmap.bgra)?);
    Some(Arc::new(RenderImage::new(vec![frame])))
}

/// Undo premultiplying by alpha.
fn straighten(bgra: &mut [u8]) {
    for pixel in bgra.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// Where pages of `sizes` are, one under another, as wide as the view (at
/// most `max_width`, centered), in the view before it's scrolled.
#[derive(Clone)]
pub struct Stack {
    pub left: Pixels,
    pub width: Pixels,
    tops: Vec<Pixels>,
    /// Pixels to a point, for each page.
    scales: Vec<f32>,
    heights: Vec<Pixels>,
    /// Below the last page.
    pub bottom: Pixels,
}

impl Stack {
    pub fn new(sizes: impl IntoIterator<Item = Size>, view_width: Pixels, max_width: Option<Pixels>) -> Self {
        let room = view_width - px(GAP * 2.);
        // However narrow the view, wide enough to see; but no wider than it should be.
        let room = room.max(px(100.));
        let width = max_width.map_or(room, |max| room.min(max)).max(px(1.));
        let left = ((view_width - width) / 2.).max(px(GAP));
        let (mut tops, mut scales, mut heights, mut y) = (vec![], vec![], vec![], px(GAP));
        for page in sizes {
            let scale = f32::from(width) / page.width.max(1.);
            let height = px(page.height * scale);
            tops.push(y);
            scales.push(scale);
            heights.push(height);
            y += height + px(GAP);
        }
        Self { left, width, tops, scales, heights, bottom: y }
    }

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    /// Page `page`'s bounds.
    pub fn page(&self, page: usize) -> Bounds<Pixels> {
        Bounds::new(point(self.left, self.tops[page]), size(self.width, self.heights[page]))
    }

    /// A box on page `page`, in points from its top left, in the view.
    pub fn rect(&self, page: usize, x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        let s = self.scales[page];
        Bounds::new(point(self.left + px(x * s), self.tops[page] + px(y * s)), size(px(width * s), px(height * s)))
    }

    /// The page `at` is on (or above, past the last one's top), and where
    /// on it, in points.
    pub fn at(&self, at: Point<Pixels>) -> Option<(usize, f32, f32)> {
        let page = self.tops.iter().rposition(|top| *top <= at.y)?;
        let s = self.scales[page];
        Some((page, f32::from(at.x - self.left) / s, f32::from(at.y - self.tops[page]) / s))
    }

    /// How many pixels wide a page is drawn, for `window`.
    pub fn pixels(&self, window: &Window) -> usize {
        (f32::from(self.width) * window.scale_factor()).round() as usize
    }

    /// The top to scroll to so `wanted` shows (if it doesn't, or `always`) a
    /// third of the way down a view `height` tall, clamped to the pages.
    pub fn reveal(&self, top: Pixels, wanted: Option<Bounds<Pixels>>, always: bool, height: Pixels) -> Pixels {
        let top = match wanted {
            Some(r) if always || r.top() < top || r.bottom() > top + height => r.top() - height / 3.,
            _ => top,
        };
        self.clamp(top, height)
    }

    /// `top` kept within the pages, for a view `height` tall.
    pub fn clamp(&self, top: Pixels, height: Pixels) -> Pixels {
        top.clamp(px(0.), (self.bottom - height).max(px(0.)))
    }
}

/// The view the pages are in, on screen, and how far it's scrolled.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub bounds: Bounds<Pixels>,
    pub scroll_top: Pixels,
}

impl Viewport {
    /// A box in the view before scrolling, on the screen.
    pub fn place(&self, r: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::new(point(self.bounds.left() + r.left(), self.bounds.top() + r.top() - self.scroll_top), r.size)
    }

    /// A point on the screen, in the view before scrolling.
    pub fn unplace(&self, at: Point<Pixels>) -> Point<Pixels> {
        point(at.x - self.bounds.left(), at.y - self.bounds.top() + self.scroll_top)
    }
}

/// The pages on screen, each with its drawing if it's ready, and what's
/// over them, ready to [`paint`].
pub struct Shown {
    pub viewport: Viewport,
    pages: Vec<(Bounds<Pixels>, Option<Arc<RenderImage>>)>,
    /// Over the pages: the selection, the cursor.
    quads: Vec<PaintQuad>,
}

impl Shown {
    /// The pages of `stack` in `viewport`, each drawn by `drawn` (by its index).
    pub fn new(stack: &Stack, viewport: Viewport, mut drawn: impl FnMut(usize) -> Option<Arc<RenderImage>>) -> Self {
        let bounds = viewport.bounds;
        let mut pages = vec![];
        for page in 0..stack.len() {
            let on_screen = viewport.place(stack.page(page));
            if on_screen.bottom() >= bounds.top() && on_screen.top() <= bounds.bottom() {
                pages.push((on_screen, drawn(page)));
            }
        }
        Self { viewport, pages, quads: vec![] }
    }

    /// Mark a box (in the view before scrolling) over the pages.
    pub fn mark(&mut self, r: Bounds<Pixels>, color: gpui::Hsla) {
        self.quads.push(fill(self.viewport.place(r), color));
    }
}

/// Pages are white whatever the theme: the cursor on them is a box that
/// reads on paper, and lets the letter through.
pub fn cursor_color(select: bool, theme: &crate::theme::Theme, focused: bool) -> gpui::Hsla {
    let color: gpui::Hsla = if select { theme.cursor_select } else { gpui::rgb(0x2563eb).into() };
    color.opacity(if focused { 0.4 } else { 0.15 })
}

/// Paint the pages, on white (until they're drawn), and what's over them.
pub fn paint(shown: &mut Shown, window: &mut Window) {
    window.with_content_mask(Some(ContentMask { bounds: shown.viewport.bounds }), |window| {
        for (at, image) in &shown.pages {
            window.paint_quad(fill(*at, gpui::white()));
            if let Some(image) = image {
                window.paint_image(*at, Corners::default(), image.clone(), 0, false).ok();
            }
        }
        for quad in shown.quads.drain(..) {
            window.paint_quad(quad);
        }
    });
}

#[cfg(test)]
mod straightening {
    use super::straighten;

    #[test]
    fn a_soft_edge_gets_its_color_back() {
        // White at half strength, then nothing, then solid.
        let mut pixels = [128, 128, 128, 128, 0, 0, 0, 0, 10, 20, 30, 255];
        straighten(&mut pixels);
        assert_eq!(pixels, [255, 255, 255, 128, 0, 0, 0, 0, 10, 20, 30, 255]);
    }
}

#[cfg(test)]
mod stacking {
    use super::*;

    #[test]
    fn a_small_page_stays_small_and_a_narrow_view_still_shows_one() {
        let small = [Size { width: 16., height: 16. }];
        assert_eq!(Stack::new(small, px(800.), Some(px(16.))).width, px(16.), "an icon isn't enlarged");
        assert_eq!(Stack::new(small, px(800.), None).width, px(776.), "with no limit, it's as wide as the view");
        assert_eq!(Stack::new(small, px(20.), None).width, px(100.), "and never too narrow to see");
    }
}
