//! Views: the keys going from the code to Files, the terminal, the debugger and a
//! plugin's views and back with `space` and a letter, the panes under `space w`, the mode line saying where they are, each view's own keys (yours, and a
//! plugin's), and a story reviewed in whichever view has them.

use ghostwrite_core::{Language, PluginView, Shows, ViewId, ViewPlace};
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

/// The test plugin's views, with the keys a test gives them ([`test_keys`]): as
/// registered, they have none, so that they're in no one else's way.
const PAGES: PluginView =
    PluginView { id: ViewId { plugin: "views-test", name: "pages", shows: Shows::Pages }, title: "Pages", place: ViewPlace::Tab, key: Some("space v") };
const GOALS: PluginView =
    PluginView { id: ViewId { plugin: "views-test", name: "goals", shows: Shows::Blocks }, title: "Goals", place: ViewPlace::Beside, key: Some("space g") };

/// The test plugin's language: its views are of its files.
static VIEWS_TEST: Language = Language { name: "views-test", extensions: &["viewstest"], ..Language::DEFAULT };

/// A plugin with views, and keys in them: `H` on its pages (an editor command, by name),
/// `K` beside a file (its own command). For files of its own language only.
struct Views;

impl Plugin for Views {
    fn name(&self) -> &str {
        "views-test"
    }

    fn commands(&self) -> &'static [ghostwrite_plugin::PluginCommand] {
        &[ghostwrite_plugin::PluginCommand { name: "views-test", what: "" }]
    }

    fn run(&self, _: &str, _: &[String], _: &ghostwrite_plugin::Context) -> ghostwrite_plugin::Reply {
        Ok(vec![ghostwrite_plugin::Action::Say("ran in its view".into())])
    }

    fn languages(&self) -> Vec<&'static Language> {
        vec![&VIEWS_TEST]
    }

    fn views(&self) -> &'static [PluginView] {
        const VIEWS: &[PluginView] = &[PluginView { key: None, ..PAGES }, PluginView { key: None, ..GOALS }];
        VIEWS
    }

    fn keys(&self, language: Option<&ghostwrite_core::Language>) -> &'static [Binding] {
        const KEYS: &[Binding] = &[
            Binding { keys: "H", view: ViewKind::Plugin(PAGES.id), command: "scroll_page_down", what: "a screen down" },
            Binding { keys: "K", view: ViewKind::Plugin(GOALS.id), command: "views-test", what: "say so" },
        ];
        if language.is_some_and(|language| language.name == VIEWS_TEST.name) { KEYS } else { &[] }
    }
}

fn with_views_plugin<'a>(text: &str, cx: &'a mut TestAppContext) -> Setup<'a> {
    static ADDED: std::sync::Once = std::sync::Once::new();
    ADDED.call_once(|| ghostwrite_plugin::add(Box::new(Views)));
    open(text, &[], cx)
}

/// The editor's keys, and the test plugin's views' on `space v` and `space g`.
fn test_keys(t: &mut Setup) {
    let keymap: &'static ghostwrite_core::Keymap = Box::leak(Box::new(ghostwrite_core::Keymap::default().with_views(&[PAGES, GOALS])));
    t.editor.update(t.cx, |e, _| e.use_keys(keymap, &[]));
}

/// What a plugin shows beside the file in front, shown.
fn show_goals(t: &mut Setup) {
    t.editor.update(t.cx, |e, _| {
        e.project.aside_shown = true;
        e.tab_mut().unwrap().doc.aside = Some(crate::editor::aside::Aside::new(GOALS, Box::new(Goals)));
    });
    draw(t);
}

/// A tab of the pages of the file in front, in front.
fn open_pages(t: &mut Setup) {
    t.editor.update(t.cx, |e, cx| {
        let tab = e.tab().unwrap();
        let (of, name) = (tab.doc.id, format!("{} · pages", tab.doc.name()));
        let ix = e.project.tabs.add(|id| Document::view(id, of, name));
        e.activate(ix, cx);
    });
}

/// The view a tab of pages of a file no plugin has views of is.
fn pages_of_none() -> ViewKind {
    ViewKind::Plugin(ViewId { plugin: "", name: "pages", shows: Shows::Pages })
}

