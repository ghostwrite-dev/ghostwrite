//! The editor driven by keystrokes, headless: modal keys, the `:` line,
//! tree mode, stories with a scripted Claude, and tours.

use std::path::PathBuf;

use ghostwrite_ai::Backend;
use ghostwrite_core::Mode;
use ghostwrite_project::{fs, scratch_dir, store};
use gpui::{Entity, TestAppContext, VisualTestContext, WindowAppearance};

use super::{Editor, Options};

struct Setup<'a> {
    editor: Entity<Editor>,
    cx: &'a mut VisualTestContext,
    dir: PathBuf,
}

/// A project folder with `doc.txt` holding `text`, open in an editor.
fn open<'a>(text: &str, replies: &[&str], cx: &'a mut TestAppContext) -> Setup<'a> {
    store::redirect_for_tests();
    fs::delete_instead_of_trash();
    let dir = scratch_dir("editor");
    let path = dir.join("doc.txt");
    std::fs::write(&path, text).unwrap();
    cx.update(|cx| {
        crate::theme::install(WindowAppearance::Light, cx);
        crate::ui::text_field::bind_keys(cx);
    });
    let backend = (!replies.is_empty()).then(|| Backend::Scripted(replies.iter().map(|r| r.to_string()).collect()));
    let root = dir.clone();
    let (editor, cx) = cx.add_window_view(|window, cx| {
        let mut editor = Editor::new(root, Options { language_servers: false, backend }, window, cx);
        editor.open_paths(vec![path], cx);
        editor
    });
    cx.run_until_parked();
    Setup { editor, cx, dir }
}

impl Setup<'_> {
    fn keys(&mut self, keys: &str) -> &mut Self {
        self.cx.simulate_keystrokes(keys);
        self.cx.run_until_parked();
        self
    }

    fn input(&mut self, text: &str) -> &mut Self {
        self.cx.simulate_input(text);
        self.cx.run_until_parked();
        self
    }

    /// `:` and a command, then Enter.
    fn command(&mut self, line: &str) -> &mut Self {
        self.keys(":").input(line).keys("enter")
    }

    fn read<R>(&mut self, f: impl FnOnce(&Editor) -> R) -> R {
        self.editor.read_with(self.cx, |e, _| f(e))
    }

    fn text(&mut self) -> String {
        self.read(|e| e.tab().map(|t| t.doc.text().to_string()).unwrap_or_default())
    }

    fn status(&mut self) -> Option<String> {
        self.read(|e| e.project.status.as_ref().map(|s| s.to_string()))
    }

    fn active(&mut self) -> Option<String> {
        let dir = self.dir.clone();
        self.read(|e| e.project.active_path().map(|p| p.strip_prefix(&dir).unwrap().display().to_string()))
    }

    fn cursor(&mut self) -> usize {
        self.read(|e| e.tab().map_or(0, |t| t.view.selection.head))
    }

    fn reviewing(&mut self) -> bool {
        self.read(|e| e.story.is_some())
    }

    fn current(&mut self) -> Option<(usize, usize)> {
        self.read(|e| e.story.as_ref()?.story.as_ref()?.current()).map(|id| (id.reply, id.index))
    }

    /// `space i`, a request, Enter, and the scripted reply arrives.
    fn ask(&mut self, what: &str) -> &mut Self {
        self.keys("space i").input(what).keys("enter")
    }

    fn file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap_or_default()
    }
}

#[gpui::test]
fn modal_keys_edit_and_undo(cx: &mut TestAppContext) {
    let mut t = open("foo bar baz\n", &[], cx);
    assert_eq!(t.keys("w d").text(), "bar baz\n");
    assert_eq!(t.keys("u").text(), "foo bar baz\n");
    // Letters in normal mode never reach the text.
    assert_eq!(t.keys("z q").text(), "foo bar baz\n");
}

