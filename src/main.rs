mod assist;
mod command;
mod editor;
mod helix;
mod lsp;
mod modal;
mod project;
mod projects;
mod shell;
mod theme;
mod tree;
mod workspace;

use std::path::PathBuf;

use futures::StreamExt as _;
use gpui_kit::*;

use crate::workspace::Workspace;

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    // Finder hands files over as file:// URLs, at launch or while running.
    let (open_tx, mut open_rx) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
    let app = gpui_kit::application();
    app.on_open_urls(move |urls| {
        let paths = urls.iter().filter_map(|u| u.strip_prefix("file://")).map(|p| PathBuf::from(percent_decode(p))).collect();
        open_tx.unbounded_send(paths).ok();
    });

    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        editor::bind_keys(cx);
        tree::bind_keys(cx);
        cx.on_action(|_: &editor::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1000.), px(760.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let (window, editor) =
            gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(paths, window, cx)))
                .expect("failed to open window");
        cx.spawn(async move |cx| {
            while let Some(paths) = open_rx.next().await {
                cx.update_window(window, |_, window, cx| {
                    window.activate_window();
                    editor.update(cx, |editor, cx| editor.open_paths(paths, window, cx));
                })
                .ok();
            }
        })
        .detach();
        cx.activate(true);
    });
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
