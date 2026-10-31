//! PDFs, through macOS's PDFKit. [`Pdf::open`] reads a file's text, page by
//! page, with where each of its characters is on the page, so the editor can
//! treat a PDF as read-only text and draw what it does to that text (the
//! cursor, a selection, what Claude says about some lines) on the pages.
//! [`render`] draws a page.
//!
//! Positions are in points from the top left of the page's crop box.
//! Rotated pages aren't turned upright.

use std::path::Path;

use anyhow::{Context, Result, bail};
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSRange, NSString, NSURL};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument, PDFPage};

/// A page's size, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// One line of [`Pdf::text`], where it is on its page.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub page: usize,
    pub top: f32,
    pub bottom: f32,
    /// Where each character starts, and then where the last one ends.
    pub xs: Vec<f32>,
}

/// A PDF's text and where it is.
#[derive(Debug, Clone)]
pub struct Pdf {
    pub pages: Vec<Size>,
    /// Each page's text, after a line saying which page it is ([`page_line`]).
    pub text: String,
    /// One for each line of `text`.
    pub lines: Vec<Line>,
}

/// How the line starting each page in [`Pdf::text`] begins.
pub const PAGE_MARK: &str = "── page";

/// The line that starts page `page` (from 0) in [`Pdf::text`].
pub fn page_line(page: usize) -> String {
    format!("{PAGE_MARK} {} ──", page + 1)
}

impl Pdf {
    /// Read `path`'s text and where it is. Blocks for a while on a long document.
    pub fn open(path: &Path) -> Result<Pdf> {
        autoreleasepool(|_| {
            let doc = document(path)?;
            let mut pdf = Pdf { pages: vec![], text: String::new(), lines: vec![] };
            for ix in 0..unsafe { doc.pageCount() } {
                let page = unsafe { doc.pageAtIndex(ix) }.context("a page is missing")?;
                pdf.add_page(&page);
            }
            Ok(pdf)
        })
    }

    fn add_page(&mut self, page: &PDFPage) {
        let crop = unsafe { page.boundsForBox(PDFDisplayBox::CropBox) };
        let number = self.pages.len();
        self.pages.push(Size { width: crop.size.width as f32, height: crop.size.height as f32 });
        let top_of = |r: CGRect| (crop.origin.y + crop.size.height - r.origin.y - r.size.height) as f32;
        let mark = page_line(number);
        self.push_line(&mark, Line { page: number, top: 0., bottom: 0., xs: vec![0.; mark.chars().count() + 1] });

        let text = unsafe { page.string() }.map_or(String::new(), |s| s.to_string());
        // Where a character is comes from selecting it: selections count in
        // UTF-16 units, as the text does (`characterBoundsAtIndex` doesn't,
        // and drifts after a math letter or a line break).
        let bounds = |unit: usize| unsafe { page.selectionForRange(NSRange::new(unit, 1)).map(|s| s.boundsForPage(page)) };
        let mut unit = 0;
        let mut chars = text.chars().peekable();
        let (mut line, mut xs, mut top, mut bottom, mut right) = (String::new(), vec![], f32::MAX, f32::MIN, 0.);
        let (mut last_bottom, mut last_rect) = (0., None);
        loop {
            let c = chars.next();
            if matches!(c, None | Some('\n' | '\r')) {
                if c == Some('\r') && chars.peek() == Some(&'\n') {
                    chars.next();
                    unit += 1;
                }
                xs.push(right);
                if top > bottom {
                    (top, bottom) = (last_bottom, last_bottom);
                }
                last_bottom = bottom;
                self.push_line(&line, Line { page: number, top, bottom, xs: std::mem::take(&mut xs) });
                (top, bottom, right, last_rect) = (f32::MAX, f32::MIN, 0., None);
                line.clear();
                match c {
                    None => break,
                    Some(_) => {
                        unit += 1;
                        continue;
                    }
                }
            }
            let c = c.unwrap_or_default();
            let r = bounds(unit).filter(|r| r.size.width > 0. && r.size.height > 0.);
            let left = match r {
                // The second letter of a ligature: it has the first's box, and the right half of it.
                Some(r) if Some(r) == last_rect => (xs.last().copied().unwrap_or(right) + right) / 2.,
                Some(r) => (r.origin.x - crop.origin.x) as f32,
                None => right,
            };
            if let Some(r) = r {
                top = top.min(top_of(r));
                bottom = bottom.max(top_of(r) + r.size.height as f32);
                right = right.max((r.origin.x + r.size.width - crop.origin.x) as f32);
            }
            last_rect = r;
            xs.push(left);
            line.push(c);
            unit += c.len_utf16();
        }
    }

    fn push_line(&mut self, text: &str, line: Line) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(text);
        self.lines.push(line);
    }

    /// The line and character (both from 0) at `x`, `y` on `page`: the line
    /// it's on or nearest to, and the character under it or nearest.
    pub fn hit(&self, page: usize, x: f32, y: f32) -> Option<(usize, usize)> {
        let distance = |l: &Line| {
            if y < l.top {
                l.top - y
            } else if y > l.bottom {
                y - l.bottom
            } else {
                0.
            }
        };
        let (ix, line) =
            self.lines.iter().enumerate().filter(|(_, l)| l.page == page && l.bottom > l.top).min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))?;
        let chars = line.xs.len() - 1;
        let col = line.xs.windows(2).position(|w| x < (w[0] + w[1]) / 2.).unwrap_or(chars);
        Some((ix, col.min(chars.saturating_sub(1))))
    }
}