#[gpui::test]
fn typing_in_insert_mode(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("o").input("new").keys("escape");
    assert_eq!(t.text(), "one\nnew\ntwo\n");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
    t.keys("u");
    assert_eq!(t.text(), "one\ntwo\n", "the insert is one undo step");
}

#[gpui::test]
fn command_line_opens_writes_and_closes(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\nthree\n", &[], cx);
    std::fs::write(t.dir.join("other.txt"), "other\n").unwrap();
    // Tab completes the path; Enter opens it in a new tab.
    t.keys(":").input("e oth").keys("tab enter");
    assert_eq!(t.active().as_deref(), Some("other.txt"));
    // Edit, refuse to close the tab with changes, then write and close it.
    t.keys("d").command("bc");
    assert_eq!(t.read(|e| e.project.tabs.len()), 2);
    assert!(t.status().is_some_and(|s| s.contains("unsaved")));
    t.command("w").command("bc");
    assert_eq!(t.file("other.txt"), "ther\n");
    assert_eq!(t.read(|e| e.project.tabs.len()), 1);
    // `:3` goes to the third line; Esc leaves the prompt without running it.
    t.command("3").keys("d");
    assert_eq!(t.text(), "one\ntwo\nhree\n");
    t.keys(":").input("bc").keys("escape");
    assert!(t.read(|e| e.prompt.is_none()));
    assert_eq!(t.read(|e| e.project.tabs.len()), 1);
}

#[gpui::test]
fn up_and_down_walk_the_command_history(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.editor.update(t.cx, |e, _| e.history.clear());
    t.command("n").command("bp");
    let typed = |t: &mut Setup| t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| f.read_with(t.cx, |f, _| f.text().to_string()));
    t.keys(": up");
    assert_eq!(typed(&mut t).as_deref(), Some("bp"));
    t.keys("up");
    assert_eq!(typed(&mut t).as_deref(), Some("n"));
    t.keys("down down");
    assert_eq!(typed(&mut t).as_deref(), Some(""));
}

#[gpui::test]
fn rm_trashes_the_current_file(cx: &mut TestAppContext) {
    let mut t = open("keep\n", &[], cx);
    std::fs::write(t.dir.join("junk.txt"), "junk\n").unwrap();
    t.command("e junk.txt").keys("d").command("rm");
    assert!(t.dir.join("junk.txt").exists(), "unsaved changes refuse :rm");
    t.command("rm!");
    assert!(!t.dir.join("junk.txt").exists());
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
}

#[gpui::test]
fn leaving_a_file_writes_it(cx: &mut TestAppContext) {
    let mut t = open("one\n", &[], cx);
    std::fs::write(t.dir.join("second.txt"), "two\n").unwrap();
    t.keys("d");
    assert_eq!(t.file("doc.txt"), "one\n", "not saved while you're in it");
    t.command("e second.txt");
    assert_eq!(t.file("doc.txt"), "ne\n", "written when you leave");
}

#[gpui::test]
fn question_mark_shows_the_cheatsheet_and_any_key_closes_it(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("?");
    assert!(t.read(|e| e.cheatsheet));
    t.keys("j");
    assert!(!t.read(|e| e.cheatsheet));
    assert_eq!(t.cursor(), 0, "the closing key does nothing else");
}

#[gpui::test]
fn tree_mode_drives_the_file_tree(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    // Rows: src/, doc.txt (the active file, under the cursor).
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
    t.keys("k l");
    assert_eq!(t.read(|e| e.project.tree.rows().len()), 3, "l opens the folder");
    t.keys("j l");
    assert_eq!(t.active().as_deref(), Some("src/main.rs"), "l on a file previews it");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree, "and the keys stay in the tree");
    t.keys("j enter");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "enter opens it and leaves the tree");
    t.keys("space e escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
    t.keys("f");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree, "f opens the tree");
    t.keys("f");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "and f closes it");
}

