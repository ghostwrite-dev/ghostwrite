//! Viewing images, as a plugin: a file is one page, the size of the image, the way up
//! it says it's meant to be. An animation shows its first frame. What's malformed in
//! a file, or shaped to take all the memory there is, can do no more than stop this plugin.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;

use ghostwrite_plugin_api::{Drawing, Page, Plugin, register};
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

    /// Never bigger than it is: what's on screen scales it up, if it has to.
    fn draw_page(key_high: u64, _key_low: u64, pixels: u32) -> Option<Drawing> {
        OPEN.with(|open| {
            let open = open.borrow();
            let image = &open.read.iter().find(|read| read.hash == key_high)?.image;
            let width = pixels.clamp(1, image.width());
            let height = ((u64::from(image.height()) * u64::from(width)).div_ceil(u64::from(image.width()))).max(1) as u32;
            if (width as usize).saturating_mul(height as usize) > MAX_PIXELS {
                return None;
            }
            let resized;
            let drawn = if width == image.width() {
                image
            } else {
                resized = image::imageops::resize(image, width, height, FilterType::Triangle);
                &resized
            };
            let mut bgra = Vec::with_capacity(drawn.as_raw().len());
            for pixel in drawn.pixels() {
                let [r, g, b, a] = pixel.0;
                let times = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                bgra.extend([times(b), times(g), times(r), a]);
            }
            Some(Drawing { width, height, bgra })
        })
    }
}

register!(Images);