/// How far the pages in front are scrolled down.
fn pages_top(t: &mut Setup) -> f32 {
    t.read(|e| e.project.tabs.by_id(e.shown_pages().unwrap()).unwrap().view.pages.top)
}

fn draw(t: &mut Setup) {
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
}

#[gpui::test]
fn space_w_v_w_q_o_are_the_panes_and_ctrl_w_moves_nowhere(cx: &mut TestAppContext) {
    let mut t = open("xy\n", &[], cx);
    std::fs::write(t.dir.join("b.txt"), "b\n").unwrap();
    t.command("e b.txt");
    t.command("bp");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("ctrl-w");
    assert_eq!(t.read(|e| e.modal.pending_keys()), "", "ctrl-w leads nowhere in normal mode");
    t.keys("l");
    assert_eq!(t.cursor(), 1, "so l after it is the code's own");
    assert_eq!((t.at(), t.active().as_deref()), ((ViewKind::Code, Mode::Normal), Some("doc.txt")));
    t.keys("space e ctrl-w l");
    assert_eq!(t.at().0, ViewKind::Files, "nor from Files");
    t.keys("escape");

    t.keys("space w v");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "space w v: two panes, the keys where they were");
    let other = |t: &mut Setup| t.read(|e| e.other_pane_doc().and_then(|doc| e.project.tabs.by_id(doc)).map(|tab| tab.doc.name().to_string()));
    assert_eq!(other(&mut t).as_deref(), Some("b.txt"), "the next tab in the other");
    t.keys("space w w");
    assert_eq!(t.active().as_deref(), Some("b.txt"), "space w w: the other pane");
    t.keys("space w w");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "and back");
    t.keys("space w q");
    assert!(t.read(|e| e.split.is_none()), "space w q: only this one");
    t.keys("space w v");
    assert!(t.read(|e| e.split.is_some()));
    t.keys("space w o");
    assert!(t.read(|e| e.split.is_none()), "space w o too");
    assert_eq!(t.text(), "xy\n", "and nothing was typed on the way");
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
#[cfg_attr(windows, ignore = "needs a POSIX shell and its tools")]
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
    test_keys(&mut t);
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
    t.keys("space g");
    assert_eq!(t.mode_line(), "NOR · goals", "and what's beside a file, by its title");
    draw(&mut t);
    let drawn = t.cx.debug_bounds("mode-line");
    assert!(drawn.is_some(), "it's drawn");
}

#[gpui::test]
fn a_views_key_puts_the_keys_beside_the_file_where_j_scrolls_and_its_key_or_esc_goes_back(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    test_keys(&mut t);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(300.)));
    t.keys("space g");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "no goals beside this file");
    assert!(t.status().is_some_and(|said| said.contains("no goals view")), "{:?}", t.status());
    show_goals(&mut t);
    t.editor.update(t.cx, |e, _| e.project.aside_shown = false);
    t.keys("space g");
    assert_eq!(t.at(), (GOALS.kind(), Mode::Normal), "its key shows it, and puts the keys there");
    assert!(t.read(|e| e.project.aside_shown));
    t.keys("j j");
    draw(&mut t);
    assert!(t.read(|e| e.aside_scroll.offset().y) < gpui::px(0.), "j scrolls it");
    assert_eq!(t.text(), "x\n");
    t.keys("x d");
    assert_eq!(t.text(), "x\n", "the code's keys don't reach the code from here");
    t.keys("space g");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "its key again: back to the code");
    assert!(t.read(|e| e.aside_showing()), "which leaves it up");
    t.keys("space g escape");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "esc goes back too");
    t.keys("space e space g");
    assert_eq!(t.at().0, GOALS.kind(), "from Files too");
    t.keys("escape");
    let aside = t.cx.debug_bounds("aside").expect("it's drawn");
    t.cx.simulate_click(aside.center(), gpui::Modifiers::none());
    assert_eq!(t.at().0, GOALS.kind(), "a click puts the keys there too");
    t.editor.update(t.cx, |e, _| e.project.aside_shown = false);
    t.keys("j");
    assert_eq!(t.at().0, ViewKind::Code, "hidden, the keys are back on the code");
}

