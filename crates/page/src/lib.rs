//! A document's pages, whoever lays them out: how big each is, a page drawn, and
//! what the editor asks of them ([`Pages`]). What's on a page and where its text
//! is differs for each; that's its own.

/// A page's size, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// The most pixels a page is drawn with (256 MB of them): a page shaped to
/// be bigger (a 5 m square, a strip miles long) isn't drawn at all.
pub const MAX_PIXELS: usize = 64 << 20;

/// A page drawn: `width` × `height` pixels, a row at a time, four bytes
/// each: blue, green, red and alpha (premultiplied).
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// A rectangle on a page, in points from its top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Pages laid out from some text, as the editor needs them, whatever they are:
/// to draw them, to show a cursor and a selection on them, and to find the text
/// under a click. Offsets are bytes of that text.
pub trait Pages: Send + Sync {
    /// How big each page is.
    fn sizes(&self) -> Vec<Size>;
    /// What says a page's drawing can be kept: it's the same if this is.
    fn key(&self, page: usize) -> u128;
    /// Draw a page about `pixels` wide; `None` if it can't be.
    fn draw(&self, page: usize, pixels: usize) -> Option<Bitmap>;
    /// Where the cursor at `at` shows.
    fn cursor(&self, at: usize) -> Option<Place>;
    /// Where to look to see `at`: where it is, or if nothing on the pages comes
    /// from there (markup), the next thing that does.
    fn nearest(&self, at: usize) -> Option<Place> {
        self.cursor(at)
    }
    /// Where the text in `range` is.
    fn covering(&self, range: std::ops::Range<usize>) -> Vec<Place>;
    /// The byte of the text at `x`, `y` on `page`.
    fn at(&self, page: usize, x: f32, y: f32) -> Option<usize>;
    /// The widest a page is shown, in points before zooming.
    fn max_width(&self) -> Option<f32> {
        None
    }
    /// The text, if these pages are what it comes from, so that it can't be edited;
    /// `None` if they're made from a file that can.
    fn text(&self) -> Option<&str> {
        None
    }
}

/// What makes [`Pages`] from a file's text, again whenever it changes.
pub trait Renderer: Send {
    /// Pages from `text`, or why there can't be any.
    fn render(&mut self, text: &str) -> Result<std::sync::Arc<dyn Pages>, String>;
}

/// The widest pages of a file that's only pages are shown, in points before zooming.
const MAX_WIDTH: f32 = 900.;

/// Pages that aren't made from any text: their tab has a line for each to move by.
pub struct OnlyPages {
    pages: std::sync::Arc<dyn Pages>,
    text: String,
    /// Where each page's line starts.
    starts: Vec<usize>,
    max_width: f32,
}

impl OnlyPages {
    /// `at_own_size`: they aren't shown bigger than they are.
    pub fn new(pages: std::sync::Arc<dyn Pages>, at_own_size: bool) -> Self {
        let widest = pages.sizes().iter().map(|size| size.width).fold(0., f32::max);
        let max_width = if at_own_size { widest } else { MAX_WIDTH };
        let (mut text, mut starts) = (String::new(), vec![]);
        for number in 1..=pages.sizes().len() {
            if number > 1 {
                text.push('\n');
            }
            starts.push(text.len());
            text.push_str(&format!("── page {number} ──"));
        }
        Self { pages, text, starts, max_width }
    }

    fn page_at(&self, at: usize) -> usize {
        self.starts.partition_point(|start| *start <= at).saturating_sub(1)
    }

    fn top_of(&self, page: usize) -> Option<Place> {
        (page < self.starts.len()).then_some(Place { page, x: 0., y: 0., width: 0., height: 0. })
    }
}

impl Pages for OnlyPages {
    fn sizes(&self) -> Vec<Size> {
        self.pages.sizes()
    }

    fn key(&self, page: usize) -> u128 {
        self.pages.key(page)
    }

    fn draw(&self, page: usize, pixels: usize) -> Option<Bitmap> {
        self.pages.draw(page, pixels)
    }

    fn cursor(&self, at: usize) -> Option<Place> {
        self.top_of(self.page_at(at))
    }

    /// There's no text on them to point at.
    fn covering(&self, _range: std::ops::Range<usize>) -> Vec<Place> {
        vec![]
    }

    fn at(&self, page: usize, _x: f32, _y: f32) -> Option<usize> {
        self.starts.get(page).copied()
    }

    fn max_width(&self) -> Option<f32> {
        Some(self.max_width)
    }

    fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Two;

    impl Pages for Two {
        fn sizes(&self) -> Vec<Size> {
            vec![Size { width: 100., height: 200. }; 2]
        }
        fn key(&self, page: usize) -> u128 {
            page as u128
        }
        fn draw(&self, _page: usize, _pixels: usize) -> Option<Bitmap> {
            None
        }
        fn cursor(&self, _at: usize) -> Option<Place> {
            None
        }
        fn covering(&self, _range: std::ops::Range<usize>) -> Vec<Place> {
            vec![]
        }
        fn at(&self, _page: usize, _x: f32, _y: f32) -> Option<usize> {
            None
        }
    }

    #[test]
    fn what_is_only_pages_has_a_line_to_move_by_for_each() {
        assert_eq!(OnlyPages::new(std::sync::Arc::new(Two), true).max_width(), Some(100.), "not shown bigger than they are");
        let pages = OnlyPages::new(std::sync::Arc::new(Two), false);
        assert_eq!(pages.max_width(), Some(MAX_WIDTH));
        let text = pages.text().unwrap().to_string();
        assert_eq!(text, "── page 1 ──\n── page 2 ──");
        let second = text.find('\n').unwrap() + 1;
        assert_eq!(pages.cursor(0).map(|place| place.page), Some(0));
        assert_eq!(pages.cursor(second - 1).map(|place| place.page), Some(0));
        assert_eq!(pages.cursor(second).map(|place| place.page), Some(1));
        assert_eq!(pages.cursor(text.len()).map(|place| place.page), Some(1), "the end of the text is on the last");
        assert_eq!((pages.at(0, 5., 5.), pages.at(1, 5., 5.), pages.at(2, 5., 5.)), (Some(0), Some(second), None), "a click goes to the page's line");
        assert!(pages.covering(0..text.len()).is_empty(), "there's no text on them");
    }
}