#[gpui::test]
fn tree_mode_adds_and_renames(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.keys("space e a").input("notes/todo.md").keys("enter");
    assert!(t.dir.join("notes/todo.md").is_file());
    assert_eq!(t.active().as_deref(), Some("notes/todo.md"), "a new file opens");
    t.keys("space e r").keys("cmd-a").input("done.md").keys("enter");
    assert!(t.dir.join("notes/done.md").is_file());
    assert_eq!(t.active().as_deref(), Some("notes/done.md"), "the tab follows the rename");
}

const ADD: &str = "def add(a, b):\n    pass\n";
const TWO_STEPS: &str = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n<step>\n<old>\ndef add(a, b):\n</old>\n<new>\ndef add(a: int, b: int) -> int:\n</new>\n</step>\n";

#[gpui::test]
fn steps_are_reviewed_and_the_last_tab_finishes(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[TWO_STEPS], cx);
    t.keys("j space i").input("implement it");
    assert_eq!(t.text(), ADD, "typing in the prompt doesn't drive normal mode");
    t.keys("enter");
    assert_eq!(t.text(), ADD, "nothing changes before accepting");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    assert_eq!(t.current(), Some((0, 1)));
    t.keys("shift-tab");
    assert_eq!(t.current(), Some((0, 1)), "a story only goes forward");
    // Skipping the last step finishes.
    t.keys("backspace");
    assert!(!t.reviewing());
    assert_eq!(t.status().as_deref(), Some("applied 1 of 2 steps"));
    t.keys("u");
    assert_eq!(t.text(), ADD, "each accepted step is its own undo");
}

#[gpui::test]
fn escape_stops_early_and_keeps_what_was_accepted(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[TWO_STEPS], cx);
    t.ask("types and body").keys("tab escape");
    assert!(!t.reviewing());
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_plain_answer_is_read_through(cx: &mut TestAppContext) {
    let mut t = open(ADD, &["<answer>\nIt adds two numbers.\n\nIt doesn't check types.\n</answer>\n"], cx);
    t.ask("what does this do");
    assert_eq!(t.current(), Some((0, 0)));
    t.keys("tab");
    assert_eq!(t.current(), Some((0, 1)), "each paragraph is a note");
    t.keys("tab");
    assert!(!t.reviewing(), "the last Tab finishes");
    assert_eq!(t.text(), ADD, "answers never edit");
}

#[gpui::test]
fn answer_notes_jump_through_the_code(cx: &mut TestAppContext) {
    let mut text = String::from("def first():\n    return 1\n");
    text.extend((0..120).map(|i| format!("# filler {i}\n")));
    text.push_str("def second():\n    return first() + 1\n");
    let reply = "<answer>\n<note>\n<at>\ndef first():\n</at>\nfirst returns one.\n</note>\n<note>\n<at>\n    return first() + 1\n</at>\nsecond builds on it.\n</note>\n</answer>\n";
    let mut t = open(&text, &[reply], cx);
    t.ask("how do these relate");
    assert_eq!(t.cursor(), 0, "the first note is about `def first`");
    t.keys("tab");
    assert_eq!(t.cursor(), text.find("    return first() + 1").unwrap(), "Tab jumps to the second note's line");
}

#[gpui::test]
fn a_correction_replaces_the_plan(cx: &mut TestAppContext) {
    let first = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n";
    let second = "<rest>\n<step>\n<old>\n    pass\n</old>\n<new>\n    return sum((a, b))\n</new>\n</step>\n</rest>\n";
    let mut t = open(ADD, &[first, second], cx);
    t.ask("implement it").keys("enter").input("use sum()");
    assert_eq!(t.text(), ADD, "typing the redirect doesn't edit");
    t.keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return sum((a, b))\n");
    assert!(!t.reviewing());
}

