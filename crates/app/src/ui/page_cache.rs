//! Drawn pages (a PDF's, a Typst file's), kept as images so scrolling
//! doesn't draw them again. They're big (a page is ~10–20 MB on a Retina
//! screen), so there's a budget: past it, the pages used longest ago go,
//! never one used in the last couple of seconds (it's on screen). Widths are
//! rounded up, so dragging a pane's edge doesn't draw every page again for
//! every pixel: the nearest drawing is shown, scaled, meanwhile.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::RenderImage;

/// Bytes of drawn pages kept at most.
const BUDGET: usize = 256 << 20;
/// Pages used this recently stay, over budget or not.
const IN_USE: Duration = Duration::from_secs(2);
/// Widths are drawn at multiples of this many pixels.
const STEP: usize = 128;

/// A width to draw at: `pixels`, rounded up.
pub fn width(pixels: usize) -> usize {
    pixels.div_ceil(STEP).max(1) * STEP
}

struct Entry {
    /// `None` while it's being drawn, or if it couldn't be.
    image: Option<Arc<RenderImage>>,
    used: Instant,
}

/// The cache, by `K` (a page and its width).
pub struct PageCache<K> {
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
    pub fn get(&mut self, key: &K) -> Option<Option<Arc<RenderImage>>> {
        let entry = self.entries.get_mut(key)?;
        entry.used = Instant::now();
        Some(entry.image.clone())
    }

    /// Any drawing of a key `like` this one (the same page at another width), to show meanwhile.
    pub fn any(&mut self, like: impl Fn(&K) -> bool) -> Option<Arc<RenderImage>> {
        let entry = self.entries.iter_mut().filter(|(k, e)| like(k) && e.image.is_some()).max_by_key(|(_, e)| e.used)?.1;
        entry.used = Instant::now();
        entry.image.clone()
    }

    /// `key` is being drawn: it's kept a place for, and the budget made
    /// room for. Returns the drawings dropped, for the caller to let go of
    /// on the GPU (`cx.drop_image`).
    pub fn drawing(&mut self, key: K) -> Vec<Arc<RenderImage>> {
        self.entries.insert(key, Entry { image: None, used: Instant::now() });
        self.make_room()
    }

    /// `key`'s drawing is done; `false` if it was dropped meanwhile.
    pub fn drawn(&mut self, key: &K, image: Option<Arc<RenderImage>>) -> bool {
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
