//! PDFs as a plugin. Needs `scripts/build-plugins.sh` to have run.

use std::path::Path;
use std::sync::OnceLock;

use ghostwrite_plugin::Plugin as _;
use ghostwrite_plugin::manifest::Manifest;

fn plugin() -> Option<&'static Manifest> {
    static PLUGIN: OnceLock<Option<Manifest>> = OnceLock::new();
    PLUGIN
        .get_or_init(|| {
            ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
            let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins/pdf");
            folder.join("pdf.wasm").is_file().then(|| Manifest::in_own_folder(&folder).expect("its manifest is sound"))
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

/// A PDF with one page of `width` by 100 points, with its bottom left filled in.
fn pdf(width: u32) -> Vec<u8> {
    let content = "0 0 0 rg 0 0 100 50 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} 100] /Contents 4 0 R >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut bytes = String::from("%PDF-1.4\n");
    let mut offsets = vec![];
    for (ix, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.push_str(&format!("{} 0 obj\n{object}\nendobj\n", ix + 1));
    }
    let xref = bytes.len();
    bytes.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for offset in offsets {
        bytes.push_str(&format!("{offset:010} 00000 n \n"));
    }
    bytes.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
    bytes.into_bytes()
}

#[test]
fn it_says_what_it_is_and_all_it_can_reach() {
    let plugin = needs_the_plugin!();
    assert!(plugin.languages()[0].only_pages);
    assert_eq!(Some(plugin.languages()[0]).map(|language| language.extensions), Some(&["pdf", "PDF"][..]));
    let said = plugin.describe();
    assert!(said.contains("reads the project") && !said.contains("downloads") && !said.contains("writes"), "{said}");
}

#[test]
fn it_draws_a_files_pages_and_again_when_the_file_changes() {
    let plugin = needs_the_plugin!();
    let root = ghostwrite_project::scratch_dir("pdf-plugin").canonicalize().unwrap();
    let path = root.join("paper.pdf");
    std::fs::write(&path, pdf(200)).unwrap();
    let mut renderer = plugin.renderer(&root, &path).unwrap();
    let first = renderer.render("").unwrap();
    assert_eq!(first.sizes().iter().map(|size| (size.width, size.height)).collect::<Vec<_>>(), [(200., 100.)]);
    let drawn = first.draw_at(0, 400).expect("it's drawn");
    assert_eq!((drawn.width, drawn.height), (400, 200), "at the width asked");
    let pixel = |x: usize, y: usize| drawn.bgra[(y * drawn.width + x) * 4..][..4].to_vec();
    assert_eq!(pixel(50, 150), [0, 0, 0, 255], "what's on it, the right way up");
    assert_eq!(pixel(350, 50), [255, 255, 255, 255], "on a white page");
    assert!(first.draw_at(1, 400).is_none(), "there's no second page");

    let same = renderer.render("").unwrap();
    assert_eq!(same.key(0), first.key(0), "the same file: no need to draw it again");
    std::fs::write(&path, pdf(300)).unwrap();
    let changed = renderer.render("").unwrap();
    assert_ne!(changed.key(0), first.key(0));
    assert_eq!(changed.sizes()[0].width, 300.);
    assert!(first.draw_at(0, 400).is_some(), "what's on screen still draws until it's replaced");

    std::fs::write(&path, "not a pdf").unwrap();
    assert!(renderer.render("").err().unwrap_or_default().contains("as a PDF"));
    std::fs::remove_file(&path).unwrap();
    assert!(renderer.render("").err().unwrap_or_default().contains("couldn't read"));
    std::fs::write(&path, pdf(200)).unwrap();
    assert!(renderer.render("").is_ok(), "and it goes on when the file's back");
    assert!(std::fs::read_dir(&root).unwrap().count() == 1, "it writes nothing beside it");
}

#[test]
fn a_file_outside_the_project_opens_and_it_reads_only_that_folder() {
    let plugin = needs_the_plugin!();
    let (project, elsewhere) =
        (ghostwrite_project::scratch_dir("pdf-project").canonicalize().unwrap(), ghostwrite_project::scratch_dir("pdf-elsewhere").canonicalize().unwrap());
    let path = elsewhere.join("paper.pdf");
    std::fs::write(&path, pdf(200)).unwrap();
    assert!(plugin.renderer(&project, &path).unwrap().render("").is_ok());
}

/// How long the real thing takes: `GHOSTWRITE_PDF_FILE`.
/// `cargo test --release -p ghostwrite-wasm --test pdf -- --ignored --nocapture`.
#[test]
#[ignore]
fn on_a_real_document() {
    use std::time::Instant;
    let plugin = needs_the_plugin!();
    let path = std::path::PathBuf::from(std::env::var("GHOSTWRITE_PDF_FILE").expect("a file"));
    let root = path.parent().unwrap();
    let ms = |from: Instant| from.elapsed().as_secs_f64() * 1000.;
    let mut renderer = plugin.renderer(root, &path).unwrap();
    let from = Instant::now();
    let pages = renderer.render("").unwrap();
    println!("opened: {:.0} ms, {} pages, {} KB", ms(from), pages.sizes().len(), std::fs::metadata(&path).unwrap().len() / 1024);
    let from = Instant::now();
    renderer.render("").unwrap();
    println!("read again, unchanged: {:.1} ms", ms(from));
    let mut times: Vec<f64> = (0..pages.sizes().len())
        .map(|page| {
            let from = Instant::now();
            let drawn = pages.draw_at(page, 1600).expect("drawn");
            assert!(drawn.bgra.chunks_exact(4).any(|pixel| pixel[0] < 128), "page {page} has something on it");
            ms(from)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    println!("a page drawn: {:.0} ms median, {:.0} ms worst", times[times.len() / 2], times[times.len() - 1]);
    let region = ghostwrite_page::Region { x: 1024, y: 1024, width: 1024, height: 1024, ..ghostwrite_page::Region::whole(pages.sizes()[0], 6400) };
    let mut tiles: Vec<f64> = (0..pages.sizes().len().min(10))
        .map(|page| {
            let from = Instant::now();
            pages.draw(page, region).expect("drawn");
            ms(from)
        })
        .collect();
    tiles.sort_by(f64::total_cmp);
    println!("a tile at 4x: {:.0} ms median, {:.0} ms worst", tiles[tiles.len() / 2], tiles[tiles.len() - 1]);
}

#[test]
fn a_page_drawn_in_pieces_is_the_page_drawn_whole() {
    let plugin = needs_the_plugin!();
    let root = ghostwrite_project::scratch_dir("pdf-pieces").canonicalize().unwrap();
    let path = root.join("paper.pdf");
    std::fs::write(&path, pdf(200)).unwrap();
    let pages = plugin.renderer(&root, &path).unwrap().render("").unwrap();
    for pixels in [200, 431, 1000] {
        ghostwrite_page::check_regions(pages.as_ref(), 0, pixels).unwrap();
    }
}
