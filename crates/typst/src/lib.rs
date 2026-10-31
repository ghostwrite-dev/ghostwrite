//! Typst, compiled in-process. A [`Compiler`] keeps one file's world alive
//! (its project's files, packages, fonts), so each compile after the first
//! only redoes what changed. The file's text is what's on screen, saved or
//! not; everything else it uses is read from disk, or for a package from
//! Typst's package cache (downloading it the first time).
//!
//! A compiled [`Page`] knows where each glyph is on it and which bytes of
//! the file it came from, so the editor can draw the cursor on the page and
//! go from a click on the page to the text. [`Page::render`] draws it.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use typst::diag::{FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_kit::datetime::Time;
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::{FileLoader, FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::FontStore;
use typst_kit::packages::SystemPackages;
use typst_layout::PagedDocument;

/// The standard library and the fonts: the system's and Typst's own, found once.
struct Env {
    library: LazyHash<Library>,
    fonts: FontStore,
}

fn env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut fonts = FontStore::new();
        fonts.extend(typst_kit::fonts::system());
        fonts.extend(typst_kit::fonts::embedded());
        Env { library: LazyHash::new(Library::default()), fonts }
    })
}

/// Files from the project and packages, but the file being compiled from `text`.
struct Loader {
    files: SystemFiles,
    main: FileId,
    text: Mutex<Bytes>,
}

impl FileLoader for Loader {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main { Ok(self.text.lock().clone()) } else { self.files.load(id) }
    }
}

struct ProjectWorld {
    store: FileStore<Loader>,
    main: FileId,
    time: Time,
}

impl World for ProjectWorld {
    fn library(&self) -> &LazyHash<Library> {
        &env().library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        env().fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.store.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.store.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        env().fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.time.today(offset)
    }
}

/// One Typst file, compiled again and again as it changes.
pub struct Compiler {
    world: ProjectWorld,
}

impl Compiler {
    /// For `path`, rooted at `root` (where `/` paths start) if it's in it,
    /// else at its own folder.
    pub fn new(root: &Path, path: &Path) -> Option<Self> {
        let root = if path.starts_with(root) { root } else { path.parent()? };
        let main = RootedPath::new(VirtualRoot::Project, VirtualPath::virtualize(root, path).ok()?).intern();
        let packages = SystemPackages::new(SystemDownloader::new(concat!("ghostwrite/", env!("CARGO_PKG_VERSION"))));
        let files = SystemFiles::new(FsRoot::new(root.to_path_buf()), packages);
        let loader = Loader { files, main, text: Mutex::new(Bytes::new(Vec::<u8>::new())) };
        Some(Self { world: ProjectWorld { store: FileStore::new(loader), main, time: Time::system() } })
    }

    /// Compile the file as `text`, with the other files as they are now.
    /// Fails with the first error, and where it is.
    pub fn compile(&mut self, text: &str) -> Result<Document, String> {
        *self.world.store.loader().text.lock() = Bytes::new(text.as_bytes().to_vec());
        self.world.store.reset();
        self.world.time.reset();
        let compiled = typst::compile::<PagedDocument>(&self.world).output;
        // Typst keeps what it computed, for next time: enough for that, not more.
        comemo::evict(10);
        let document = compiled.map_err(|errors| self.describe(&errors))?;
        Ok(Document { pages: document.pages().iter().map(|page| Page::new(page.clone(), &self.world)).collect() })
    }

    /// `path:line: error: message`, of the first error.
    fn describe(&self, errors: &[SourceDiagnostic]) -> String {
        let Some(error) = errors.first() else { return "typst failed".into() };
        let at = error.span.id().zip(self.world.range(error.span)).and_then(|(id, range)| {
            let line = self.world.source(id).ok()?.lines().byte_to_line(range.start)?;
            Some(format!("{}:{}: ", id.get().vpath().get_without_slash(), line + 1))
        });
        format!("{}error: {}", at.unwrap_or_default(), error.message)
    }
}

pub struct Document {
    pub pages: Vec<Page>,
}

/// A page as laid out: its size in points, and its glyphs, in reading order.
#[derive(Clone)]
pub struct Page {
    page: typst_layout::Page,
    pub width: f32,
    pub height: f32,
    /// Changes when what's on it does: pages that stay the same needn't be drawn again.
    pub hash: u128,
    pub glyphs: Vec<Glyph>,
}

