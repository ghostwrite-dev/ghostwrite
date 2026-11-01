//! The few things that aren't modal keys: menu items and the standard Mac
//! shortcuts. Everything else is a key the modal editor reads, or a `:`
//! command.

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
    ]
);

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
    cx.set_menus(vec![
        Menu { name: "Ghostwrite".into(), items: vec![MenuItem::action("Quit Ghostwrite", Quit)] },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open…", Open),
                MenuItem::action("Open Folder…", OpenFolder),
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
