//! The few things that aren't modal keys: menu items and the standard Mac
//! shortcuts. Everything else is a key the modal editor reads, or a `:`
//! command.

use std::path::PathBuf;

use ghostwrite_project::store;
use gpui::{App, KeyBinding, Menu, MenuItem, actions};

actions!(
    ghostwrite,
    [
        Quit,
        NewWindow,
        Open,
        OpenFolder,
        ShowProjects,
        Save,
        SaveAs,
        CloseTab,
        NewTab,
        NextTab,
        MoveTabLeft,
        MoveTabRight,
        PrevTab,
        ToggleTree,
        ToggleTerminal,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        SelectAll,
        ClearRecent,
    ]
);

/// File › Open Recent: a project opened before.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = ghostwrite, no_json)]
pub struct OpenRecent {
    pub root: PathBuf,
}

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(&crate::keys::app("cmd-q"), Quit, None),
        KeyBinding::new(&crate::keys::app("cmd-o"), Open, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-shift-o"), OpenFolder, None),
        KeyBinding::new(&crate::keys::app("cmd-shift-n"), NewWindow, None),
        KeyBinding::new(&crate::keys::app("cmd-="), ZoomIn, None),
        KeyBinding::new(&crate::keys::app("cmd-+"), ZoomIn, None),
        KeyBinding::new(&crate::keys::app("cmd--"), ZoomOut, None),
        KeyBinding::new(&crate::keys::app("cmd-0"), ResetZoom, None),
        KeyBinding::new(&crate::keys::app("cmd-s"), Save, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-shift-s"), SaveAs, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-w"), CloseTab, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-n"), NewTab, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-t"), NewTab, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-alt-left"), MoveTabLeft, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-alt-right"), MoveTabRight, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-shift-]"), NextTab, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-shift-["), PrevTab, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-z"), Undo, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-shift-z"), Redo, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-c"), Copy, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-x"), Cut, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-v"), Paste, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-a"), SelectAll, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-j"), ToggleTerminal, Some("Editor")),
        KeyBinding::new(&crate::keys::app("cmd-b"), ToggleTree, Some("Editor")),
    ]);
    crate::ui::text_field::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| crate::workspace::quit(false, cx));
    cx.on_action(|_: &ZoomIn, cx| crate::theme::zoom(1, cx));
    cx.on_action(|_: &ZoomOut, cx| crate::theme::zoom(-1, cx));
    cx.on_action(|_: &ResetZoom, cx| crate::theme::zoom(0, cx));
    // With no window to take it (one does, in its own place), a window of its own.
    cx.on_action(|recent: &OpenRecent, cx| crate::workspace::open_anywhere(vec![recent.root.clone()], cx));
    cx.on_action(|_: &ClearRecent, cx| {
        store::recent_projects().iter().for_each(|root| store::forget_project(root));
        set_menus(cx);
    });
    set_menus(cx);
}

/// A project as the menu shows it: its folder, `~/…` if it's in home.
fn label(root: &std::path::Path) -> String {
    match std::env::home_dir().and_then(|home| root.strip_prefix(home).ok().map(std::path::Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => root.display().to_string(),
    }
}

/// The menu bar, with the projects opened lately: set again when they change.
pub fn set_menus(cx: &mut App) {
    let mut recent: Vec<MenuItem> =
        store::recent_projects().into_iter().filter(|root| root.is_dir()).map(|root| MenuItem::action(label(&root), OpenRecent { root })).collect();
    if !recent.is_empty() {
        recent.extend([MenuItem::separator(), MenuItem::action("Clear Menu", ClearRecent)]);
    }
    cx.set_menus(vec![
        Menu { name: "Ghostwrite".into(), items: vec![MenuItem::action("Quit Ghostwrite", Quit)] },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open…", Open),
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::submenu(Menu { name: "Open Recent".into(), items: recent }),
                MenuItem::action("Switch Project…", ShowProjects),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Save As…", SaveAs),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle File Tree", ToggleTree),
                MenuItem::action("Toggle Terminal", ToggleTerminal),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Actual Size", ResetZoom),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::action("Move Tab Left", MoveTabLeft),
                MenuItem::action("Move Tab Right", MoveTabRight),
            ],
        },
    ]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn recent_projects_are_shown_from_home() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(super::label(&home.join("Projects/ghostwrite")), "~/Projects/ghostwrite");
        assert_eq!(super::label(std::path::Path::new("/opt/work")), "/opt/work");
    }
}