/// A plugin's panel opens with its view's key, made for the file as it's asked for.
#[gpui::test]
fn a_files_plugins_view_beside_it_opens_with_its_key(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let path = t.dir.join("Basic.lean");
    std::fs::write(&path, "example : 1 = 1 := rfl\n").unwrap();
    t.editor.update(t.cx, |e, cx| e.open_paths(vec![path], cx));
    assert!(t.read(|e| e.tab().unwrap().doc.aside.is_none() && !e.aside_showing()), "nothing beside it until it's asked for");
    let view = t.read(|e| ghostwrite_plugin::view_showing(Shows::Blocks, e.tab().unwrap().doc.language)).expect("the plugin's view");
    t.keys(view.key.expect("with a key"));
    assert_eq!(t.at(), (view.kind(), Mode::Normal));
    assert_eq!(t.mode_line(), format!("NOR · {}", view.title.to_lowercase()), "by its title");
    draw(&mut t);
    assert!(t.cx.debug_bounds("aside").is_some(), "drawn");
    t.keys(view.key.unwrap());
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal));
}

/// While something's being debugged, the debugger is a view of its own beside the code:
/// alone, or over a plugin's panel.
#[gpui::test]
fn the_debugger_is_a_view_beside_the_code_on_space_x(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    test_keys(&mut t);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(600.)));
    t.keys("space x");
    assert_eq!(t.at().0, ViewKind::Code, "nothing's being debugged");
    assert!(t.status().is_some_and(|said| said.starts_with("nothing's being debugged")), "{:?}", t.status());
    // A debugger that answers `initialize`, and then listens.
    let adapter = t.dir.join("adapter.sh");
    let answer = r#"{"seq":1,"type":"response","request_seq":1,"success":true,"command":"initialize","body":{}}"#;
    std::fs::write(&adapter, format!("printf 'Content-Length: {}\\r\\n\\r\\n%s' '{answer}'\nexec cat > /dev/null\n", answer.len())).unwrap();
    let adapter = vec!["sh".to_string(), adapter.display().to_string()];
    let frame = ghostwrite_dap::Frame { id: 1, name: "main".into(), path: None, line: None };
    let first = ghostwrite_dap::Variable { name: "first".into(), value: "40".into(), children: 0 };
    t.editor.update(t.cx, |e, cx| e.pretend_debugging(&adapter, vec![frame], vec![first], cx)).unwrap();
    draw(&mut t);
    let pane = t.cx.debug_bounds("debug-pane").expect("the debugger is drawn");
    let aside = t.cx.debug_bounds("aside").expect("beside the code");
    assert!(aside.contains(&pane.center()), "{pane:?} in {aside:?}");
    assert!(pane.size.height > aside.size.height * 0.9, "alone, it has the whole of it: {pane:?} in {aside:?}");
    let code = t.cx.debug_bounds("code-view").unwrap();
    assert!(pane.origin.x >= code.origin.x + code.size.width, "beside the code, not under it");
    t.keys("space x");
    assert_eq!(t.at(), (ViewKind::Debugger, Mode::Normal), "space x goes there");
    assert_eq!(t.mode_line(), "NOR · debugger");
    t.keys("space x");
    assert_eq!(t.at().0, ViewKind::Code, "and back");

    // With a plugin's panel: the debugger on top, each a view.
    show_goals(&mut t);
    let pane = t.cx.debug_bounds("debug-pane").expect("still drawn");
    let aside = t.cx.debug_bounds("aside").unwrap();
    assert_eq!(pane.origin.y, aside.origin.y, "on top");
    assert!(pane.size.height < aside.size.height, "over the panel");
    t.cx.simulate_click(aside.center() + gpui::point(gpui::px(0.), aside.size.height / 4.), gpui::Modifiers::none());
    assert_eq!(t.at().0, GOALS.kind(), "a click on the panel puts the keys there");
    assert_eq!(t.mode_line(), "NOR · goals");
    t.cx.simulate_click(pane.center(), gpui::Modifiers::none());
    assert_eq!(t.at().0, ViewKind::Debugger, "and on the debugger, there");
    t.keys("space g");
    assert_eq!(t.at().0, GOALS.kind(), "from one to the other with their keys");
    t.keys("space x");
    assert_eq!(t.at().0, ViewKind::Debugger);
    t.command("stop");
    draw(&mut t);
    assert!(t.read(|e| e.debug.session.is_none()), "stopped: {:?}", t.status());
    assert_eq!(t.mode_line(), "NOR", "and the keys are back on the code");
    assert!(t.cx.debug_bounds("panel").is_some(), "the panel's all there is");
}