#[gpui::test]
fn escape_leaves_a_redirect_but_keeps_reviewing(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[TWO_STEPS], cx);
    t.ask("implement it").keys("enter").input("never mind").keys("escape");
    assert!(t.reviewing() && t.read(|e| e.prompt.is_none()));
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_story_can_create_a_file_and_write_into_it(cx: &mut TestAppContext) {
    let story = "<create>\nsrc/util.rs\n</create>\n<step>\n<file>\nsrc/util.rs\n</file>\n<old>\n</old>\n<new>\npub fn one() -> u32 { 1 }\n</new>\n</step>\n<step>\n<old>\nfn main() {}\n</old>\n<new>\nmod util;\nfn main() {}\n</new>\n</step>\n";
    let mut t = open("fn main() {}\n", &[story], cx);
    t.ask("move a helper into util");
    assert!(!t.dir.join("src/util.rs").exists(), "nothing is created before Tab");
    assert!(t.read(|e| e.project.tree.ghost().is_some()), "the new file shows in the tree");
    t.keys("tab");
    assert!(t.dir.join("src/util.rs").is_file(), "Tab creates it, folders included");
    assert_eq!(t.active().as_deref(), Some("src/util.rs"), "and opens it");
    t.keys("tab");
    let util = t.dir.join("src/util.rs");
    let util_text = t.read(|e| e.project.tabs.text_of(&util).map(str::to_string));
    assert_eq!(util_text.as_deref(), Some("pub fn one() -> u32 { 1 }\n"), "the next step writes into it");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "then back to the first file for its step");
    t.keys("tab");
    assert_eq!(t.text(), "mod util;\nfn main() {}\n");
    assert_eq!(t.status().as_deref(), Some("applied 3 of 3 steps"));
}

#[gpui::test]
fn a_story_can_remove_and_rename_files(cx: &mut TestAppContext) {
    let story = "<remove>\nold.txt\n</remove>\n<rename>\ndoc.txt\nnotes/renamed.txt\n</rename>\n<step>\n<file>\nnotes/renamed.txt\n</file>\n<old>\none\n</old>\n<new>\nuno\n</new>\n</step>\n";
    let mut t = open("one\n", &[story], cx);
    std::fs::write(t.dir.join("old.txt"), "stale\n").unwrap();
    t.ask("tidy up");
    assert!(t.dir.join("old.txt").exists(), "nothing is removed before Tab");
    t.keys("tab");
    assert!(!t.dir.join("old.txt").exists());
    t.keys("tab");
    assert!(!t.dir.join("doc.txt").exists() && t.dir.join("notes/renamed.txt").is_file(), "renamed, folder made");
    assert_eq!(t.active().as_deref(), Some("notes/renamed.txt"), "the tab followed");
    t.keys("tab");
    assert_eq!(t.text(), "uno\n");
}

#[gpui::test]
fn an_unnamed_step_is_found_in_its_project_file(cx: &mut TestAppContext) {
    let mut t = open("y = 0\n", &["<step>\n<old>\nx = 1\n</old>\n<new>\nx = 2\n</new>\n</step>\n"], cx);
    std::fs::write(t.dir.join("other.py"), "x = 1\nprint(x)\n").unwrap();
    t.ask("bump x");
    assert_eq!(t.active().as_deref(), Some("other.py"), "it goes to the file with those lines");
    t.keys("tab");
    assert_eq!(t.text(), "x = 2\nprint(x)\n");
}

#[gpui::test]
fn a_finished_story_is_remembered(cx: &mut TestAppContext) {
    let reply = "<answer>\nIt adds.\n</answer>\n<memory>\nThe user is learning Python.\n</memory>\n";
    let mut t = open(ADD, &[reply], cx);
    t.ask("what is this").keys("tab");
    assert!(!t.reviewing());
    assert_eq!(store::load_memory(&t.dir), "The user is learning Python.");
    let logged = store::recent_interactions(&t.dir, 5);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["request"], "what is this");
    assert_eq!(logged[0]["outcome"], "answered");
}

#[gpui::test]
fn docs_notes_leave_the_cursor_alone(cx: &mut TestAppContext) {
    let mut t = open("def f():\n    return compute(1, 2)\n", &["<answer>\nok\n</answer>"], cx);
    t.keys("j w w");
    let before = t.cursor();
    t.editor.update(t.cx, |e, cx| e.show_note("compute(a, b): adds", cx));
    t.cx.run_until_parked();
    assert!(t.reviewing(), "the docs show");
    assert_eq!(t.cursor(), before, "and the cursor stays put");
    t.keys("tab");
    assert!(!t.reviewing());
}

