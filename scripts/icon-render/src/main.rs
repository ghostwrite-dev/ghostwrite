//! `icon-render SVG OUT_DIR`: the icon at each size the platforms take. An iconset for
//! `iconutil` (macOS), a 512 px PNG (Linux), and an .ico of PNGs (Windows).

use std::path::Path;

fn drawn(tree: &resvg::usvg::Tree, size: u32) -> Vec<u8> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("a size");
    let scale = size as f32 / tree.size().width();
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().expect("encoded")
}

/// An .ico holding a PNG for each size (Vista on reads them).
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend(png);
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (svg, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let tree = resvg::usvg::Tree::from_data(&std::fs::read(svg).expect("the svg"), &Default::default()).expect("an svg");
    let set = out.join("ghostwrite.iconset");
    std::fs::create_dir_all(&set).expect("a folder");
    for size in [16, 32, 128, 256, 512] {
        std::fs::write(set.join(format!("icon_{size}x{size}.png")), drawn(&tree, size)).expect("written");
        std::fs::write(set.join(format!("icon_{size}x{size}@2x.png")), drawn(&tree, size * 2)).expect("written");
    }
    std::fs::write(out.join("ghostwrite.png"), drawn(&tree, 512)).expect("written");
    let sizes: Vec<(u32, Vec<u8>)> = [16, 24, 32, 48, 64, 256].into_iter().map(|size| (size, drawn(&tree, size))).collect();
    std::fs::write(out.join("ghostwrite.ico"), ico(&sizes)).expect("written");
}