/// A glyph's box, in points from the page's top left, and the bytes of the
/// compiled file it came from (none from another file, or made by Typst).
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub source: Option<Range<usize>>,
}

/// A page drawn: rows of pixels, blue, green, red and alpha.
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

impl Page {
    fn new(page: typst_layout::Page, world: &ProjectWorld) -> Self {
        let size = page.frame.size();
        let mut glyphs = vec![];
        collect(&page.frame, Transform::identity(), world, &mut glyphs);
        let hash = typst::utils::hash128(&page.frame);
        Self { width: size.x.to_pt() as f32, height: size.y.to_pt() as f32, hash, glyphs, page }
    }

    /// Draw it, `pixel_per_pt` pixels to a point.
    pub fn render(&self, pixel_per_pt: f32) -> Bitmap {
        let options = typst_render::RenderOptions { pixel_per_pt: typst::utils::Scalar::new(pixel_per_pt.into()), ..Default::default() };
        let pixmap = typst_render::render(&self.page, &options);
        let mut bgra = pixmap.data().to_vec();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Bitmap { width: pixmap.width() as usize, height: pixmap.height() as usize, bgra }
    }
}

/// The glyphs in `frame`, placed by `ts`, as they're laid out.
fn collect(frame: &Frame, ts: Transform, world: &ProjectWorld, out: &mut Vec<Glyph>) {
    for (pos, item) in frame.items() {
        let ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => collect(&group.frame, ts.pre_concat(group.transform), world, out),
            FrameItem::Text(text) => {
                let metrics = text.font.metrics();
                let (top, bottom) = (-metrics.ascender.at(text.size), -metrics.descender.at(text.size));
                let mut x = Abs::zero();
                for glyph in &text.glyphs {
                    let left = x + glyph.x_offset.at(text.size);
                    let advance = glyph.x_advance.at(text.size);
                    x += advance;
                    let [a, b] = [Point::new(left, top), Point::new(left + advance, bottom)].map(|p| p.transform(ts));
                    let (span, offset) = glyph.span;
                    let source = (span.id() == Some(world.main)).then(|| world.range(span)).flatten().map(|range| {
                        let from = (range.start + usize::from(offset)).min(range.end);
                        from..(from + glyph.range().len()).clamp(from, range.end.max(from + 1))
                    });
                    out.push(Glyph {
                        x: a.x.min(b.x).to_pt() as f32,
                        y: a.y.min(b.y).to_pt() as f32,
                        width: (a.x - b.x).abs().to_pt() as f32,
                        height: (a.y - b.y).abs().to_pt() as f32,
                        source,
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("ghostwrite-typst-{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("paper")).unwrap();
        root
    }

    #[test]
    fn compiles_the_text_on_screen_with_the_files_around_it() {
        let root = project("files");
        std::fs::write(root.join("words.typ"), "#let word = [from above]").unwrap();
        let path = root.join("paper/paper.typ");
        std::fs::write(&path, "saved").unwrap();
        let mut compiler = Compiler::new(&root, &path).unwrap();
        let text = "#set page(width: 200pt, height: auto)\n#import \"../words.typ\": word\nHello #word.";
        let document = compiler.compile(text).unwrap();
        assert_eq!(document.pages.len(), 1);
        let page = &document.pages[0];
        assert_eq!(page.width, 200.);
        let hello = text.find("Hello").unwrap();
        let first = page.glyphs.iter().find(|g| g.source.is_some()).expect("glyphs know their source");
        assert_eq!(first.source, Some(hello..hello + 1), "H is the H of Hello");
        assert!(page.glyphs.iter().any(|g| g.source.is_none()), "the imported words are from another file");

        let again = compiler.compile(&text.replace("Hello", "Hi")).unwrap();
        assert_ne!(again.pages[0].hash, page.hash, "an edit changes the page");
        let error = compiler.compile("#oops(").err().unwrap();
        assert!(error.starts_with("paper/paper.typ:1: error:"), "{error}");

        let drawn = page.render(2.);
        assert_eq!((drawn.width, drawn.bgra.len()), (400, 400 * drawn.height * 4));
        std::fs::remove_dir_all(root).ok();
    }
}
