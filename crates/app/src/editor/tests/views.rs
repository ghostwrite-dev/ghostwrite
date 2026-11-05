//! Views: the keys going from the code to Files, the terminal and a plugin's views and
//! back, the mode line saying where they are, each view's own keys (yours, and a
//! plugin's), and a story reviewed in whichever view has them.

use ghostwrite_plugin::{Binding, Block, Panel, Plugin};

use super::terminal::{printed, wait_for};
use super::*;
use crate::editor::document::Document;

/// What a plugin shows beside a file, for a test: a heading.
struct Goals;

impl Panel for Goals {
    fn title(&self) -> &'static str {
        "Goals"
    }
    fn requests(&self) -> &'static [&'static str] {
        &[]
    }
    fn answered(&mut self, _: Vec<Option<serde_json::Value>>) {}
    fn blocks(&self) -> Vec<Block> {
        (0..80).map(|i| Block::Muted(format!("goal {i}"))).collect()
    }
}

/// A plugin with keys in its views: `H` on pages (an editor command, by name), `K`
/// beside a file (its own command). For files of no language only, so it's in no one else's way.
struct Views;

impl Plugin for Views {
    fn commands(&self) -> &'static [ghostwrite_plugin::PluginCommand] {
        &[ghostwrite_plugin::PluginCommand { name: "views-test", what: "" }]
    }

    fn run(&self, _: &str, _: &[String], _: &ghostwrite_plugin::Context) -> ghostwrite_plugin::Reply {
        Ok(vec![ghostwrite_plugin::Action::Say("ran in its view".into())])
    }

    fn keys(&self, language: Option<&ghostwrite_core::Language>) -> &'static [Binding] {
        const KEYS: &[Binding] = &[
            Binding { keys: "H", view: ViewKind::Pages, command: "scroll_page_down", what: "a screen down" },
            Binding { keys: "K", view: ViewKind::Aside, command: "views-test", what: "say so" },
        ];
        if language.is_none() { KEYS } else { &[] }
    }
}

fn with_views_plugin<'a>(text: &str, cx: &'a mut TestAppContext) -> Setup<'a> {
    static ADDED: std::sync::Once = std::sync::Once::new();
    ADDED.call_once(|| ghostwrite_plugin::add(Box::new(Views)));
    open(text, &[], cx)
}

/// What a plugin shows beside the file in front, shown.
fn show_goals(t: &mut Setup) {
    t.editor.update(t.cx, |e, _| {
        e.project.aside_shown = true;
        e.tab_mut().unwrap().doc.aside = Some(crate::editor::aside::Aside::new(Box::new(Goals)));
    });
    draw(t);
}

/// A tab of doc.txt's pages, in front.
fn open_pages(t: &mut Setup) {
    t.editor.update(t.cx, |e, cx| {
        let of = e.tab().unwrap().doc.id;
        let ix = e.project.tabs.add(|id| Document::view(id, of, "doc.txt · pages".into()));
        e.activate(ix, cx);
    });
}

/// How far doc.txt's pages are scrolled down.
fn pages_top(t: &mut Setup) -> f32 {
    t.read(|e| e.project.tabs.iter().find(|tab| tab.doc.view_of.is_none()).unwrap().view.pages.top)
}

fn draw(t: &mut Setup) {
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
}

#[gpui::test]
fn ctrl_w_h_j_k_l_go_from_view_to_view_by_where_they_are(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("b.txt"), "b\n").unwrap();
    t.command("e b.txt");
    t.command("bp");
    t.keys("space t").input("echo here").keys("enter");
    wait_for(&mut t, printed("here"));
    t.keys("escape escape");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal));
    t.keys("ctrl-w k");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "the code is above the terminal");
    t.keys("ctrl-w j");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal), "and the terminal below it, in the mode the keys were in");
    assert!(t.read(|e| e.reading()), "on its text");
    t.keys("ctrl-w h");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal), "Files is left of the terminal, as of the code");
    t.keys("ctrl-w l");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "and the code right of it");
    t.keys("ctrl-w k");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "nothing's above the code");

    // Two panes: doc.txt with the keys on the left, b.txt on the right.
    t.keys("ctrl-w v");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("ctrl-w l");
    assert_eq!((t.at().0, t.active().as_deref()), (ViewKind::Code, Some("b.txt")), "the pane on the right");
    t.keys("ctrl-w l");
    assert_eq!(t.active().as_deref(), Some("b.txt"), "and nothing's right of that");
    t.keys("ctrl-w h");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "back to the left pane");
    t.keys("ctrl-w h");
    assert_eq!(t.at().0, ViewKind::Files);
    t.keys("ctrl-w l");
    assert_eq!((t.at().0, t.active().as_deref()), (ViewKind::Code, Some("doc.txt")), "from Files, the pane next to it");
    t.keys("ctrl-w w");
    assert_eq!(t.active().as_deref(), Some("b.txt"), "ctrl-w w is the other pane, as ever");
    t.keys("ctrl-w q");
    assert!(t.read(|e| e.split.is_none()), "and ctrl-w q only this one");
}

