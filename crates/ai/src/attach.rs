//! Files a model is shown as they are, not as text: images and PDFs, for models that
//! can see them. What it can't take as it is (too big, or a format it doesn't know)
//! is made into something it can.

use std::io::Cursor;
use std::path::Path;

use base64::Engine as _;
use image::{DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat, ImageReader, Limits};
use serde_json::{Value, json};

/// What an image can be once it's in base64: 5 MB is what every platform takes.
const MAX_IMAGE: usize = 5 * 1024 * 1024 / 4 * 3;
/// How long an image's sides can be at all, and what's worth keeping of one that's
/// shrunk: more than the sharpest a model sees it.
const MAX_SIDE: u32 = 8000;
const SHRUNK_SIDE: u32 = 2576;
/// A PDF: what fits in a request of 32 MB, once it's in base64.
const MAX_PDF: usize = 32 * 1024 * 1024 / 4 * 3 - 1024 * 1024;
/// The most of a file that's looked at, and the most memory reading an image can take.
const MAX_FILE: u64 = 256 << 20;
const MAX_MEMORY: u64 = 2 << 30;

/// A file as a model is shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub media_type: &'static str,
    pub data: Vec<u8>,
}

impl Attachment {
    /// As it goes in a message.
    pub fn block(&self) -> Value {
        let data = base64::engine::general_purpose::STANDARD.encode(&self.data);
        let kind = if self.media_type == "application/pdf" { "document" } else { "image" };
        json!({ "type": kind, "source": { "type": "base64", "media_type": self.media_type, "data": data } })
    }
}

/// What the file's first bytes say it is, if it's one a model can be shown:
/// what it's called doesn't come into it.
pub fn kind_of(path: &Path) -> Option<&'static str> {
    use std::io::Read as _;
    let mut start = [0; 64];
    let read = std::fs::File::open(path).ok()?.read(&mut start).ok()?;
    let start = &start[..read];
    if start.starts_with(b"%PDF-") {
        return Some("a PDF");
    }
    let known = [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::WebP, ImageFormat::Bmp, ImageFormat::Tiff, ImageFormat::Ico];
    image::guess_format(start).ok().filter(|format| known.contains(format)).map(|_| "an image")
}

/// The file at `path` to show a model, or why it can't be shown. It has to be what
/// [`kind_of`] knows.
pub fn attach(path: &Path) -> Result<Attachment, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err(format!("it's too big to show, at {} MB", size >> 20));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(b"%PDF-") {
        if bytes.len() > MAX_PDF {
            return Err(format!("it's too big to show, at {} MB: the most is {} MB", bytes.len() >> 20, MAX_PDF >> 20));
        }
        return Ok(Attachment { media_type: "application/pdf", data: bytes });
    }
    image_of(bytes)
}

fn reader(bytes: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_MEMORY);
    reader.limits(limits);
    Ok(reader)
}

fn image_of(bytes: Vec<u8>) -> Result<Attachment, String> {
    let format = reader(&bytes)?.format().ok_or("it isn't an image")?;
    let media_type = match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    };
    let (width, height) = reader(&bytes)?.into_dimensions().map_err(|e| e.to_string())?;
    if let Some(media_type) = media_type.filter(|_| bytes.len() <= MAX_IMAGE && width.max(height) <= MAX_SIDE) {
        return Ok(Attachment { media_type, data: bytes });
    }
    let mut decoder = reader(&bytes)?.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    let mut side = image.width().max(image.height()).min(SHRUNK_SIDE);
    // Smaller until it fits: a photo's size depends on what's in it.
    for _ in 0..6 {
        let shrunk = if image.width().max(image.height()) > side { image.thumbnail(side, side) } else { image.clone() };
        let encoded = encoded(&shrunk)?;
        if encoded.data.len() <= MAX_IMAGE {
            return Ok(encoded);
        }
        side = side * 3 / 4;
    }
    Err("it's too big to show, even made smaller".into())
}

