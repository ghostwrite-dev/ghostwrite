//! Ghostwrite: a helix-style editor with stepwise Claude stories.
//!
//! - [`workspace`]: the window: the projects page, or a project's editor
//! - [`editor`]: the modal editor (tabs, file tree, prompts, Claude, tours)
//! - [`projects_page`]: recent projects
//! - [`ui`]: shared widgets
//! - [`theme`], [`actions`], [`keys`]: look, menus, and keystrokes as modal keys

mod actions;
mod config;
mod editor;
mod keys;
mod projects_page;
mod theme;
mod ui;
mod workspace;

use std::path::PathBuf;

use futures::StreamExt as _;
use gpui::{AppContext as _, Application, Bounds, WindowBounds, WindowOptions, px, size};

use crate::workspace::Workspace;

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let config = config::get();
    ghostwrite_ai::configure(ghostwrite_ai::Settings { model: config.model.clone(), context_bytes: config.context_bytes, max_diff: config.max_diff });
    ghostwrite_project::store::set_max_recent(config.max_recent_projects);

    // Finder hands files over as file:// URLs, at launch or while running.
    let (open_tx, mut open_rx) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
    let app = Application::new();
    app.on_open_urls(move |urls| {
        let paths = urls.iter().filter_map(|u| ghostwrite_lsp::convert::path_of(u)).collect();
        open_tx.unbounded_send(paths).ok();
    });

    app.run(move |cx| {
        actions::install(cx);
        theme::install(cx.window_appearance(), cx);
        cx.on_window_closed(|cx| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1100.), px(780.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let window = cx
            .open_window(options, |window, cx| {
                theme::observe(window);
                cx.new(|cx| Workspace::new(paths, window, cx))
            })
            .expect("the window opens");
        cx.spawn(async move |cx| {
            while let Some(paths) = open_rx.next().await {
                window
                    .update(cx, |workspace, window, cx| {
                        window.activate_window();
                        workspace.open_paths(paths, window, cx);
                    })
                    .ok();
            }
        })
        .detach();
        cx.activate(true);
    });
}
