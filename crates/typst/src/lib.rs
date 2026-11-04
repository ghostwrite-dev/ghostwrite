//! Typst, compiled in-process. A [`Compiler`] keeps one file's world alive
//! (its project's files, packages, fonts), so each compile after the first
//! only redoes what changed. The file's text is what's on screen, saved or
//! not; everything else it uses is read from disk, or for a package from
//! Typst's package cache (downloading it the first time).
//!
//! A compiled [`Page`] knows where each glyph is on it and which bytes of
//! the file it came from, so the editor can draw the cursor on the page and
//! go from a click on the page to the text ([`Document::glyph_at`],
//! [`Page::hit`]). [`Page::render`] draws it.

use ghostwrite_core::Language;
use ghostwrite_lsp::registry::Spec;
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub use ghostwrite_page::{Bitmap, Place, Size};
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
        if id == self.main { Ok(self.text.lock().unwrap().clone()) } else { self.files.load(id) }
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
    pub fn new(root: &Path, path: &Path) -> Result<Self, String> {
        let root = if path.starts_with(root) { root } else { path.parent().ok_or("no folder to compile it in")? };
        let virtual_path = VirtualPath::virtualize(root, path).map_err(|_| format!("{} isn't in {}", path.display(), root.display()))?;
        let main = RootedPath::new(VirtualRoot::Project, virtual_path).intern();
        let packages = SystemPackages::new(SystemDownloader::new(concat!("ghostwrite/", env!("CARGO_PKG_VERSION"))));
        let files = SystemFiles::new(FsRoot::new(root.to_path_buf()), packages);
        let loader = Loader { files, main, text: Mutex::new(Bytes::new(Vec::<u8>::new())) };
        Ok(Self { world: ProjectWorld { store: FileStore::new(loader), main, time: Time::system() } })
    }

    /// Compile the file as `text`, with the other files as they are now.
    /// Fails with the first error, and where it is.
    pub fn compile(&mut self, text: &str) -> Result<Document, String> {
        *self.world.store.loader().text.lock().unwrap() = Bytes::new(text.as_bytes().to_vec());
        self.world.store.reset();
        self.world.time.reset();
        let compiled = typst::compile::<PagedDocument>(&self.world).output;
        // Typst keeps what it computed, for next time: enough for that, not
        // more. Its cache is the process's, shared by every compiler.
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

impl Document {
    /// The glyph made from byte `at` of the text, and its page; or, for a
    /// byte nothing on the pages comes from (markup), the next one that is if `near`.
    pub fn glyph_at(&self, at: usize, near: bool) -> Option<(usize, &Glyph)> {
        let glyphs = || self.pages.iter().enumerate().flat_map(|(ix, page)| page.glyphs.iter().map(move |g| (ix, g)));
        let exact = glyphs().find(|(_, g)| g.source.as_ref().is_some_and(|s| s.contains(&at)));
        exact.or_else(|| {
            near.then(|| glyphs().filter(|(_, g)| g.source.as_ref().is_some_and(|s| s.start >= at)).min_by_key(|(_, g)| g.source.as_ref().map(|s| s.start)))
                .flatten()
        })
    }
}

impl Document {
    fn place(page: usize, glyph: &Glyph) -> Place {
        Place { page, x: glyph.x, y: glyph.y, width: glyph.width, height: glyph.height }
    }
}

impl ghostwrite_page::Pages for Document {
    fn sizes(&self) -> Vec<Size> {
        self.pages.iter().map(|page| page.size).collect()
    }

    fn key(&self, page: usize) -> u128 {
        self.pages.get(page).map_or(0, |page| page.hash)
    }

    fn draw(&self, page: usize, region: ghostwrite_page::Region) -> Option<Bitmap> {
        self.pages.get(page)?.render(region)
    }

    fn cursor(&self, at: usize) -> Option<Place> {
        self.glyph_at(at, false).map(|(page, glyph)| Self::place(page, glyph))
    }

    fn nearest(&self, at: usize) -> Option<Place> {
        self.glyph_at(at, true).map(|(page, glyph)| Self::place(page, glyph))
    }

    fn covering(&self, range: Range<usize>) -> Vec<Place> {
        let on = |glyph: &&Glyph| glyph.source.as_ref().is_some_and(|s| s.start < range.end && range.start < s.end);
        self.pages.iter().enumerate().flat_map(|(ix, page)| page.glyphs.iter().filter(on).map(move |glyph| Self::place(ix, glyph))).collect()
    }

    fn at(&self, page: usize, x: f32, y: f32) -> Option<usize> {
        self.pages.get(page)?.hit(x, y)
    }
}

static TYPST: Language = Language {
    name: "typst",
    comment: Some("//"),
    extensions: &["typ"],
    servers: &["tinymist"],
    // `$x$`: math.
    pairs: &[('$', '$')],
    // The crate doesn't export its query; this is its queries/typst/highlights.scm.
    load: || ghostwrite_core::syntax::highlights("typst", codebook_tree_sitter_typst::LANGUAGE, include_str!("../queries/highlights.scm")),
    ..Language::DEFAULT
};

static SERVER: Spec = Spec { name: "tinymist", command: &["tinymist"], ..Spec::DEFAULT };

/// Typst as a plugin: its language, its server, and its pages.
pub struct Plugin;

impl ghostwrite_plugin::Plugin for Plugin {
    fn keys(&self, language: Option<&Language>) -> &'static [ghostwrite_plugin::Binding] {
        const KEYS: &[ghostwrite_plugin::Binding] =
            &[ghostwrite_plugin::Binding { keys: "space v", command: "preview", what: "to the pages, and back to the file" }];
        if language.is_some_and(|l| std::ptr::eq(l, &TYPST)) { KEYS } else { &[] }
    }

    fn languages(&self) -> Vec<&'static Language> {
        vec![&TYPST]
    }

    fn servers(&self) -> Vec<&'static Spec> {
        vec![&SERVER]
    }

    fn renders(&self, language: &Language) -> bool {
        std::ptr::eq(language, &TYPST)
    }

    fn renderer(&self, root: &Path, path: &Path) -> Result<Box<dyn ghostwrite_page::Renderer>, String> {
        Ok(Box::new(Compiler::new(root, path)?))
    }
}

