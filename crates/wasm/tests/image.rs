//! Viewing images as a plugin. Needs `scripts/build-plugins.sh` to have run.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ghostwrite_page::Renderer;
use ghostwrite_plugin::Plugin as _;
use ghostwrite_plugin::manifest::Manifest;
use image::{ImageEncoder, ImageFormat, Rgba, RgbaImage};

fn plugin() -> Option<&'static Manifest> {
    static PLUGIN: OnceLock<Option<Manifest>> = OnceLock::new();
    PLUGIN
        .get_or_init(|| {
            ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
            let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins/image");
            folder.join("image.wasm").is_file().then(|| Manifest::in_own_folder(&folder).expect("its manifest is sound"))
        })
        .as_ref()
}

macro_rules! needs_the_plugin {
    () => {
        match plugin() {
            Some(plugin) => plugin,
            None => return eprintln!("scripts/build-plugins.sh hasn't run: skipped"),
        }
    };
}

/// 40 by 20: red on its left half, blue on its right.
fn picture() -> RgbaImage {
    RgbaImage::from_fn(40, 20, |x, _| if x < 20 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) })
}

fn encoded(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(vec![]);
    if format == ImageFormat::Jpeg {
        image::DynamicImage::ImageRgba8(image.clone()).into_rgb8().write_to(&mut bytes, format).unwrap();
    } else {
        image.write_to(&mut bytes, format).unwrap();
    }
    bytes.into_inner()
}

fn in_a_project(name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
    let root = ghostwrite_project::scratch_dir("image-plugin").canonicalize().unwrap();
    let path = root.join(name);
    std::fs::write(&path, bytes).unwrap();
    (root, path)
}

/// Blue, green, red and alpha at `x`, `y`.
fn pixel(drawn: &ghostwrite_page::Bitmap, x: usize, y: usize) -> [u8; 4] {
    drawn.bgra[(y * drawn.width + x) * 4..][..4].try_into().unwrap()
}

#[test]
fn it_says_what_it_is_and_all_it_can_reach() {
    let plugin = needs_the_plugin!();
    let language = plugin.languages()[0];
    assert!(language.only_pages && language.pages_at_own_size);
    let said = plugin.describe();
    assert!(said.contains("reads the project") && !said.contains("downloads") && !said.contains("writes"), "{said}");
}

#[test]
fn it_reads_each_format_and_draws_it() {
    let plugin = needs_the_plugin!();
    for (name, format) in [
        ("a.png", ImageFormat::Png),
        ("a.jpg", ImageFormat::Jpeg),
        ("a.gif", ImageFormat::Gif),
        ("a.bmp", ImageFormat::Bmp),
        ("a.tiff", ImageFormat::Tiff),
        ("a.webp", ImageFormat::WebP),
        ("a.ico", ImageFormat::Ico),
    ] {
        let (root, path) = in_a_project(name, &encoded(&picture(), format));
        let pages = plugin.renderer(&root, &path).unwrap().render("").unwrap_or_else(|why| panic!("{name}: {why}"));
        assert_eq!(pages.sizes().iter().map(|size| (size.width, size.height)).collect::<Vec<_>>(), [(40., 20.)], "{name}");
        let drawn = pages.draw(0, 40).expect("it's drawn");
        assert_eq!((drawn.width, drawn.height), (40, 20), "{name}");
        let (left, right) = (pixel(&drawn, 5, 10), pixel(&drawn, 35, 10));
        assert!(left[2] > 200 && left[0] < 60 && right[0] > 200 && right[2] < 60, "{name}: {left:?} {right:?}");
    }
}

#[test]
fn it_goes_by_whats_in_the_file_not_what_its_called() {
    let plugin = needs_the_plugin!();
    let (root, path) = in_a_project("really-a-png.jpg", &encoded(&picture(), ImageFormat::Png));
    assert!(plugin.renderer(&root, &path).unwrap().render("").is_ok());
}