#[gpui::test]
fn space_e_and_space_t_go_there_and_again_back_to_the_code(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal));
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal));
    t.keys("space t");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Insert), "the terminal, typing in it");
    t.keys("escape escape");
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal), "from the terminal to Files");
    t.keys("space t");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Insert), "and from Files to the terminal");
    t.keys("escape escape space t");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "space t on the terminal goes back to the code");
    assert!(t.read(|e| e.terminal.shown), "which leaves it up");
}

#[gpui::test]
fn esc_esc_or_ctrl_backslash_ctrl_n_stop_typing_in_the_terminal(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    // A program that takes an Esc as it comes (the shell would take it as alt with the next key).
    t.keys("space t").input("cat -v").keys("enter");
    wait_for(&mut t, |e| e.terminal.current().unwrap().busy() == Some(true));
    t.keys("escape");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Insert), "one Esc is the shell's");
    t.cx.executor().advance_clock(std::time::Duration::from_millis(300));
    t.cx.run_until_parked();
    assert!(t.read(|e| e.terminal.held_escape.is_none()), "and goes to it after a moment");
    t.keys("escape escape");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal), "two leave typing");
    t.keys("i");
    t.keys("ctrl-\\ ctrl-n");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal), "so does ctrl-\\ ctrl-n, as in Neovim");
    assert!(t.read(|e| e.reading()), "to move through its text");
    t.keys("i ctrl-c");
    wait_for(&mut t, |e| e.terminal.current().unwrap().busy() == Some(false));

    // A full-screen program: Esc is its own, however often, and ctrl-\ ctrl-n still leaves.
    t.input("printf '\\033[?1049h'; cat -v").keys("enter");
    wait_for(&mut t, |e| e.terminal.current().unwrap().modes().full_screen);
    t.keys("escape escape escape");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Insert), "the program has every Esc");
    t.keys("ctrl-\\ ctrl-n");
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal));
    t.keys("i ctrl-c");
}

#[gpui::test]
fn the_mode_line_says_the_mode_and_the_view(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    assert_eq!(t.mode_line(), "NOR", "the code is just the mode");
    t.keys("i");
    assert_eq!(t.mode_line(), "INS");
    t.keys("escape space t");
    assert_eq!(t.mode_line(), "INS · terminal");
    t.keys("escape escape");
    assert_eq!(t.mode_line(), "NOR · terminal");
    t.keys("v");
    assert_eq!(t.mode_line(), "SEL · terminal");
    t.keys("escape escape");
    assert_eq!(t.mode_line(), "NOR", "Esc ends select mode, then goes back to the code");
    t.keys("space e");
    assert_eq!(t.mode_line(), "NOR · files");
    t.keys("a");
    assert_eq!(t.mode_line(), "INS · files", "naming a file is typing");
    t.keys("escape");
    assert_eq!(t.mode_line(), "NOR · files");
    t.keys("escape");
    open_pages(&mut t);
    assert_eq!(t.mode_line(), "NOR · pages", "a tab of pages is a plugin's view");
    t.command("bp");
    show_goals(&mut t);
    t.keys("ctrl-w l");
    assert_eq!(t.mode_line(), "NOR · goals", "and what's beside a file, by its title");
    draw(&mut t);
    let drawn = t.cx.debug_bounds("mode-line");
    assert!(drawn.is_some(), "it's drawn");
}

#[gpui::test]
fn ctrl_w_l_puts_the_keys_beside_the_file_where_j_scrolls_and_esc_goes_back(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(300.)));
    show_goals(&mut t);
    t.keys("ctrl-w l");
    assert_eq!(t.at(), (ViewKind::Aside, Mode::Normal));
    t.keys("j j");
    draw(&mut t);
    assert!(t.read(|e| e.aside_scroll.offset().y) < gpui::px(0.), "j scrolls it");
    assert_eq!(t.text(), "x\n");
    t.keys("x d");
    assert_eq!(t.text(), "x\n", "the code's keys don't reach the code from here");
    t.keys("escape");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal));
    t.keys("ctrl-w l ctrl-w h");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "ctrl-w h from beside it is the code");
    let aside = t.cx.debug_bounds("aside").expect("it's drawn");
    t.cx.simulate_click(aside.center(), gpui::Modifiers::none());
    assert_eq!(t.at().0, ViewKind::Aside, "a click puts the keys there too");
    t.editor.update(t.cx, |e, _| e.project.aside_shown = false);
    t.keys("j");
    assert_eq!(t.at().0, ViewKind::Code, "hidden, the keys are back on the code");
}

