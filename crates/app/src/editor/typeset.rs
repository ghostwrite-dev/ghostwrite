//! Claude's math shows typeset: what it says about a Typst file is Typst,
//! and elsewhere its `$…$` is Typst math (see [`markup`]). The built-in
//! Typst ([`ghostwrite_typst`]) typesets a note in the background, once for
//! each text and look; until it's done (or if it fails) the note shows as
//! its text.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ghostwrite_typst::Compiler;
use gpui::{App, Global, Hsla, Pixels, RenderImage, Rgba, Size, px, size};

/// A typeset note, and the size it's drawn at.
#[derive(Clone)]
pub(super) struct Page {
    pub image: Arc<RenderImage>,
    pub size: Size<Pixels>,
}

/// How a note is typeset: as wide as its text would be, in the editor's font and color.
pub(super) struct Look {
    pub width: Pixels,
    pub font: gpui::SharedString,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub color: Hsla,
    /// Pixels per point on the screen.
    pub scale: f32,
}

/// Typeset notes by their source and resolution (`None` while compiling,
/// or if it failed), and when each was last shown.
#[derive(Default)]
struct Pages(HashMap<(String, u32), (Option<Page>, std::time::Instant)>);

impl Global for Pages {}

/// Enough for a story's notes at a few widths.
const KEPT: usize = 64;

/// A note as Typst markup. About a Typst file it already is; elsewhere only
/// its `$…$` is math, and the rest is set as written (`\$` is a dollar).
/// `None` if there's nothing to typeset, or a `$` isn't closed.
pub(super) fn markup(text: &str, typst_file: bool) -> Option<String> {
    if typst_file {
        return Some(text.to_string());
    }
    let (mut out, mut literal, mut math) = (String::new(), String::new(), None::<String>);
    let flush = |out: &mut String, literal: &mut String| {
        if !literal.is_empty() {
            let escaped = literal.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
            out.push_str(&format!("#\"{escaped}\""));
            literal.clear();
        }
    };
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, math.as_mut()) {
            ('\\', None) if chars.as_str().starts_with('$') => literal.push(chars.next()?),
            ('$', None) => {
                flush(&mut out, &mut literal);
                math = Some(String::new());
            }
            ('$', Some(m)) => {
                out.push_str(&format!("${m}$"));
                math = None;
            }
            ('\\', Some(m)) => m.extend(std::iter::once(c).chain(chars.next())),
            (c, Some(m)) => m.push(c),
            (c, None) => literal.push(c),
        }
    }
    if math.is_some() || !out.contains('$') {
        return None;
    }
    flush(&mut out, &mut literal);
    Some(out)
}

/// `text` typeset in `look`, if it's ready; if it isn't, it's started, and the
/// windows redraw when it's done.
pub(super) fn note(text: &str, look: &Look, cx: &mut App) -> Option<Page> {
    let key = (source(text, look), (72. * look.scale).round() as u32);
    let now = std::time::Instant::now();
    let pages = cx.default_global::<Pages>();
    if let Some((page, shown)) = pages.0.get_mut(&key) {
        *shown = now;
        return page.clone();
    }
    // Full: the ones shown longest ago go (failures too), never one on screen.
    if pages.0.len() >= KEPT {
        let mut old: Vec<_> =
            pages.0.iter().filter(|(_, (_, shown))| now - *shown > std::time::Duration::from_secs(2)).map(|(k, (_, shown))| (k.clone(), *shown)).collect();
        old.sort_by_key(|(_, shown)| *shown);
        let gone: Vec<Page> = old.into_iter().take(pages.0.len() + 1 - KEPT).filter_map(|(k, _)| pages.0.remove(&k)?.0).collect();
        for page in gone {
            cx.drop_image(page.image, None);
        }
    }
    let pages = cx.default_global::<Pages>();
    pages.0.insert(key.clone(), (None, now));
    let (src, ppi) = key.clone();
    let compiling = cx.background_executor().spawn(async move { compile(&src, ppi as f32 / 72.) });
    cx.spawn(async move |cx| {
        let page = compiling.await;
        drop(cx.update(|cx| {
            cx.default_global::<Pages>().0.insert(key, (page, std::time::Instant::now()));
            cx.refresh_windows();
        }));
    })
    .detach();
    None
}

