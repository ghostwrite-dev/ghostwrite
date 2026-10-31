//! Ghostwrite: a helix-style editor with stepwise Claude stories.
//!
//! - [`workspace`]: the window: the projects page, or a project's editor
//! - [`editor`]: the modal editor (tabs, file tree, prompts, stories)
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
use gpui::Application;

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

    // The servers and the login shell's environment, looked up while the window opens.
    ghostwrite_lsp::warm();
    app.run(move |cx| {
        actions::install(cx);
        theme::install(cx.window_appearance(), cx);
        // Closing the last window quits, as before there could be more than one.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.on_action(|_: &actions::NewWindow, cx| {
            workspace::open_window(vec![], cx);
        });
        workspace::open_window(paths, cx).expect("the window opens");
        cx.spawn(async move |cx| {
            while let Some(paths) = open_rx.next().await {
                cx.update(|cx| workspace::open_anywhere(paths, cx)).ok();
            }
        })
        .detach();
        cx.activate(true);
    });
}
