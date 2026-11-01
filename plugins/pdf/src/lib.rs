//! PDFs as a plugin: their pages, drawn by a renderer in pure Rust. It doesn't say
//! where the text is, so a PDF is pages to look at, with nothing to select or ask about.
//! What's malformed in one can do no more than stop this plugin.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ghostwrite_plugin_api::{Drawing, Page, Plugin, register};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};

/// The most pixels a page is drawn with: one shaped to be bigger isn't drawn at all.
const MAX_PIXELS: usize = 64 << 20;

/// The file as it was read: what's on screen is drawn from it until the next is shown.
struct Read {
    /// What says it's the same file: the high half of its pages' keys.
    hash: u64,
    pdf: Pdf,
}

#[derive(Default)]
struct Open {
    path: String,
    /// The last read and the one before it.
    read: Vec<Read>,
}

thread_local! {
    static OPEN: RefCell<Open> = RefCell::default();
}

struct Pdfs;

impl Plugin for Pdfs {
    fn open_pages(path: String, _utc_offset_minutes: i32) -> Result<(), String> {
        OPEN.with(|open| *open.borrow_mut() = Open { path, read: vec![] });
        Ok(())
    }

    /// Its pages, read again: there's no text to make them from.
    fn make_pages(_text: String) -> Result<Vec<Page>, String> {
        OPEN.with(|open| {
            let mut open = open.borrow_mut();
            let bytes = std::fs::read(format!("/project/{}", open.path)).map_err(|e| format!("couldn't read {}: {e}", open.path))?;
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            let hash = hasher.finish();
            if open.read.last().is_none_or(|read| read.hash != hash) {
                let pdf = Pdf::new(Arc::new(bytes)).map_err(|e| format!("couldn't read {} as a PDF: {e:?}", open.path))?;
                if open.read.len() == 2 {
                    open.read.remove(0);
                }
                open.read.push(Read { hash, pdf });
            }
            let read = open.read.last().expect("just read");
            let pages: Vec<Page> = read
                .pdf
                .pages()
                .iter()
                .enumerate()
                .map(|(number, page)| {
                    let (width, height) = page.render_dimensions();
                    Page { width, height, key_high: hash, key_low: number as u64, glyphs: vec![] }
                })
                .collect();
            if pages.is_empty() {
                return Err(format!("{} has no pages", open.path));
            }
            Ok(pages)
        })
    }

    fn draw_page(key_high: u64, key_low: u64, pixels: u32) -> Option<Drawing> {
        OPEN.with(|open| {
            let open = open.borrow();
            let read = open.read.iter().find(|read| read.hash == key_high)?;
            let pages = read.pdf.pages();
            let page = pages.get(usize::try_from(key_low).ok()?)?;
            let (page_width, page_height) = page.render_dimensions();
            let width = (pixels as usize).max(1);
            let scale = width as f32 / page_width.max(1.);
            let height = ((page_height * scale).round() as usize).max(1);
            if width.saturating_mul(height) > MAX_PIXELS || width > u16::MAX as usize || height > u16::MAX as usize {
                return None;
            }
            let settings = RenderSettings { x_scale: scale, y_scale: scale, width: Some(width as u16), height: Some(height as u16), bg_color: WHITE };
            let pixmap = hayro::render(page, &RenderCache::new(), &InterpreterSettings::default(), &settings);
            let mut bgra = pixmap.data_as_u8_slice().to_vec();
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Some(Drawing { width: width as u32, height: height as u32, bgra })
        })
    }
}

register!(Pdfs);