/// As a PNG if it has to keep what's see-through or is small enough to, else as a JPEG.
fn encoded(image: &DynamicImage) -> Result<Attachment, String> {
    let mut png = Cursor::new(vec![]);
    image.write_to(&mut png, ImageFormat::Png).map_err(|e| e.to_string())?;
    let png = png.into_inner();
    if png.len() <= MAX_IMAGE || image.color().has_alpha() && image.pixels().any(|(_, _, pixel)| pixel.0[3] != 255) {
        return Ok(Attachment { media_type: "image/png", data: png });
    }
    let mut jpeg = Cursor::new(vec![]);
    DynamicImage::ImageRgb8(image.to_rgb8()).write_to(&mut jpeg, ImageFormat::Jpeg).map_err(|e| e.to_string())?;
    Ok(Attachment { media_type: "image/jpeg", data: jpeg.into_inner() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn written(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = ghostwrite_project::scratch_dir("attach").join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn encoded_as(image: &RgbImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(vec![]);
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    /// An image that doesn't compress, however it's stored.
    fn noise(width: u32, height: u32) -> RgbImage {
        let mut seed = 1u32;
        RgbImage::from_fn(width, height, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let [r, g, b, _] = seed.to_be_bytes();
            Rgb([r, g, b])
        })
    }

    fn sides(attachment: &Attachment) -> (u32, u32) {
        image::load_from_memory(&attachment.data).unwrap().dimensions()
    }

    #[test]
    fn what_it_takes_goes_as_it_is() {
        let small = RgbImage::from_pixel(4, 4, Rgb([255, 0, 0]));
        for (name, format, media_type) in [
            ("a.png", ImageFormat::Png, "image/png"),
            ("a.jpg", ImageFormat::Jpeg, "image/jpeg"),
            ("a.gif", ImageFormat::Gif, "image/gif"),
            ("a.webp", ImageFormat::WebP, "image/webp"),
        ] {
            let bytes = encoded_as(&small, format);
            let path = written(name, &bytes);
            assert_eq!(kind_of(&path), Some("an image"));
            assert_eq!(attach(&path), Ok(Attachment { media_type, data: bytes }), "{name}");
        }
        let pdf = b"%PDF-1.4\nwhatever follows".to_vec();
        let path = written("a.pdf", &pdf);
        assert_eq!(kind_of(&path), Some("a PDF"));
        assert_eq!(attach(&path), Ok(Attachment { media_type: "application/pdf", data: pdf }));
    }

    #[test]
    fn a_format_it_doesnt_know_is_made_into_one_it_does() {
        let image = RgbImage::from_pixel(4, 4, Rgb([255, 0, 0]));
        let mut icon = Cursor::new(vec![]);
        DynamicImage::ImageRgb8(image.clone()).to_rgba8().write_to(&mut icon, ImageFormat::Ico).unwrap();
        let files = [("a.bmp", encoded_as(&image, ImageFormat::Bmp)), ("a.tiff", encoded_as(&image, ImageFormat::Tiff)), ("a.ico", icon.into_inner())];
        for (name, bytes) in files {
            let shown = attach(&written(name, &bytes)).unwrap();
            assert_eq!(shown.media_type, "image/png", "{name}");
            assert_eq!(sides(&shown), (4, 4), "{name}");
        }
    }

    #[test]
    fn it_goes_by_whats_in_the_file_and_text_isnt_shown() {
        assert_eq!(kind_of(&written("really.txt", &encoded_as(&RgbImage::new(1, 1), ImageFormat::Png))), Some("an image"));
        assert_eq!(kind_of(&written("fake.png", b"just some words in a file, nothing more")), None);
        assert_eq!(kind_of(&written("empty.png", b"")), None);
        assert_eq!(kind_of(Path::new("/no/such/file.png")), None);
    }

    #[test]
    fn whats_too_big_is_made_smaller_until_it_fits() {
        let huge = encoded_as(&noise(3000, 2000), ImageFormat::Png);
        assert!(huge.len() > MAX_IMAGE, "{}", huge.len());
        let shown = attach(&written("huge.png", &huge)).unwrap();
        assert!(shown.data.len() <= MAX_IMAGE, "{}", shown.data.len());
        let (width, height) = sides(&shown);
        assert!(width <= SHRUNK_SIDE && width * 2 / 3 == height, "the same shape: {width} by {height}");
        assert_eq!(shown.media_type, "image/jpeg");

        let wide = encoded_as(&RgbImage::new(9000, 10), ImageFormat::Png);
        assert!(wide.len() <= MAX_IMAGE);
        assert!(sides(&attach(&written("wide.png", &wide)).unwrap()).0 <= MAX_SIDE, "too long a side, however few bytes");
    }

    #[test]
    fn a_block_is_as_the_model_takes_it() {
        let image = Attachment { media_type: "image/png", data: b"hi".to_vec() }.block();
        assert_eq!(image, json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" } }));
        assert_eq!(Attachment { media_type: "application/pdf", data: vec![] }.block()["type"], "document");
    }

    #[test]
    fn what_isnt_what_it_says_says_so() {
        assert!(attach(&written("broken.png", b"\x89PNG\r\n\x1a\nthen nothing sensible")).is_err());
    }
}
