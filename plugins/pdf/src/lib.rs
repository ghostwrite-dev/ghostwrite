//! PDFs as a plugin: their pages, drawn by a renderer in pure Rust. It doesn't say
//! where the text is, so a PDF is pages to look at, with nothing to select or ask about.
//! What's malformed in one can do no more than stop this plugin.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ghostwrite_plugin_api::{Drawing, Page, Plugin, Region, register};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

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

    /// The page on white, moved and scaled so the region is what lands on the pixels.
    fn draw(key_high: u64, key_low: u64, region: Region) -> Option<Drawing> {
        OPEN.with(|open| {
            let open = open.borrow();
            let read = open.read.iter().find(|read| read.hash == key_high)?;
            let pages = read.pdf.pages();
            let page = pages.get(usize::try_from(key_low).ok()?)?;
            let (width, height) = (u16::try_from(region.width).ok()?, u16::try_from(region.height).ok()?);
            if (region.width as usize).saturating_mul(region.height as usize) > MAX_PIXELS || width == 0 || height == 0 || !(region.scale > 0.) {
                return None;
            }
            let scale = f64::from(region.scale);
            let transform = Affine::translate((-f64::from(region.x), -f64::from(region.y))) * Affine::scale(scale) * page.initial_transform(true).to_kurbo();
            let mut ctx = RenderContext::new(width, height);
            render_into(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &mut ctx, transform);
            ctx.flush();
            let mut pixmap = Pixmap::new(width, height);
            let settings = RasterizerSettings { target_init: TargetInit::Clear(WHITE), ..Default::default() };
            ctx.render_with(&mut pixmap, &mut Resources::default(), settings);
            let mut bgra = pixmap.data_as_u8_slice().to_vec();
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let (page_width, page_height) = page.render_dimensions();
            off_the_page(&mut bgra, &region, page_width * region.scale, page_height * region.scale);
            Some(Drawing { width: region.width, height: region.height, bgra })
        })
    }
}

/// Clear what of `region` is past the page's right or bottom edge, at `width` by `height` pixels.
fn off_the_page(bgra: &mut [u8], region: &Region, width: f32, height: f32) {
    let (right, bottom) = (width.round().max(0.) as usize, height.round().max(0.) as usize);
    for (row, line) in bgra.chunks_exact_mut(region.width as usize * 4).enumerate() {
        let y = region.y as usize + row;
        for (col, pixel) in line.chunks_exact_mut(4).enumerate() {
            if y >= bottom || region.x as usize + col >= right {
                pixel.fill(0);
            }
        }
    }
}

register!(Pdfs);
