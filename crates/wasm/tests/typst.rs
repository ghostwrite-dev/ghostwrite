//! Typst as a plugin, against its native crate: the same pages from the same files,
//! and it reaches only what it's given. Needs `scripts/build-plugins.sh` to have run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ghostwrite_page::{Pages, Renderer};
use ghostwrite_plugin::Plugin as _;
use ghostwrite_plugin::manifest::Manifest;

fn plugin() -> Option<&'static Manifest> {
    static PLUGIN: OnceLock<Option<Manifest>> = OnceLock::new();
    PLUGIN
        .get_or_init(|| {
            ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
            let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins/typst");
            if !folder.join("typst.wasm").is_file() {
                return None;
            }
            Some(Manifest::in_own_folder(&folder).expect("its manifest is sound"))
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

/// A project, and the file to make pages of in it.
fn project() -> (PathBuf, PathBuf) {
    let root = ghostwrite_project::scratch_dir("typst-plugin").canonicalize().unwrap();
    std::fs::create_dir(root.join("paper")).unwrap();
    std::fs::write(root.join("words.typ"), "#let word = [from above]").unwrap();
    let path = root.join("paper/paper.typ");
    std::fs::write(&path, "saved").unwrap();
    (root, path)
}

const TEXT: &str = "#set page(width: 300pt, height: auto)\n#set text(font: \"New Computer Modern\")\n#import \"../words.typ\": word\n= Title\nHello #word, with $sum_i x_i^2$ and a table:\n#table(columns: 2, [a], [b])\n#pagebreak()\nThe second page.";

fn both(root: &Path, path: &Path) -> (Box<dyn Renderer>, Box<dyn Renderer>) {
    let wasm = plugin().unwrap().renderer(root, path).expect("it makes pages");
    (wasm, Box::new(ghostwrite_typst::Compiler::new(root, path).unwrap()))
}

fn same_pages(wasm: &Arc<dyn Pages>, native: &Arc<dyn Pages>, text: &str) {
    assert_eq!(wasm.sizes(), native.sizes(), "the same pages");
    for at in 0..text.len() {
        assert_eq!(wasm.cursor(at), native.cursor(at), "the cursor at {at}");
        assert_eq!(wasm.nearest(at), native.nearest(at), "looking near {at}");
    }
    assert_eq!(wasm.covering(0..text.len()), native.covering(0..text.len()));
    for (page, size) in wasm.sizes().iter().enumerate() {
        for y in [1., size.height / 2., size.height - 1.] {
            for x in [1., size.width / 2., size.width - 1.] {
                assert_eq!(wasm.at(page, x, y), native.at(page, x, y), "a click at {x}, {y} on page {page}");
            }
        }
        let (mine, theirs) = (wasm.draw_at(page, 800).expect("it's drawn"), native.draw_at(page, 800).expect("it's drawn"));
        assert_eq!((mine.width, mine.height), (theirs.width, theirs.height));
        assert!(mine.bgra == theirs.bgra, "page {page} is drawn the same, pixel for pixel");
    }
}

#[test]
fn it_makes_the_same_pages_as_the_native_crate() {
    needs_the_plugin!();
    let (root, path) = project();
    let (mut wasm, mut native) = both(&root, &path);
    let (mine, theirs) = (wasm.render(TEXT).unwrap(), native.render(TEXT).unwrap());
    assert_eq!(mine.sizes().len(), 2);
    same_pages(&mine, &theirs, TEXT);

    // Typing, then a file it uses changing.
    let typed = TEXT.replace("Hello", "Hello there");
    let (mine, theirs) = (wasm.render(&typed).unwrap(), native.render(&typed).unwrap());
    same_pages(&mine, &theirs, &typed);
    std::fs::write(root.join("words.typ"), "#let word = [changed]").unwrap();
    let (mine, theirs) = (wasm.render(&typed).unwrap(), native.render(&typed).unwrap());
    same_pages(&mine, &theirs, &typed);

    for wrong in ["#oops(", "#import \"nowhere.typ\": x", "#image(\"missing.png\")"] {
        assert_eq!(wasm.render(wrong).err(), native.render(wrong).err(), "{wrong}: the same error");
    }
    assert!(wasm.render("#oops(").err().unwrap_or_default().starts_with("paper/paper.typ:1: error:"));
    wasm.render(&typed).expect("and it goes on after an error");
}

/// A font of the system, not one Typst comes with, on the same terms as the native crate.
#[test]
fn it_uses_the_systems_fonts() {
    needs_the_plugin!();
    let family = if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(windows) {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    };
    let (root, path) = project();
    let text = format!("#set page(width: 300pt, height: auto)\n#set text(font: \"{family}\", fallback: false)\nSome words in it.");
    let (mut wasm, mut native) = both(&root, &path);
    let (mine, theirs) = (wasm.render(&text), native.render(&text));
    let (mine, theirs) = (mine.expect("it makes pages"), theirs.expect("it makes pages"));
    let width = |pages: &Arc<dyn Pages>| pages.cursor(text.find("Some").unwrap()).expect("the glyph is there").width;
    assert_eq!(width(&mine), width(&theirs), "the same font, by its glyph's width");
    let default = "#set page(width: 300pt, height: auto)\nSome words in it.";
    let plain = wasm.render(default).unwrap();
    let plain_width = plain.cursor(default.find("Some").unwrap()).expect("the glyph is there").width;
    assert_ne!(width(&mine), plain_width, "not what it would fall back to");
    same_pages(&mine, &theirs, &text);
    assert!(ghostwrite_project::platform::cache_dir().unwrap().join("plugin-data/typst/fonts.json").is_file(), "what it found is kept");
}

#[test]
fn pages_on_screen_can_still_be_drawn_after_the_next_ones_are_made() {
    needs_the_plugin!();
    let (root, path) = project();
    let mut wasm = plugin().unwrap().renderer(&root, &path).unwrap();
    let old = wasm.render("#set page(width: 100pt, height: 100pt)\nOld").unwrap();
    wasm.render("#set page(width: 100pt, height: 100pt)\nNew").unwrap();
    assert!(old.draw_at(0, 200).is_some(), "it's on screen until the new ones replace it");
    wasm.render("#set page(width: 100pt, height: 100pt)\nNewer").unwrap();
    assert!(old.draw_at(0, 200).is_none(), "older than that it's let go, and asking is harmless");
}

#[test]
fn it_reads_the_project_and_nothing_outside_it() {
    needs_the_plugin!();
    let (root, path) = project();
    let outside = root.parent().unwrap().join(format!("{}-secret.txt", root.file_name().unwrap().to_string_lossy()));
    std::fs::write(&outside, "private").unwrap();
    let mut wasm = plugin().unwrap().renderer(&root, &path).unwrap();
    wasm.render("#read(\"../words.typ\")").expect("the project is there");
    let name = outside.file_name().unwrap().to_string_lossy().into_owned();
    assert!(wasm.render(&format!("#read(\"../../{name}\")")).is_err(), "out of the project, by the path");
    assert!(wasm.render(&format!("#read(\"{}\")", outside.display())).is_err(), "by a whole path");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
        assert!(wasm.render("#read(\"../link.txt\")").is_err(), "by way of a link");
    }
    assert!(wasm.render("#let saved = \"a\"\n#read(\"/words.typ\")").is_ok(), "and it still works");
    std::fs::remove_file(outside).ok();
}

#[test]
fn it_writes_nothing_to_the_project() {
    needs_the_plugin!();
    let (root, path) = project();
    let before = std::fs::read_dir(&root).unwrap().count();
    plugin().unwrap().renderer(&root, &path).unwrap().render(TEXT).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved", "the file on disk isn't touched");
}

#[test]
fn it_highlights_a_file() {
    use ghostwrite_core::syntax::scope_named;
    let plugin = needs_the_plugin!();
    let language = plugin.languages()[0];
    let text = "= Title\n#let x = 1\n*bold* // note\n";
    let spans = language.highlight(text);
    let at = |offset: usize| spans.iter().find(|s| s.range.contains(&offset)).map(|s| s.scope);
    assert_eq!(at(2), scope_named("text.title"));
    assert_eq!(at(text.find("let").unwrap()), scope_named("keyword"));
    assert_eq!(at(text.find('1').unwrap()), scope_named("number"));
    assert_eq!(at(text.find("bold").unwrap()), scope_named("text.strong"));
    assert_eq!(at(text.find("note").unwrap()), scope_named("comment"));
    assert!(spans.windows(2).all(|pair| pair[0].range.end <= pair[1].range.start), "in order, without overlaps");
    assert!(language.highlight("#let é = \"ü\"").iter().all(|span| span.range.start < span.range.end), "and on character boundaries");
}

#[test]
fn it_says_all_it_can_reach() {
    let plugin = needs_the_plugin!();
    let said = plugin.describe();
    for part in
        ["reads the project", "reads the system's fonts", "typst/packages", "downloads from packages.typst.org", "starts `tinymist` with your permissions"]
    {
        assert!(said.contains(part), "{said}");
    }
}

/// How it compares on a real document: `GHOSTWRITE_TYPST_FILE`, in the project
/// `GHOSTWRITE_TYPST_ROOT`. In release: `cargo test --release -p ghostwrite-wasm --test typst -- --ignored --nocapture`.
#[test]
#[ignore]
fn on_a_real_document() {
    use std::time::Instant;
    needs_the_plugin!();
    let (file, root) = (std::env::var("GHOSTWRITE_TYPST_FILE").expect("a file"), std::env::var("GHOSTWRITE_TYPST_ROOT").expect("its project"));
    let (path, root) = (PathBuf::from(file), PathBuf::from(root));
    let text = std::fs::read_to_string(&path).unwrap();
    // Somewhere in the middle, between paragraphs: where typing would be.
    let middle = text[text.len() / 2..].find("\n\n").map_or(text.len(), |at| text.len() / 2 + at + 1);
    let typed = |words: usize| format!("{}{}\n{}", &text[..middle], (0..words).map(|n| format!("word{n} ")).collect::<String>(), &text[middle..]);
    let median = |mut times: Vec<f64>| {
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let ms = |from: Instant| from.elapsed().as_secs_f64() * 1000.;
    let (mut wasm, mut native) = both(&root, &path);
    for (name, renderer) in [("native", &mut native), ("wasm", &mut wasm)] {
        let from = Instant::now();
        let pages = renderer.render(&text).unwrap_or_else(|why| panic!("{name}: {why}"));
        println!("{name:>7} first: {:8.1} ms, {} pages", ms(from), pages.sizes().len());
        let from = Instant::now();
        renderer.render(&typed(0)).unwrap();
        println!("{name:>7} second: {:7.1} ms", ms(from));
        let typing: Vec<f64> = (1..=15)
            .map(|words| {
                let from = Instant::now();
                renderer.render(&typed(words)).unwrap();
                ms(from)
            })
            .collect();
        println!("{name:>7} typing: {:7.1} ms median, {:.1} ms worst", median(typing.clone()), typing.iter().copied().fold(0., f64::max));
        let pages = renderer.render(&typed(15)).unwrap();
        let drawing: Vec<f64> = (0..pages.sizes().len().min(6))
            .map(|page| {
                let from = Instant::now();
                pages.draw_at(page, 1600).expect("drawn");
                ms(from)
            })
            .collect();
        println!("{name:>7} a page drawn: {:6.1} ms median, {:.1} ms worst", median(drawing.clone()), drawing.iter().copied().fold(0., f64::max));
        // Zoomed in four times: a tile, as the editor draws one.
        let size = pages.sizes()[0];
        let region = ghostwrite_page::Region { x: 1024, y: 1024, width: 1024, height: 1024, ..ghostwrite_page::Region::whole(size, 6400) };
        let tiles: Vec<f64> = (0..5)
            .map(|_| {
                let from = Instant::now();
                pages.draw(0, region).expect("drawn");
                ms(from)
            })
            .collect();
        println!("{name:>7} a tile at 4x: {:6.1} ms median", median(tiles));
    }
    let final_text = typed(15);
    let (mine, theirs) = (wasm.render(&final_text).unwrap(), native.render(&final_text).unwrap());
    assert_eq!(mine.sizes(), theirs.sizes(), "the same pages");
    let mut different = vec![];
    for page in 0..mine.sizes().len() {
        let (a, b) = (mine.draw_at(page, 1600).expect("drawn"), theirs.draw_at(page, 1600).expect("drawn"));
        assert_eq!((a.width, a.height), (b.width, b.height));
        let deltas: Vec<u8> = a.bgra.iter().zip(&b.bgra).map(|(x, y)| x.abs_diff(*y)).collect();
        let off = deltas.chunks_exact(4).filter(|pixel| pixel.iter().any(|d| *d > 0)).count();
        let (most, large) = (deltas.iter().copied().max().unwrap_or(0), deltas.chunks_exact(4).filter(|pixel| pixel.iter().any(|d| *d > 8)).count());
        if off > 0 {
            println!("page {}: {off} of {} pixels differ, by at most {most}/255; {large} by more than 8", page + 1, a.width * a.height);
            let dump = std::env::var("GHOSTWRITE_TYPST_DUMP").ok();
            if let Some(dump) = dump.filter(|_| large > 0) {
                let write = |name: &str, bgra: &[u8]| {
                    let mut out = format!("P6 {} {} 255\n", a.width, a.height).into_bytes();
                    for pixel in bgra.chunks_exact(4) {
                        let alpha = 255 - pixel[3];
                        out.extend([pixel[2].saturating_add(alpha), pixel[1].saturating_add(alpha), pixel[0].saturating_add(alpha)]);
                    }
                    std::fs::write(format!("{dump}/{name}-{}.ppm", page + 1), out).unwrap();
                };
                write("wasm", &a.bgra);
                write("native", &b.bgra);
                let spread: Vec<u8> = deltas
                    .chunks_exact(4)
                    .flat_map(|pixel| {
                        let d = 255u8.saturating_sub(pixel.iter().copied().max().unwrap_or(0).saturating_mul(16));
                        [d, d, d, 255]
                    })
                    .collect();
                write("difference", &spread);
            }
            if most > 16 {
                different.push(page + 1);
            }
        }
    }
    let places = (0..final_text.len()).step_by(37).filter(|at| mine.cursor(*at) != theirs.cursor(*at)).count();
    let clicks = (0..mine.sizes().len()).filter(|page| mine.at(*page, 200., 300.) != theirs.at(*page, 200., 300.)).count();
    println!("pages drawn visibly differently: {different:?}; cursors placed differently: {places}; clicks landing differently: {clicks}");
    assert!(different.is_empty() && places == 0 && clicks == 0);
}

/// Live: a package it hasn't got is downloaded by way of the editor, to Typst's own folder.
#[test]
#[ignore]
fn it_downloads_a_package() {
    needs_the_plugin!();
    let (root, path) = project();
    let mut wasm = plugin().unwrap().renderer(&root, &path).unwrap();
    let pages = wasm.render("#import \"@preview/example:0.1.0\": add\n#add(1, 2)").expect("it's fetched and used");
    assert_eq!(pages.sizes().len(), 1);
    let folder = ghostwrite_project::platform::cache_dir().unwrap().parent().unwrap().join("typst/packages/preview/example");
    assert!(folder.join("0.1.0/typst.toml").is_file(), "it's where Typst's own tools look");
    assert!(!std::fs::read_dir(&folder).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().starts_with('.')), "with nothing left lying about");
    assert!(wasm.render("#import \"@preview/no-such-package-at-all:9.9.9\": x").is_err());
}

#[test]
fn a_page_drawn_in_pieces_is_the_page_drawn_whole() {
    let plugin = needs_the_plugin!();
    let (root, path) = project();
    let mut renderer = plugin.renderer(&root, &path).unwrap();
    let pages =
        renderer.render("#set page(width: 200pt, height: 120pt, fill: rgb(\"#fffbe6\"))\n= Pieces\nThey meet where they're cut: $sum_(i=1)^n i$.").unwrap();
    for pixels in [200, 431, 1600] {
        ghostwrite_page::check_regions(pages.as_ref(), 0, pixels).unwrap();
    }
}