fn run_sh(t: &mut Setup, command: &str) {
    t.command(&format!("sh {command}"));
    for _ in 0..200 {
        if t.read(|e| e.tour.as_ref().is_none_or(|state| !state.tour.running())) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        t.cx.run_until_parked();
    }
    panic!("`{command}` never finished");
}

#[gpui::test]
fn sh_errors_are_stops_across_files(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("a.py"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(t.dir.join("b.py"), "uno\n").unwrap();
    run_sh(&mut t, "printf 'error: bad\\n  --> a.py:2:1\\n\\nerror: worse\\n --> b.py:1\\n'; exit 1");
    assert_eq!(t.active().as_deref(), Some("a.py"), "the first error opens its file");
    assert_eq!(t.cursor(), 4, "at its line");
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("b.py"));
    t.keys("shift-tab");
    assert_eq!(t.active().as_deref(), Some("a.py"));
    t.keys("tab tab");
    assert!(t.read(|e| e.tour.is_none()));
    assert!(t.status().is_some_and(|s| s.ends_with("✗ exit 1, 2 locations")));
}

#[gpui::test]
fn enter_hands_an_error_to_claude_then_moves_on(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["<step>\n<old>\ntwo\n</old>\n<new>\n2\n</new>\n</step>\n"], cx);
    std::fs::write(t.dir.join("a.py"), "one\ntwo\n").unwrap();
    std::fs::write(t.dir.join("b.py"), "uno\n").unwrap();
    run_sh(&mut t, "printf 'error: bad\\n  --> a.py:2\\n\\nerror: worse\\n --> b.py:1\\n'");
    t.keys("enter");
    let typed = t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| f.read_with(t.cx, |f, _| f.text().to_string()));
    assert_eq!(typed.as_deref(), Some("fix this"));
    t.keys("enter").keys("tab");
    assert_eq!(t.file("a.py"), "one\n2\n", "moving on to b.py writes a.py");
    assert_eq!(t.active().as_deref(), Some("b.py"), "Claude is done: on to the next error");
}

#[gpui::test]
fn space_slash_tours_the_projects_matches(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("let total = 1;\n", &[], cx);
    std::fs::write(t.dir.join("b.txt"), "nothing\nprint(total)\n").unwrap();
    let selected = |t: &mut Setup| t.read(|e| e.tab().map(|t| t.doc.text()[t.view.selection.bytes()].to_string()));
    t.keys("space /").input("TOTAL").keys("enter");
    t.cx.run_until_parked();
    assert_eq!(t.status().as_deref(), Some("no matches for TOTAL"), "a capital makes it exact");
    t.keys("space /").input("total").keys("enter");
    t.cx.run_until_parked();
    assert_eq!((t.active(), selected(&mut t)), (Some("b.txt".into()), Some("total".into())), "files in order; the match is selected");
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("tab");
    assert!(t.read(|e| e.tour.is_none()), "the last Tab finishes");
}