#[test]
fn it_shrinks_but_never_enlarges_and_keeps_transparency_as_the_editor_takes_it() {
    let plugin = needs_the_plugin!();
    let (root, path) = in_a_project("a.png", &encoded(&picture(), ImageFormat::Png));
    let pages = plugin.renderer(&root, &path).unwrap().render("").unwrap();
    let smaller = pages.draw(0, 20).unwrap();
    assert_eq!((smaller.width, smaller.height), (20, 10));
    let larger = pages.draw(0, 4000).unwrap();
    assert_eq!((larger.width, larger.height), (40, 20), "what's on screen scales it, if it has to");

    let see_through = RgbaImage::from_pixel(4, 4, Rgba([200, 100, 50, 128]));
    let (root, path) = in_a_project("b.png", &encoded(&see_through, ImageFormat::Png));
    let drawn = plugin.renderer(&root, &path).unwrap().render("").unwrap().draw(0, 4).unwrap();
    assert_eq!(pixel(&drawn, 1, 1), [25, 50, 100, 128], "premultiplied, as pages are");
}

#[test]
fn a_photo_is_the_way_up_it_says_it_is() {
    let plugin = needs_the_plugin!();
    let mut bytes = vec![];
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    // Exif: big-endian, one entry, orientation 6 (turn it a quarter clockwise).
    let exif = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0".to_vec();
    encoder.set_exif_metadata(exif).unwrap();
    let image = picture();
    encoder.write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgba8).unwrap();
    let (root, path) = in_a_project("photo.png", &bytes);
    let pages = plugin.renderer(&root, &path).unwrap().render("").unwrap();
    assert_eq!((pages.sizes()[0].width, pages.sizes()[0].height), (20., 40.), "taller than it's wide, once it's turned");
    let drawn = pages.draw(0, 20).unwrap();
    assert!(pixel(&drawn, 10, 5)[2] > 200, "red, its left, is now on top");
    assert!(pixel(&drawn, 10, 35)[0] > 200, "blue on the bottom");
}

#[test]
fn it_draws_again_when_the_file_changes_and_says_whats_wrong() {
    let plugin = needs_the_plugin!();
    let (root, path) = in_a_project("a.png", &encoded(&picture(), ImageFormat::Png));
    let mut renderer: Box<dyn Renderer> = plugin.renderer(&root, &path).unwrap();
    let first = renderer.render("").unwrap();
    assert_eq!(renderer.render("").unwrap().key(0), first.key(0), "the same file: no need to draw it again");
    std::fs::write(&path, encoded(&RgbaImage::from_pixel(10, 10, Rgba([0, 255, 0, 255])), ImageFormat::Png)).unwrap();
    let changed = renderer.render("").unwrap();
    assert_ne!(changed.key(0), first.key(0));
    assert_eq!(changed.sizes()[0].width, 10.);
    assert!(first.draw(0, 40).is_some(), "what's on screen still draws until it's replaced");
    std::fs::write(&path, "not an image").unwrap();
    assert!(renderer.render("").err().unwrap_or_default().contains("as an image"));
    std::fs::remove_file(&path).unwrap();
    assert!(renderer.render("").err().unwrap_or_default().contains("couldn't read"));
    std::fs::write(&path, encoded(&picture(), ImageFormat::Png)).unwrap();
    assert!(renderer.render("").is_ok(), "and it goes on when the file's back");
}

/// How long the real thing takes: `GHOSTWRITE_IMAGE_FILE`.
/// `cargo test --release -p ghostwrite-wasm --test image -- --ignored --nocapture`.
#[test]
#[ignore]
fn on_a_real_image() {
    use std::time::Instant;
    let plugin = needs_the_plugin!();
    let path = PathBuf::from(std::env::var("GHOSTWRITE_IMAGE_FILE").expect("a file"));
    let ms = |from: Instant| from.elapsed().as_secs_f64() * 1000.;
    let mut renderer = plugin.renderer(path.parent().unwrap(), &path).unwrap();
    let from = Instant::now();
    let pages = renderer.render("").unwrap();
    let size = pages.sizes()[0];
    println!("opened: {:.0} ms, {} by {}, {} KB", ms(from), size.width, size.height, std::fs::metadata(&path).unwrap().len() / 1024);
    let from = Instant::now();
    pages.draw(0, 1600).expect("drawn");
    println!("drawn 1600 wide: {:.0} ms", ms(from));
}
