//! The few things that aren't modal keys: menu items and the standard Mac
//! shortcuts. Everything else is a key the modal editor reads, or a `:`
//! command.

use gpui::{App, KeyBinding, Menu, MenuItem, actions};

actions!(
    ghostwrite,
    [Quit, Open, OpenFolder, ShowProjects, Save, SaveAs, CloseTab, NewTab, NextTab, PrevTab, ToggleTree, Undo, Redo, Copy, Cut, Paste, SelectAll,]
);

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-o", Open, Some("Editor")),
        KeyBinding::new("cmd-shift-o", OpenFolder, None),
        KeyBinding::new("cmd-s", Save, Some("Editor")),
        KeyBinding::new("cmd-shift-s", SaveAs, Some("Editor")),
        KeyBinding::new("cmd-w", CloseTab, Some("Editor")),
        KeyBinding::new("cmd-t", NewTab, Some("Editor")),
        KeyBinding::new("cmd-z", Undo, Some("Editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
    ]);
    crate::ui::text_field::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![
        Menu { name: "Ghostwrite".into(), items: vec![MenuItem::action("Quit Ghostwrite", Quit)] },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
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
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Previous Tab", PrevTab),
            ],
        },
    ]);
}