#[gpui::test]
fn your_keys_can_be_a_views_own(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    std::fs::write(t.dir.join("a.txt"), "").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    let keys = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let (keymap, _) = ghostwrite_core::Keymap::default().with_yours_in(ViewKind::Files, &keys(&[("J", "files_down"), ("j", "no_op")]));
    let (keymap, _) = keymap.with_yours_in(ViewKind::Pages, &keys(&[("n", "scroll_page_down")]));
    let keymap: &'static ghostwrite_core::Keymap = Box::leak(Box::new(keymap));
    t.editor.update(t.cx, |e, _| e.use_keys(keymap, &[]));
    t.keys("space e g g");
    let row = |t: &mut Setup| t.read(|e| e.project.tree.cursor());
    t.keys("J");
    assert_eq!(row(&mut t), 1, "J is a row down in Files");
    t.keys("j");
    assert_eq!(row(&mut t), 1, "and j nothing there");
    t.keys("escape j");
    assert_eq!(t.cursor(), "one\n".len(), "the code's j is as it was");
    open_pages(&mut t);
    t.keys("n");
    assert!(pages_top(&mut t) > 40., "n is a screen down on pages");
}

#[gpui::test]
fn a_plugin_binds_keys_in_its_own_views(cx: &mut TestAppContext) {
    let mut t = with_views_plugin("x\n", cx);
    t.keys("H");
    assert_eq!(t.text(), "x\n", "nothing in the code");
    open_pages(&mut t);
    let top = pages_top(&mut t);
    t.keys("j");
    assert_eq!(pages_top(&mut t), top + 40., "the view's own keys");
    t.keys("H");
    assert!(pages_top(&mut t) > top + 80., "and the plugin's, an editor command by name");
    t.command("bp");
    show_goals(&mut t);
    t.keys("ctrl-w l K");
    t.cx.run_until_parked();
    assert_eq!(t.status().as_deref(), Some("ran in its view"), "and its own command, beside the file");
}

#[gpui::test]
fn the_cheatsheet_has_each_views_keys_and_every_section(cx: &mut TestAppContext) {
    let _t = open("", &[], cx);
    let sheet = crate::editor::view::cheatsheet(ghostwrite_core::Keymap::standard());
    let section = |title: &str| sheet.iter().find(|(t, _)| t == title).map(|(_, rows)| rows.iter().map(|row| (row.view, row.keys.clone())).collect::<Vec<_>>());
    let files = section("Files  space e").expect("Files has a section");
    assert!(files.contains(&(ViewKind::Files, "j down k up".into())), "{files:?}");
    assert!(files.contains(&(ViewKind::Files, "a r d".into())), "{files:?}");
    let terminal = section("Terminal  space t").expect("so does the terminal");
    assert!(terminal.contains(&(ViewKind::Terminal, "enter".into())), "{terminal:?}");
    assert!(terminal.iter().any(|(_, keys)| keys.contains("ctrl-\\ ctrl-n")), "{terminal:?}");
    assert!(section("Pages  :preview").is_some() && section("Views  ctrl-w").is_some() && section("Window  ⌘").is_some());
    // Every section has a place in the columns, in order.
    let titles: Vec<String> = sheet.iter().map(|(title, _)| title.clone()).collect();
    let columns = crate::editor::view::columns(sheet, 3);
    assert_eq!(columns.len(), 3);
    let placed: Vec<String> = columns.into_iter().flatten().map(|(title, _)| title).collect();
    assert_eq!(placed, titles);
}

#[gpui::test]
fn a_story_goes_from_the_code_to_files_to_the_terminal_reviewed_wherever_the_keys_are(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let create = "<create>\nnew.txt\n</create>\n<say>\nA new file.\n</say>\n<new>\nhello\n</new>\n";
    let run = "<run>\necho ran-$((40 + 2))\n</run>\n<say>\nCheck.\n</say>\n";
    let mut t = open(ADD, &[&fix(), create, run, DONE], cx);
    t.keys("space e");
    t.keys("space a").input("go on").keys("enter");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Ai), "the first step, in the code");
    t.keys("ctrl-w h");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Ai), "the keys can go elsewhere, the story still up");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "and Tab accepts from there");
    assert_eq!(t.read(|e| e.attention.on), crate::editor::attention::At::Files, "a new file is proposed in Files");
    t.keys("tab");
    assert_eq!(t.read(|e| e.attention.on), crate::editor::attention::At::Terminal);
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Ai), "the command: the story takes the keys to the terminal");
    assert!(t.read(|e| e.reading() && e.command_proposed()));
    t.keys("tab");
    wait_for(&mut t, |e| e.story.is_none());
    wait_for(&mut t, printed("ran-42"));
    assert_eq!(t.at(), (ViewKind::Terminal, Mode::Normal), "it ended with a command: the keys stay on the terminal");
}