#[gpui::test]
fn your_keys_can_be_a_views_own(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    std::fs::write(t.dir.join("a.txt"), "").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    let keys = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let (keymap, _) = ghostwrite_core::Keymap::default().with_yours_in(ViewKind::Files, &keys(&[("J", "files_down"), ("j", "no_op")]));
    let (keymap, _) = keymap.with_yours_in(pages_of_none(), &keys(&[("n", "scroll_page_down")]));
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
    test_keys(&mut t);
    let path = t.dir.join("views.viewstest");
    std::fs::write(&path, "x\n").unwrap();
    t.editor.update(t.cx, |e, cx| e.open_paths(vec![path], cx));
    t.keys("H");
    assert_eq!(t.text(), "x\n", "nothing in the code");
    open_pages(&mut t);
    assert_eq!(t.read(|e| e.view_now()), PAGES.kind(), "a tab of its pages is its view of them");
    let top = pages_top(&mut t);
    t.keys("j");
    assert_eq!(pages_top(&mut t), top + 40., "the view's own keys");
    t.keys("H");
    assert!(pages_top(&mut t) > top + 80., "and the plugin's, an editor command by name");
    t.command("bp");
    show_goals(&mut t);
    t.keys("space g K");
    t.cx.run_until_parked();
    assert_eq!(t.status().as_deref(), Some("views-test: ran in its view"), "and its own command, beside the file");
}

/// Your keys in a plugin's view, by its name, go over what it shows and the plugin's.
#[gpui::test]
fn your_keys_go_over_a_plugins_in_its_view(cx: &mut TestAppContext) {
    let mut t = with_views_plugin("x\n", cx);
    let path = t.dir.join("yours.viewstest");
    std::fs::write(&path, "x\n").unwrap();
    t.editor.update(t.cx, |e, cx| e.open_paths(vec![path], cx));
    let yours = [("H", "scroll_up"), ("J", "scroll_page_down"), ("j", "no_op")].map(|(k, v)| (k.to_string(), v.to_string()));
    let (keymap, wrong) = ghostwrite_core::Keymap::default().with_views(&[PAGES, GOALS]).with_yours_in(PAGES.kind(), &yours);
    assert!(wrong.is_empty(), "{wrong:?}");
    let keymap: &'static ghostwrite_core::Keymap = Box::leak(Box::new(keymap));
    t.editor.update(t.cx, |e, _| e.use_keys(keymap, &[]));
    open_pages(&mut t);
    let top = pages_top(&mut t);
    t.keys("j");
    assert_eq!(pages_top(&mut t), top, "j unbound there");
    t.keys("J");
    let down = pages_top(&mut t);
    assert!(down > top + 80., "J a screen down");
    t.keys("H");
    assert_eq!(pages_top(&mut t), down - 40., "and H yours, not the plugin's");
    t.keys("space v");
    assert_eq!(t.at().0, ViewKind::Code, "its key goes back to the file");
}

/// `focus_view` and a view's name, bound to a key of yours, goes there as its own key does.
#[gpui::test]
fn focus_view_is_a_key_of_yours_for_a_view(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let yours = [("space G", "focus_view goals"), ("space g", "no_op")].map(|(k, v)| (k.to_string(), v.to_string()));
    let (keymap, wrong) = ghostwrite_core::Keymap::default().with_views(&[PAGES, GOALS]).with_yours(&yours);
    assert!(wrong.is_empty(), "{wrong:?}");
    let keymap: &'static ghostwrite_core::Keymap = Box::leak(Box::new(keymap));
    t.editor.update(t.cx, |e, _| e.use_keys(keymap, &[]));
    show_goals(&mut t);
    t.keys("space g");
    assert_eq!(t.at().0, ViewKind::Code, "its own key, unbound");
    t.keys("space G");
    assert_eq!(t.at().0, GOALS.kind(), "yours");
    t.keys("space G");
    assert_eq!(t.at().0, ViewKind::Code, "and back");
    let sheet = crate::editor::view::cheatsheet(keymap);
    assert!(sheet.iter().any(|(title, _)| title == "Goals  space G"), "the cheatsheet calls it by its title and your key");
}