/// A page as wide as the note, as tall as its text, in the editor's font
/// (the math in Typst's), one point to a pixel.
fn source(text: &str, look: &Look) -> String {
    let Rgba { r, g, b, a } = look.color.into();
    let hex: String = [r, g, b, a].iter().map(|c| format!("{:02x}", (c * 255.).round() as u8)).collect();
    let (width, size) = (f32::from(look.width).round(), f32::from(look.font_size));
    // Ascender to descender is about 1.2em; the rest of a line is leading.
    let leading = (f32::from(look.line_height) - size * 1.2).max(0.);
    format!(
        "#set page(width: {width}pt, height: auto, margin: (x: 0pt, y: 2pt), fill: none)\n\
         #set text(font: (\"{}\", \"New Computer Modern\"), size: {size}pt, fill: rgb(\"#{hex}\"), top-edge: \"ascender\", bottom-edge: \"descender\")\n\
         #set par(leading: {leading}pt)\n\
         {text}\n",
        look.font
    )
}

/// Typeset `source` at `scale` pixels to a point. It can't read the project:
/// its root is the temp folder. One compiler for every note, so each reuses
/// what the last computed.
fn compile(source: &str, scale: f32) -> Option<Page> {
    static NOTES: OnceLock<Option<Mutex<Compiler>>> = OnceLock::new();
    let compiler = NOTES.get_or_init(|| {
        let root = std::env::temp_dir();
        Compiler::new(&root, &root.join("ghostwrite-note.typ")).map(Mutex::new)
    });
    let document = compiler.as_ref()?.lock().ok()?.compile(source).ok()?;
    let page = document.pages.first()?;
    let bitmap = page.render(scale);
    let frame = image::Frame::new(image::RgbaImage::from_raw(bitmap.width as u32, bitmap.height as u32, bitmap.bgra)?);
    Some(Page { image: Arc::new(RenderImage::new(vec![frame])), size: size(px(page.width), px(page.height)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look() -> Look {
        Look { width: px(300.), font: "Menlo".into(), font_size: px(13.), line_height: px(20.), color: gpui::white(), scale: 2. }
    }

    #[test]
    fn outside_typst_only_the_math_is_markup() {
        assert_eq!(markup("x_1 *is* #1", true).as_deref(), Some("x_1 *is* #1"), "a Typst file's notes are Typst");
        assert_eq!(markup("It's O(n) on `a_b`.", false), None, "no math, no typesetting");
        assert_eq!(markup("costs \\$5 and $5", false), None, "an open `$` shows the text");
        assert_eq!(markup("The *sum* \"is\"\n$sum_i x_i$, \\$1.", false).as_deref(), Some("#\"The *sum* \\\"is\\\"\\n\"$sum_i x_i$#\", $1.\""));
        assert_eq!(markup("$a \\$ b$", false).as_deref(), Some("$a \\$ b$"), "an escaped dollar in math stays in it");
    }

    #[test]
    fn the_source_sets_the_look_and_then_the_note() {
        let src = source("The sum $sum_i i$.", &look());
        assert!(src.contains("width: 300pt") && src.contains("size: 13pt") && src.contains("rgb(\"#ffffffff\")"), "{src}");
        assert!(src.ends_with("The sum $sum_i i$.\n"));
    }

    #[test]
    fn math_compiles_to_a_page_as_wide_as_the_note() {
        let page = compile(&source("$a^2 + b^2 = c^2$", &look()), 2.).expect("typst compiles it");
        assert_eq!(page.size.width, px(300.));
        assert!(page.size.height > px(10.), "{:?}", page.size);
        assert!(compile(&source("$unbalanced", &look()), 2.).is_none(), "an error shows the text instead");
    }
}