#[gpui::test]
fn a_tour_walks_through_other_files(cx: &mut TestAppContext) {
    let tour = "<answer>\n<note>\n<at>\nfn main() { run(); }\n</at>\nIt starts here.\n</note>\n<note>\n<file>\nsrc/lib.rs\n</file>\n<at>\npub fn run() {\n</at>\nThe work happens in run.\n</note>\n<note>\n<at>\n    helper();\n</at>\nWhich calls the helper, found by its lines.\n</note>\n</answer>\n";
    let mut t = open("fn main() { run(); }\n", &[tour], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {\n    helper();\n}\n").unwrap();
    t.ask("give me a tour");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "a note names its file");
    assert_eq!(t.cursor(), 0);
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "an unnamed note is found where its lines are");
    assert_eq!(t.cursor(), "pub fn run() {\n".len());
    t.keys("tab");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_change_edits_other_existing_files(cx: &mut TestAppContext) {
    let story = "<step>\n<file>\nsrc/lib.rs\n</file>\n<old>\npub fn run() {}\n</old>\n<new>\npub fn run(verbose: bool) {}\n</new>\n</step>\n<step>\n<old>\nfn main() { run(); }\n</old>\n<new>\nfn main() { run(true); }\n</new>\n</step>\n<step>\n<file>\nsrc/new.rs\n</file>\n<old>\n</old>\n<new>\n// made without a <create>\n</new>\n</step>\n";
    let mut t = open("fn main() { run(); }\n", &[story], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    // Unsaved changes elsewhere are written before Claude reads the project.
    t.command("e src/lib.rs").keys("x y p").command("e doc.txt").keys("u");
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "the first step is in another file");
    t.keys("tab");
    assert_eq!(t.read(|e| e.project.tabs.text_of(&e.project.root.join("src/lib.rs")).map(str::to_string)).as_deref(), Some("pub fn run(verbose: bool) {}\npub fn run() {}\n"));
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "back for the next step");
    t.keys("tab");
    assert_eq!(t.text(), "fn main() { run(true); }\n");
    assert!(t.read(|e| e.project.tree.ghost().is_some()), "a file that isn't there yet shows in the tree");
    t.keys("tab");
    assert_eq!(t.file("src/new.rs"), "", "made on Tab, written into after");
    assert_eq!(t.read(|e| e.project.tabs.text_of(&e.project.root.join("src/new.rs")).map(str::to_string)).as_deref(), Some("// made without a <create>\n"));
}

#[gpui::test]
fn ai_mode_reviews_and_goes_back_to_where_you_were(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[TWO_STEPS], cx);
    // From select mode: the selection is the context, and comes back after.
    t.keys("v j");
    let selected = t.read(|e| e.tab().unwrap().view.selection);
    t.keys("space i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter");
    // Looking around works; editing doesn't.
    t.keys("g g d x");
    assert_eq!(t.text(), ADD, "no edits in AI mode");
    t.keys("escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Select, "back to select mode");
    assert_eq!(t.read(|e| e.tab().unwrap().view.selection), selected, "with the selection");
}

#[gpui::test]
fn ai_mode_from_the_tree_goes_back_to_the_tree(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[TWO_STEPS], cx);
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
    t.keys("space i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    t.keys("backspace");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
}

#[gpui::test]
fn fixing_a_tours_error_goes_ai_then_tour_then_home(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["<step>\n<old>\ntwo\n</old>\n<new>\n2\n</new>\n</step>\n"], cx);
    std::fs::write(t.dir.join("a.py"), "one\ntwo\n").unwrap();
    std::fs::write(t.dir.join("b.py"), "uno\n").unwrap();
    run_sh(&mut t, "printf 'error: bad\\n  --> a.py:2\\n\\nerror: worse\\n --> b.py:1\\n'");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour);
    t.keys("enter");
    t.keys("enter");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour, "the fix is in: back to the tour, at the next error");
    assert_eq!(t.active().as_deref(), Some("b.py"));
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "the tour is over");
}

#[gpui::test]
fn a_question_mid_tour_comes_back_to_the_same_stop(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["<answer>\nIt prints lines.\n</answer>\n"], cx);
    run_sh(&mut t, "echo one; echo two");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour);
    // Enter on a page of output asks about it.
    t.keys("enter");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("what is this?").keys("enter");
    assert!(t.read(|e| e.story.as_ref().is_some_and(|s| s.origin.output().is_some_and(|o| o.contains("one\ntwo")))), "with the output");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour, "back on the tour");
    // `space i` asks about the code, and the tour waits.
    t.keys("space i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("and this?").keys("enter").keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour, "still at the same stop");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
}

