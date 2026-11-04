//! Viewing images, as a plugin: a file is one page, the size of the image, the way up
//! it says it's meant to be. An animation shows its first frame. What's malformed in
//! a file, or shaped to take all the memory there is, can do no more than stop this plugin.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;

use ghostwrite_plugin_api::{Drawing, Page, Plugin, Region, register};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbaImage};

/// The most pixels an image is drawn with, and the most memory reading one can take.
const MAX_PIXELS: usize = 64 << 20;
const MAX_MEMORY: u64 = 2 << 30;

/// The file as it was read: what's on screen is drawn from it until the next is shown.
struct Read {
    /// What says it's the same file: the high half of its page's key.
    hash: u64,
    image: RgbaImage,
}

#[derive(Default)]
struct Open {
    path: String,
    /// The last read and the one before it.
    read: Vec<Read>,
}

thread_local! {
    static OPEN: RefCell<Open> = RefCell::default();
    /// The image last made smaller, by its file's hash and its width: the regions of one drawing are cut from it.
    static SHRUNK: RefCell<Option<(u64, u32, RgbaImage)>> = const { RefCell::new(None) };
}

fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_MEMORY);
    let mut reader = reader;
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

struct Images;

impl Plugin for Images {
    fn open_pages(path: String, _utc_offset_minutes: i32) -> Result<(), String> {
        OPEN.with(|open| *open.borrow_mut() = Open { path, read: vec![] });
        Ok(())
    }

    /// Its one page, read again: there's no text to make it from.
    fn make_pages(_text: String) -> Result<Vec<Page>, String> {
        OPEN.with(|open| {
            let mut open = open.borrow_mut();
            let bytes = std::fs::read(format!("/project/{}", open.path)).map_err(|e| format!("couldn't read {}: {e}", open.path))?;
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            let hash = hasher.finish();
            if open.read.last().is_none_or(|read| read.hash != hash) {
                let image = decode(&bytes).map_err(|e| format!("couldn't read {} as an image: {e}", open.path))?;
                if open.read.len() == 2 {
                    open.read.remove(0);
                }
                open.read.push(Read { hash, image });
            }
            let image = &open.read.last().expect("just read").image;
            Ok(vec![Page { width: image.width() as f32, height: image.height() as f32, key_high: hash, key_low: 0, glyphs: vec![] }])
        })
    }

    /// Made smaller (the whole image, once, so its regions meet exactly), or larger,
    /// smoothly, pixel by pixel.
    fn draw(key_high: u64, _key_low: u64, region: Region) -> Option<Drawing> {
        if (region.width as usize).saturating_mul(region.height as usize) > MAX_PIXELS || region.width == 0 || region.height == 0 || !(region.scale > 0.) {
            return None;
        }
        OPEN.with(|open| {
            let open = open.borrow();
            let image = &open.read.iter().find(|read| read.hash == key_high)?.image;
            let mut bgra = vec![0; region.width as usize * region.height as usize * 4];
            if region.scale < 1. {
                let width = ((image.width() as f32 * region.scale).round() as u32).max(1);
                let height = ((image.height() as f32 * region.scale).round() as u32).max(1);
                SHRUNK.with(|shrunk| {
                    let mut shrunk = shrunk.borrow_mut();
                    if shrunk.as_ref().is_none_or(|(hash, made, _)| (*hash, *made) != (key_high, width)) {
                        *shrunk = Some((key_high, width, image::imageops::resize(image, width, height, FilterType::Triangle)));
                    }
                    let (_, _, small) = shrunk.as_ref().expect("just made");
                    for (row, line) in bgra.chunks_exact_mut(region.width as usize * 4).enumerate() {
                        let y = region.y + row as u32;
                        for (col, pixel) in line.chunks_exact_mut(4).enumerate() {
                            let x = region.x + col as u32;
                            if x < small.width() && y < small.height() {
                                pixel.copy_from_slice(&premultiplied(small.get_pixel(x, y).0));
                            }
                        }
                    }
                });
            } else {
                enlarged(image, &region, &mut bgra);
            }
            Some(Drawing { width: region.width, height: region.height, bgra })
        })
    }
}

/// A pixel as it's drawn: blue, green, red, alpha, multiplied by alpha.
fn premultiplied([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let times = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
    [times(b), times(g), times(r), a]
}

/// `region` of `image` drawn `region.scale` times its size, at least as big as it
/// is: each pixel blended from the four nearest it, nothing past its edges.
fn enlarged(image: &RgbaImage, region: &Region, bgra: &mut [u8]) {
    let (w, h) = (image.width() as f32, image.height() as f32);
    let at = |x: i64, y: i64| premultiplied(image.get_pixel(x.clamp(0, w as i64 - 1) as u32, y.clamp(0, h as i64 - 1) as u32).0).map(f32::from);
    for (row, line) in bgra.chunks_exact_mut(region.width as usize * 4).enumerate() {
        let y = (region.y as f32 + row as f32 + 0.5) / region.scale;
        for (col, pixel) in line.chunks_exact_mut(4).enumerate() {
            let x = (region.x as f32 + col as f32 + 0.5) / region.scale;
            if x >= w || y >= h {
                continue;
            }
            let (fx, fy) = ((x - 0.5).floor(), (y - 0.5).floor());
            let (tx, ty) = (x - 0.5 - fx, y - 0.5 - fy);
            let (x0, y0) = (fx as i64, fy as i64);
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            for i in 0..4 {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                pixel[i] = (top + (bottom - top) * ty).round().clamp(0., 255.) as u8;
            }
        }
    }
}

register!(Images);