fn document(path: &Path) -> Result<Retained<PDFDocument>> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let doc = unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }.context("not a PDF")?;
    if unsafe { doc.isLocked() } {
        bail!("it's locked with a password");
    }
    Ok(doc)
}

/// A document open on a thread: its file, as of when it last changed.
type Open = (std::path::PathBuf, Option<std::time::SystemTime>, Retained<PDFDocument>);

/// `path`'s document, kept open on this thread while it's the one drawn
/// (pages are drawn one after another): opened again if the file changed.
fn opened(path: &Path) -> Result<Retained<PDFDocument>> {
    thread_local! {
        static OPEN: std::cell::RefCell<Option<Open>> = const { std::cell::RefCell::new(None) };
    }
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some(doc) = OPEN.with_borrow(|open| open.as_ref().filter(|(p, m, _)| p == path && *m == modified).map(|(_, _, doc)| doc.clone())) {
        return Ok(doc);
    }
    let doc = document(path)?;
    OPEN.set(Some((path.to_path_buf(), modified, doc.clone())));
    Ok(doc)
}

/// A drawn page: `width` × `height` pixels, four bytes each, blue green red alpha.
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Draw page `page` (from 0) of `path`, `width` pixels wide, on white.
pub fn render(path: &Path, page: usize, width: usize) -> Result<Bitmap> {
    autoreleasepool(|_| {
        let doc = opened(path)?;
        let page = unsafe { doc.pageAtIndex(page) }.context("no such page")?;
        let crop = unsafe { page.boundsForBox(PDFDisplayBox::CropBox) };
        let width = width.max(1);
        let scale = width as f64 / crop.size.width.max(1.);
        let height = ((crop.size.height * scale).round() as usize).max(1);
        let mut bgra = vec![0u8; width * height * 4];
        let space = CGColorSpace::new_device_rgb();
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
        // The context draws into `bgra`, and is gone before it's returned.
        let context = unsafe { CGBitmapContextCreate(bgra.as_mut_ptr().cast(), width, height, 8, width * 4, space.as_deref(), info) }
            .context("couldn't make a bitmap")?;
        CGContext::set_rgb_fill_color(Some(&context), 1., 1., 1., 1.);
        CGContext::fill_rect(Some(&context), CGRect::new(CGPoint::new(0., 0.), CGSize::new(width as f64, height as f64)));
        CGContext::scale_ctm(Some(&context), scale, scale);
        unsafe { page.drawWithBox_toContext(PDFDisplayBox::CropBox, &context) };
        drop(context);
        Ok(Bitmap { width, height, bgra })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with two lines of text, made by macOS's own `cupsfilter`-free path: Typst, if it's installed.
    fn sample() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("ghostwrite-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let (source, pdf) = (dir.join("a.typ"), dir.join("a.pdf"));
        std::fs::write(&source, "#set page(width: 300pt, height: 200pt, margin: 20pt)\nHello world\n\nSecond line").ok()?;
        let status = std::process::Command::new("typst").arg("compile").arg(&source).arg(&pdf).status().ok()?;
        status.success().then_some(pdf)
    }

    #[test]
    fn a_line_is_where_its_characters_are() {
        let Some(path) = sample() else { return eprintln!("skipped: no typst to make a PDF with") };
        let pdf = Pdf::open(&path).unwrap();
        assert_eq!(pdf.pages, vec![Size { width: 300., height: 200. }]);
        let lines: Vec<&str> = pdf.text.lines().collect();
        assert_eq!(lines[0], "── page 1 ──");
        assert!(lines.contains(&"Hello world"), "{lines:?}");
        assert_eq!(pdf.lines.len(), lines.len());
        let hello = lines.iter().position(|l| *l == "Hello world").unwrap();
        let line = &pdf.lines[hello];
        assert!(line.top >= 15. && line.top < 30. && line.bottom > line.top, "from the top, inside the margin: {line:?}");
        assert!((line.xs[0] - 20.).abs() < 1., "at the margin: {line:?}");
        assert!(line.xs.windows(2).all(|w| w[0] <= w[1]), "left to right: {line:?}");
        assert_eq!(pdf.hit(0, line.xs[1] + 0.5, line.top + 2.), Some((hello, 1)), "a click on the e");
        let second = lines.iter().position(|l| *l == "Second line").unwrap();
        assert!(pdf.lines[second].top > line.bottom, "the next paragraph is lower");
    }

    #[test]
    fn a_page_draws_at_the_width_asked() {
        let Some(path) = sample() else { return eprintln!("skipped: no typst to make a PDF with") };
        let page = render(&path, 0, 600).unwrap();
        assert_eq!((page.width, page.height), (600, 400));
        assert!(page.bgra.chunks(4).any(|p| p[0] < 128), "there's dark text");
        assert!(page.bgra.chunks(4).filter(|p| p[0] > 250).count() > page.width * page.height / 2, "on white");
        assert!(Pdf::open(&path.with_extension("typ")).is_err(), "the source isn't a PDF");
    }
}