#[gpui::test]
fn a_follow_up_is_answered_where_it_was_asked(cx: &mut TestAppContext) {
    let plan = "<step>\n<file>\nsrc/lib.rs\n</file>\n<old>\npub fn run() {}\n</old>\n<new>\npub fn run(verbose: bool) {}\n</new>\n</step>\n";
    let mut t = open("fn main() { run(); }\n", &[plan, "<answer>\nSo callers can ask for detail.\n</answer>\n"], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "\n\npub fn run() {}\n").unwrap();
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"));
    // Asked about the step in lib.rs: the answer shows there, under it.
    t.keys("enter").input("why?").keys("enter");
    let at = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let story = state.story.as_ref().unwrap();
        let located = state.locate(&e.project, story.current().unwrap()).unwrap();
        (located.tab.map(|ix| e.project.tabs.get(ix).unwrap().doc.name().to_string()), located.near)
    });
    assert_eq!(at, (Some("lib.rs".into()), 2..3), "under the step's line, not the first file's");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "and the view stays there");
}

#[gpui::test]
fn a_question_about_output_is_answered_in_the_panel(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &["<answer>\nIt prints two lines.\n</answer>\n"], cx);
    run_sh(&mut t, "echo one; echo two");
    t.keys("enter");
    assert!(t.read(|e| e.story.as_ref().is_some_and(|s| s.origin.in_panel())), "asked from the panel");
    t.input("what is this?").keys("enter");
    let in_panel = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let story = state.story.as_ref().unwrap();
        state.spot(&e.project, story.current().unwrap()) == Some(ghostwrite_ai::Spot::Panel)
    });
    assert!(in_panel, "the answer shows in the panel");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "nothing jumps");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tour);
}

#[gpui::test]
fn a_comment_changes_what_comes_next(cx: &mut TestAppContext) {
    let tour = "<answer>\n<note>\n<at>\ndef add(a, b):\n</at>\nIt adds.\n</note>\n<note>\n<at>\n    pass\n</at>\nIt does nothing yet.\n</note>\n</answer>\n";
    let revised = "<answer>\n<note>\nRight, types it is.\n</note>\n</answer>\n<rest>\n<step>\n<old>\ndef add(a, b):\n</old>\n<new>\ndef add(a: int, b: int):\n</new>\n</step>\n</rest>\n";
    let mut t = open(ADD, &[tour, revised], cx);
    t.ask("walk me through this");
    t.keys("enter").input("I care about the types, not the body").keys("enter");
    let notes = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().items().len());
    assert_eq!(notes, 3, "the note, the answer, and the revised rest: the old second note is gone");
    t.keys("tab tab");
    assert_eq!(t.text(), "def add(a: int, b: int):\n    pass\n", "the new step came next");
    assert!(!t.reviewing());
}

#[gpui::test]
fn the_tree_marks_where_the_story_is_waiting(cx: &mut TestAppContext) {
    let story = "<step>\n<file>\nsrc/lib.rs\n</file>\n<old>\npub fn run() {}\n</old>\n<new>\npub fn run(verbose: bool) {}\n</new>\n</step>\n<step>\n<old>\nfn main() { run(); }\n</old>\n<new>\nfn main() { run(true); }\n</new>\n</step>\n";
    let mut t = open("fn main() { run(); }\n", &[story], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let waiting = |t: &mut Setup| {
        let dir = t.dir.clone();
        let mut files: Vec<(String, super::story::Waiting)> =
            t.read(|e| e.story.as_ref().map(|s| s.files(&e.project)).unwrap_or_default()).into_iter().map(|(p, w)| (p.strip_prefix(&dir).unwrap().display().to_string(), w)).collect();
        files.sort();
        files
    };
    t.keys("space i");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), super::story::Waiting::Here)], "asking about this file");
    t.input("add a verbose flag").keys("enter");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), super::story::Waiting::Later), ("src/lib.rs".into(), super::story::Waiting::Here)]);
    t.keys("tab");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), super::story::Waiting::Here)], "lib.rs is done");
}
