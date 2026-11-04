//! Typst as a plugin: pages from a project's files, live as they're typed, that
//! know where their text came from; and it reaches only what it's given.
//! Needs `scripts/build-plugins.sh` to have run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ghostwrite_page::Pages;
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

/// The pages know where their text came from: each letter of `text` is on
/// them, a click on it finds it again, and every page is drawn, with ink on it.
fn pages_know_their_text(pages: &Arc<dyn Pages>, text: &str, letters: &str) {
    for (at, _) in text.match_indices(letters) {
        for at in at..at + letters.len() {
            let place = pages.cursor(at).unwrap_or_else(|| panic!("{:?} at {at} is on the pages", &text[at..at + 1]));
            assert_eq!(pages.nearest(at), Some(place));
            let clicked = pages.at(place.page, place.x + place.width / 2., place.y + place.height / 2.);
            assert_eq!(clicked, Some(at), "a click on {:?} finds it", &text[at..at + 1]);
        }
        assert!(!pages.covering(at..at + letters.len()).is_empty(), "{letters} covered");
    }
    for (page, size) in pages.sizes().iter().enumerate() {
        let drawn = pages.draw_at(page, 800).expect("it's drawn");
        assert_eq!(drawn.width, 800);
        assert_eq!(drawn.height, (800. * size.height / size.width).round() as usize, "as tall as the page is, for its width");
        assert!(drawn.bgra.chunks_exact(4).any(|pixel| pixel[3] > 0 && pixel[..3] != [255; 3]), "page {page} has ink on it");
    }
}

#[test]
fn it_makes_pages_live_as_its_typed() {
    let plugin = needs_the_plugin!();
    let (root, path) = project();
    let mut wasm = plugin.renderer(&root, &path).expect("it makes pages");
    let pages = wasm.render(TEXT).unwrap();
    assert_eq!(pages.sizes().len(), 2);
    assert_eq!(pages.sizes()[0].width, 300.);
    pages_know_their_text(&pages, TEXT, "Title");
    pages_know_their_text(&pages, TEXT, "Hello");
    assert_eq!(pages.cursor(TEXT.find("second").unwrap()).map(|place| place.page), Some(1), "after the break, the next page");

    // Typing: what follows moves along.
    let with = |text: &str, pages: &Arc<dyn Pages>| pages.cursor(text.find("with").unwrap()).unwrap();
    let typed = TEXT.replace("Hello", "Hello there");
    let retyped = wasm.render(&typed).unwrap();
    pages_know_their_text(&retyped, &typed, "there");
    assert!(with(&typed, &retyped).x > with(TEXT, &pages).x || with(&typed, &retyped).y > with(TEXT, &pages).y);

    // A file it uses changing.
    let before = retyped.key(0);
    std::fs::write(root.join("words.typ"), "#let word = [changed]").unwrap();
    assert_ne!(wasm.render(&typed).unwrap().key(0), before, "the page is made again");

    for (wrong, says) in [("#oops(", "error:"), ("#import \"nowhere.typ\": x", "nowhere.typ"), ("#image(\"missing.png\")", "missing.png")] {
        let error = wasm.render(wrong).err().unwrap_or_else(|| panic!("{wrong}: an error"));
        assert!(error.starts_with("paper/paper.typ:1: ") && error.contains(says), "{wrong}: {error}");
    }
    wasm.render(&typed).expect("and it goes on after an error");
}