/// A view whose key is the editor's, or another plugin's, is said at the start, with the
/// rest that's wrong with the keys, and in `:plugins`.
#[gpui::test]
fn a_views_key_thats_taken_is_said_at_the_start_and_in_the_list(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let taken = PluginView { key: Some("space f"), ..GOALS };
    let rival = PluginView { id: ViewId { plugin: "rival-test", name: "proofs", shows: Shows::Blocks }, ..GOALS };
    t.editor.update(t.cx, |e, _| e.make_keys(&[PAGES, taken], true));
    let said = t.status().unwrap_or_default();
    assert!(said.starts_with("plugins' keys: views-test's goals view has no key: space f is the editor's (file_picker)"), "{said}");
    t.keys("space f");
    assert!(t.read(|e| e.picker.is_some()), "the editor's comes first");
    t.keys("escape");
    t.editor.update(t.cx, |e, _| e.make_keys(&[GOALS, rival], true));
    let said = t.status().unwrap_or_default();
    assert!(said.contains("rival-test's proofs view has no key: space g is views-test's goals view's"), "the plugin installed first has it: {said}");
    t.command("plugins");
    let listed = t.status().unwrap_or_default();
    assert!(listed.contains("Keys taken: rival-test's proofs view has no key"), "{listed}");
}

#[gpui::test]
fn the_cheatsheet_has_each_views_keys_and_every_section(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let sheet = crate::editor::view::cheatsheet(t.read(|e| e.modal.keymap()));
    let section = |title: &str| sheet.iter().find(|(t, _)| t == title).map(|(_, rows)| rows.iter().map(|row| (row.view, row.keys.clone())).collect::<Vec<_>>());
    let files = section("Files  space e").expect("Files has a section");
    assert!(files.contains(&(ViewKind::Files, "j down k up".into())), "{files:?}");
    assert!(files.contains(&(ViewKind::Files, "a r d".into())), "{files:?}");
    let terminal = section("Terminal  space t").expect("so does the terminal");
    assert!(terminal.contains(&(ViewKind::Terminal, "enter".into())), "{terminal:?}");
    assert!(terminal.iter().any(|(_, keys)| keys.contains("ctrl-\\ ctrl-n")), "{terminal:?}");
    let debugger = section("Debugger  space x").expect("and the debugger");
    assert!(debugger.contains(&(ViewKind::Debugger, "esc".into())), "{debugger:?}");
    let pages = section("Pages  space v").expect("a plugin's view, by its title and key");
    assert!(pages.iter().any(|(view, keys)| matches!(view, ViewKind::Plugin(id) if id.shows == Shows::Pages) && keys == "+ - ="), "{pages:?}");
    assert!(sheet.iter().any(|(title, _)| title.ends_with("  space g")), "and the other plugin's");
    assert!(section("Window  ⌘").is_some());
    let views = section("Views and panes").expect("going from view to view, and the panes");
    for keys in ["space e", "space t", "space x", "space v", "space g", "space w v", "space w w", "space w q  space w o", "click"] {
        assert!(views.iter().any(|(_, shown)| shown == keys), "{keys}: {views:?}");
    }
    assert!(sheet.iter().flat_map(|(_, rows)| rows).all(|row| !row.keys.starts_with("ctrl-w") || row.keys.contains('⌫')), "no ctrl-w but typing's");
    // Every section has a place in the columns, in order, the plugins' views' after the editor's.
    let titles: Vec<String> = sheet.iter().map(|(title, _)| title.clone()).collect();
    let at = |title: &str| titles.iter().position(|t| t == title);
    assert!(at("Debugger  space x") < at("Pages  space v") && at("Pages  space v") < at("Reviewing"), "{titles:?}");
    let columns = crate::editor::view::columns(sheet, 3);
    assert_eq!(columns.len(), 3);
    let placed: Vec<String> = columns.into_iter().flatten().map(|(title, _)| title).collect();
    assert_eq!(placed, titles);
    // Drawn, too.
    t.keys("space ?");
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    assert!(t.read(|e| e.cheatsheet));
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
    t.keys("space e");
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