impl ghostwrite_page::Renderer for Compiler {
    fn render(&mut self, text: &str) -> Result<std::sync::Arc<dyn ghostwrite_page::Pages>, String> {
        Ok(std::sync::Arc::new(self.compile(text)?))
    }
}

/// A page as laid out: its size, and its glyphs, in reading order.
#[derive(Clone)]
pub struct Page {
    page: typst_layout::Page,
    pub size: Size,
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

impl Page {
    fn new(page: typst_layout::Page, world: &ProjectWorld) -> Self {
        let size = page.frame.size();
        let mut glyphs = vec![];
        collect(&page.frame, Transform::identity(), world, &mut glyphs);
        let hash = typst::utils::hash128(&page.frame);
        Self { size: Size { width: size.x.to_pt() as f32, height: size.y.to_pt() as f32 }, hash, glyphs, page }
    }

    /// The byte of the text at `x`, `y` (points from the top left): the
    /// glyph there, or the nearest (lines count more than columns); after
    /// it if it's on its right half.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let distance = |g: &Glyph| {
            let dx = (g.x - x).max(x - (g.x + g.width)).max(0.);
            let dy = (g.y - y).max(y - (g.y + g.height)).max(0.);
            dy * 4. + dx
        };
        let glyph = self.glyphs.iter().filter(|g| g.source.is_some()).min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        let source = glyph.source.clone()?;
        Some(if x > glyph.x + glyph.width / 2. { source.end } else { source.start })
    }

    /// Draw `region` of it: a page of the region's size with this one on it, moved
    /// so the region is what's on it. `None` if it's too big.
    pub fn render(&self, region: ghostwrite_page::Region) -> Option<Bitmap> {
        if region.too_big() {
            return None;
        }
        let (width, height, scale) = (region.width, region.height, f64::from(region.scale));
        let mut frame = Frame::hard(typst::layout::Size::new(Abs::pt(width as f64 / scale), Abs::pt(height as f64 / scale)));
        frame.push_frame(Point::new(Abs::pt(-(region.x as f64) / scale), Abs::pt(-(region.y as f64) / scale)), self.page.frame.clone());
        let mut part = self.page.clone();
        part.frame = frame;
        let options = typst_render::RenderOptions { pixel_per_pt: typst::utils::Scalar::new(scale), ..Default::default() };
        let pixmap = typst_render::render(&part, &options);
        let mut bgra = vec![0; width * height * 4];
        let (drawn_width, drawn) = (pixmap.width() as usize, pixmap.data());
        for row in 0..height.min(pixmap.height() as usize) {
            let take = width.min(drawn_width) * 4;
            bgra[row * width * 4..][..take].copy_from_slice(&drawn[row * drawn_width * 4..][..take]);
        }
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let (right, bottom) = ((self.size.width * region.scale).round() as usize, (self.size.height * region.scale).round() as usize);
        for (row, line) in bgra.chunks_exact_mut(width * 4).enumerate() {
            for (col, pixel) in line.chunks_exact_mut(4).enumerate() {
                if region.y + row >= bottom || region.x + col >= right {
                    pixel.fill(0);
                }
            }
        }
        Some(Bitmap { width, height, bgra })
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
        assert_eq!(page.size.width, 200.);
        let hello = text.find("Hello").unwrap();
        let first = page.glyphs.iter().find(|g| g.source.is_some()).expect("glyphs know their source");
        assert_eq!(first.source, Some(hello..hello + 1), "H is the H of Hello");
        assert!(page.glyphs.iter().any(|g| g.source.is_none()), "the imported words are from another file");

        let again = compiler.compile(&text.replace("Hello", "Hi")).unwrap();
        assert_ne!(again.pages[0].hash, page.hash, "an edit changes the page");
        let error = compiler.compile("#oops(").err().unwrap();
        assert!(error.starts_with("paper/paper.typ:1: error:"), "{error}");

        // A click: on the left half of the H, before it; on its right, after it.
        let h = first.clone();
        assert_eq!(page.hit(h.x + 0.5, h.y + 1.), Some(hello), "its left half: before it");
        assert_eq!(page.hit(h.x + h.width - 0.5, h.y + 1.), Some(hello + 1), "its right half: after it");
        assert_eq!(page.hit(h.x, h.y - 15.), Some(hello), "above the line: the nearest glyph");
        assert_eq!(document.glyph_at(hello, false).map(|(_, g)| g.source.clone()), Some(Some(hello..hello + 1)));
        assert_eq!(document.glyph_at(0, true).map(|(_, g)| g.source.clone()), Some(Some(hello..hello + 1)), "from markup, the text after it");

        let drawn = page.render(ghostwrite_page::Region::whole(page.size, 400)).unwrap();
        assert!(page.render(ghostwrite_page::Region::whole(page.size, 1_000_000)).is_none(), "too big to draw");
        assert_eq!((drawn.width, drawn.bgra.len()), (400, 400 * drawn.height * 4));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn its_language_is_highlighted_through_aliases() {
        use ghostwrite_core::syntax::scope_named;
        let text = "= Title\n#let x = 1\n*bold*";
        let spans = super::TYPST.highlight(text);
        let at = |offset: usize| spans.iter().find(|s| s.range.contains(&offset)).map(|s| s.scope);
        assert_eq!(at(2), scope_named("text.title"));
        assert_eq!(at(text.find("let").unwrap()), scope_named("keyword"));
        assert_eq!(at(text.find("1").unwrap()), scope_named("number"));
        assert_eq!(at(text.find("bold").unwrap()), scope_named("text.strong"));
    }
}