/// A font of the system, not one Typst comes with.
#[test]
fn it_uses_the_systems_fonts() {
    let plugin = needs_the_plugin!();
    let family = if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(windows) {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    };
    let (root, path) = project();
    let text = format!("#set page(width: 300pt, height: auto)\n#set text(font: \"{family}\", fallback: false)\nSome words in it.");
    let mut wasm = plugin.renderer(&root, &path).unwrap();
    let mine = wasm.render(&text).expect("it makes pages");
    // The width of a letter of "Some words in it.", `nth` along.
    let width = |pages: &Arc<dyn Pages>, text: &str, nth: usize| pages.cursor(text.find("Some").unwrap() + nth).expect("the glyph is there").width;
    let i = "Some words in it.".find('i').unwrap();
    assert_eq!(width(&mine, &text, 0), width(&mine, &text, i), "monospaced: the font asked for");
    let default = "#set page(width: 300pt, height: auto)\nSome words in it.";
    let plain = wasm.render(default).unwrap();
    assert_ne!(width(&mine, &text, i), width(&plain, default, i), "not what it would fall back to");
    pages_know_their_text(&mine, &text, "words");
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

/// How fast it is on a real document: `GHOSTWRITE_TYPST_FILE`, in the project
/// `GHOSTWRITE_TYPST_ROOT`. In release: `cargo test --release -p ghostwrite-wasm --test typst -- --ignored --nocapture`.
#[test]
#[ignore]
fn on_a_real_document() {
    use std::time::Instant;
    let plugin = needs_the_plugin!();
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
    let mut renderer = plugin.renderer(&root, &path).unwrap();
    let from = Instant::now();
    let pages = renderer.render(&text).unwrap_or_else(|why| panic!("{why}"));
    println!("first: {:8.1} ms, {} pages", ms(from), pages.sizes().len());
    let from = Instant::now();
    renderer.render(&typed(0)).unwrap();
    println!("second: {:7.1} ms", ms(from));
    let typing: Vec<f64> = (1..=15)
        .map(|words| {
            let from = Instant::now();
            renderer.render(&typed(words)).unwrap();
            ms(from)
        })
        .collect();
    println!("typing: {:7.1} ms median, {:.1} ms worst", median(typing.clone()), typing.iter().copied().fold(0., f64::max));
    let pages = renderer.render(&typed(15)).unwrap();
    let drawing: Vec<f64> = (0..pages.sizes().len().min(6))
        .map(|page| {
            let from = Instant::now();
            pages.draw_at(page, 1600).expect("drawn");
            ms(from)
        })
        .collect();
    println!("a page drawn: {:6.1} ms median, {:.1} ms worst", median(drawing.clone()), drawing.iter().copied().fold(0., f64::max));
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
    println!("a tile at 4x: {:6.1} ms median", median(tiles));
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

/// Live: tinymist, the language server the plugin names, through the client as
/// the editor drives it. Needs `tinymist` installed.
#[test]
#[ignore]
fn tinymist_jumps_completes_and_rechecks_while_typing() {
    use futures::channel::{mpsc, oneshot};
    use ghostwrite_lsp::{Document, Event, Registry, convert};
    use serde_json::Value;
    use std::time::{Duration, Instant};

    fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn answer(mut rx: oneshot::Receiver<Result<Value, String>>) -> Value {
        wait(|| rx.try_recv().unwrap()).unwrap()
    }

    let plugin = needs_the_plugin!();
    let servers = plugin.languages()[0].servers;
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins/typst");
    ghostwrite_plugin::install(|| vec![Box::new(Manifest::in_own_folder(&folder).expect("its manifest is sound"))]);
    let (root, _) = project();
    let (main, other) = (root.join("main.typ"), root.join("other.typ"));
    std::fs::write(&other, "#let shout = 2\n").unwrap();
    let text = "#import \"other.typ\": shout\n#let greet = 1\n#greet #shout\n";
    std::fs::write(&main, text).unwrap();

    let (tx, mut events) = mpsc::unbounded();
    let mut registry = Registry::new(root.clone(), tx);
    let mut doc = Document::open(registry.start(servers, &main).expect("tinymist installed"), "typst", &main, text);

    // gd within the file, and into another file.
    let at = text.rfind("#greet").unwrap() + 2;
    let (path, pos) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!((path.file_name().unwrap(), pos.line), (main.file_name().unwrap(), 1));
    let at = text.rfind("shout").unwrap() + 1;
    let (path, _) = convert::first_location(&answer(doc.ask("textDocument/definition", text, at))).expect("a definition");
    assert_eq!(path.file_name().unwrap(), "other.typ");

    // Completion after `#gr`, before saving anything.
    let typed = format!("{text}#gr");
    doc.changed(&typed, None);
    let items = convert::completions(answer(doc.ask("textDocument/completion", &typed, typed.len())), &typed, typed.len());
    assert!(items.iter().any(|c| c.text == "greet"), "{:?}", items.iter().map(|c| &c.label).collect::<Vec<_>>());

    // An error typed (not saved) is reported, and goes away once fixed.
    let broken = format!("{text}#nope\n");
    doc.changed(&broken, None);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { diagnostics, .. }) if diagnostics.iter().any(|d| d.message.contains("nope")) => Some(()),
        _ => None,
    });
    doc.changed(text, None);
    wait(|| match events.try_recv() {
        Ok(Event::Diagnostics { path, diagnostics, .. }) if path.ends_with("main.typ") && diagnostics.is_empty() => Some(()),
        _ => None,
    });
}
