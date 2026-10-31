//! The editor driven by keystrokes, headless: modal keys, the `:` line,
//! tree mode, and stories (with a scripted model, or given by the editor).

use std::path::PathBuf;

use ghostwrite_ai::Backend;
use ghostwrite_core::Mode;
use ghostwrite_project::tree::Proposal;
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
    open_with(text, replies, false, cx)
}

/// [`open`], with the language servers running or not.
fn open_with<'a>(text: &str, replies: &[&str], language_servers: bool, cx: &'a mut TestAppContext) -> Setup<'a> {
    store::redirect_for_tests();
    fs::delete_instead_of_trash();
    let dir = scratch_dir("editor");
    let path = dir.join("doc.txt");
    std::fs::write(&path, text).unwrap();
    cx.update(|cx| {
        crate::theme::install(WindowAppearance::Light, cx);
        crate::ui::text_field::bind_keys(cx);
    });
    let backends = (!replies.is_empty()).then(|| Backend::Scripted(replies.iter().map(|r| r.to_string()).collect())).into_iter().collect();
    let root = dir.clone();
    let (editor, cx) = cx.add_window_view(|window, cx| {
        let mut editor = Editor::new(root, Options { language_servers, previews: false, backends }, window, cx);
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

    /// Which reply the item under review is.
    fn current(&mut self) -> Option<usize> {
        self.read(|e| e.story.as_ref()?.story.as_ref()?.current()).map(|id| id.0)
    }

    /// `space i`, a request, Enter, and the scripted reply arrives.
    fn ask(&mut self, what: &str) -> &mut Self {
        self.keys("space a").input(what).keys("enter")
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
fn a_click_moves_the_cursor_and_shift_or_v_extends(cx: &mut TestAppContext) {
    let mut t = open("foo bar baz\n", &[], cx);
    let selected = |t: &mut Setup| t.read(|e| (e.tab().map(|tab| tab.doc.text()[tab.view.selection.bytes()].to_string()).unwrap(), e.modal.mode));
    let editor = t.editor.clone();
    let click = |t: &mut Setup, at: usize, shift: bool, clicks: usize| t.cx.update(|_, cx| editor.update(cx, |e, cx| e.click(at, shift, clicks, cx)));
    click(&mut t, 4, false, 1);
    assert_eq!(selected(&mut t), ("".into(), Mode::Normal), "the cursor, still in normal mode");
    click(&mut t, 9, true, 1);
    assert_eq!(selected(&mut t), ("bar ba".into(), Mode::Normal), "shift selects to the char clicked");
    click(&mut t, 0, false, 1);
    t.keys("v");
    click(&mut t, 2, false, 1);
    assert_eq!(selected(&mut t), ("foo".into(), Mode::Select), "in select mode a click extends");
    click(&mut t, 5, false, 2);
    assert_eq!(selected(&mut t), ("bar".into(), Mode::Select), "a double click is the word");
    t.keys("space e escape");
    click(&mut t, 8, false, 1);
    assert_eq!(selected(&mut t), ("".into(), Mode::Normal), "going to the tree ended the selection");
    click(&mut t, 5, false, 3);
    assert_eq!(selected(&mut t), ("foo bar baz\n".into(), Mode::Normal), "a triple click is the line");
    click(&mut t, 4, false, 1);
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.drag_to(9, cx)));
    assert_eq!(selected(&mut t), ("bar ba".into(), Mode::Normal), "dragging from a click selects to the char under the mouse");
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.drag_to(1, cx)));
    assert_eq!(selected(&mut t), ("oo ".into(), Mode::Normal), "and back before where it was pressed");
}

#[gpui::test]
fn zooming_sizes_everything(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let sizes = |t: &mut Setup| t.cx.update(|_, cx| (crate::theme::theme(cx).zoom, f32::from(crate::theme::theme(cx).rem_size())));
    let zoom = |t: &mut Setup, by| t.cx.update(|_, cx| crate::theme::zoom(by, cx));
    let code = |t: &mut Setup| t.cx.update(|_, cx| f32::from(crate::theme::theme(cx).mono_size));
    let base = code(&mut t);
    zoom(&mut t, 1);
    zoom(&mut t, 1);
    assert_eq!(sizes(&mut t), (1.25, 20.), "the window's rems, which the tree and the bars are sized in");
    assert_eq!(code(&mut t), base * 1.25, "and the code text with them");
    zoom(&mut t, -1);
    t.cx.update(|_, cx| crate::theme::follow(WindowAppearance::Dark, cx));
    assert_eq!(sizes(&mut t).0, 1.1, "switching to dark keeps the zoom");
    (0..20).for_each(|_| zoom(&mut t, -1));
    assert_eq!(sizes(&mut t).0, 0.5, "as far out as it goes");
    zoom(&mut t, 0);
    assert_eq!((sizes(&mut t).0, code(&mut t)), (1.0, base), "back to 100%");
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
fn saving_never_writes_over_what_another_program_changed(cx: &mut TestAppContext) {
    let mut t = open("one\n", &[], cx);
    let path = t.dir.join("doc.txt");
    t.keys("i").input("mine ").keys("escape");
    // Another program writes, and the watcher hasn't said yet.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, "theirs\n").unwrap();
    t.command("w");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", ":w refuses");
    assert!(t.status().is_some_and(|s| s.contains("changed on disk")), "{:?}", t.status());
    t.command("n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n", "leaving the tab doesn't write it either");
    t.command("bp").command("w!");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine one\n", ":w! does");
    t.command("w other.txt");
    assert_eq!(std::fs::read_to_string(t.dir.join("other.txt")).unwrap(), "mine one\n", "a new file is written");
    std::fs::write(t.dir.join("other.txt"), "keep\n").unwrap();
    t.command("w other.txt");
    assert_eq!(std::fs::read_to_string(t.dir.join("other.txt")).unwrap(), "keep\n", "an existing one takes :w!");
}

#[gpui::test]
fn crlf_files_keep_their_line_endings(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("win.txt");
    std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
    t.command("e win.txt");
    assert_eq!(t.text(), "one\ntwo\n");
    t.keys("A").input("!").keys("enter").input("new").keys("escape").command("w");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one!\r\nnew\r\ntwo\r\n");
}

#[gpui::test]
fn rm_stays_in_the_project(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("dir")).unwrap();
    t.command("rm .");
    assert!(t.dir.exists() && t.status().is_some_and(|s| s.contains("isn't in the project")));
    t.command("rm ../");
    assert!(t.status().is_some_and(|s| s.contains("isn't in the project")));
    t.command("rm dir");
    assert!(t.dir.join("dir").exists() && t.status().is_some_and(|s| s.contains(":rm!")), "a folder takes :rm!");
    t.command("rm! dir");
    assert!(!t.dir.join("dir").exists());
}

#[gpui::test]
fn the_same_file_opens_once_however_its_spelled(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/a.rs"), "").unwrap();
    std::os::unix::fs::symlink(t.dir.join("src"), t.dir.join("link")).unwrap();
    t.command("e src/a.rs").command("e src/../src/a.rs").command("e link/a.rs");
    assert_eq!(t.read(|e| e.project.tabs.len()), 2);
}

#[gpui::test]
fn searching_the_file(cx: &mut TestAppContext) {
    let mut t = open("one Two\ntwo three\nTWO\n", &[], cx);
    let selected = |t: &mut Setup| t.read(|e| e.tab().map(|tab| (tab.view.selection.from(), tab.doc.text()[tab.view.selection.bytes()].to_string())).unwrap());
    t.keys("/").input("two");
    assert_eq!(selected(&mut t), (4, "Two".into()), "found as typed, ignoring case without a capital");
    t.keys("enter n");
    assert_eq!(selected(&mut t), (8, "two".into()));
    assert_eq!(t.status().as_deref(), Some("2/3 two"));
    t.keys("n n");
    assert_eq!(selected(&mut t).0, 4, "round the end");
    t.keys("N");
    assert_eq!(selected(&mut t).0, 18, "and back");
    t.keys("/").input("TWO").keys("escape");
    assert_eq!(selected(&mut t).0, 18, "esc goes back to where it began");
    t.keys("/").input("TWO").keys("enter");
    assert_eq!(selected(&mut t), (18, "TWO".into()), "a capital: case matters");
    t.keys("g g j e *");
    assert_eq!(selected(&mut t), (18, "TWO".into()), "* searches for what's selected, from it on");
    t.keys("?").input("one").keys("enter");
    assert_eq!(selected(&mut t).0, 0, "? searches back");
    t.keys("space ?");
    assert!(t.read(|e| e.cheatsheet), "the cheatsheet is space ?");
}

#[gpui::test]
fn ctrl_o_goes_back_where_a_jump_came_from(cx: &mut TestAppContext) {
    let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let mut t = open(&text, &[], cx);
    let line = |t: &mut Setup| t.read(|e| e.tab().map(|tab| tab.doc.buffer.line_of(tab.cursor())).unwrap());
    t.keys("j j G");
    assert_eq!(line(&mut t), 199);
    t.keys("ctrl-o");
    assert_eq!(line(&mut t), 2, "back where G came from");
    t.keys("ctrl-i");
    assert_eq!(line(&mut t), 199, "and forward again");
    t.keys("j k");
    t.keys("ctrl-o ctrl-o");
    assert_eq!(line(&mut t), 2, "small moves aren't jumps");
    t.command("n");
    t.keys("ctrl-o");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "another file is a jump too");
}

#[gpui::test]
fn pickers_open_files_by_a_few_letters(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("src/editor")).unwrap();
    for f in ["src/editor/mod.rs", "src/editor/view.rs", "src/main.rs"] {
        std::fs::write(t.dir.join(f), "").unwrap();
    }
    t.keys("space f");
    for _ in 0..100 {
        t.cx.run_until_parked();
        if t.read(|e| e.picker.as_ref().is_some_and(|p| !p.listing())) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    t.input("edmod").keys("enter");
    assert_eq!(t.active().as_deref(), Some("src/editor/mod.rs"));
    t.keys("space b").input("doc").keys("enter");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "space b: the open ones");
    t.keys("space b down escape");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "esc leaves it be");
    assert!(t.read(|e| e.picker.is_none()));
}

#[gpui::test]
fn colors_are_worked_out_aside_and_follow_edits_meanwhile(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    std::fs::write(t.dir.join("main.rs"), "fn main() {}\n").unwrap();
    t.command("e main.rs");
    let scope_at = |t: &mut Setup, at: usize| {
        t.read(|e| e.tab().unwrap().doc.cached_highlights().iter().find(|s| s.range.contains(&at)).map(|s| ghostwrite_core::syntax::SCOPES[s.scope]))
    };
    assert_eq!(scope_at(&mut t, 0), Some("keyword"), "fn, once worked out");
    t.keys("O").input("// hi").keys("escape");
    assert_eq!(scope_at(&mut t, 0), Some("comment"), "the new line too");
    assert_eq!(scope_at(&mut t, "// hi\n".len()), Some("keyword"), "and fn moved down with its text");
}

#[gpui::test]
fn dot_repeats_the_last_change_and_q_plays_a_macro(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\nthree\n", &[], cx);
    t.keys("A").input("!").keys("escape j .");
    assert_eq!(t.text(), "one!\ntwo!\nthree\n", ". appends again on the next line");
    t.keys("g g Q x d Q");
    assert_eq!(t.text(), "two!\nthree\n");
    t.keys("q");
    assert_eq!(t.text(), "three\n", "q plays what Q recorded");
    t.keys("u u");
    t.keys("g g J");
    assert_eq!(t.text(), "one! two!\nthree\n", "J");
    t.keys("g g i").input("foo bar").keys("alt-backspace escape");
    assert_eq!(t.text(), "foo one! two!\nthree\n", "option-backspace takes the word before");
}

/// What the watcher reports when another program changes `paths`.
#[gpui::test]
fn a_lean_file_types_symbols_from_backslashes(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let path = t.dir.join("Basic.lean");
    std::fs::write(&path, "").unwrap();
    t.editor.update(t.cx, |e, cx| e.open_paths(vec![path], cx));
    t.keys("i");
    for c in "\\forall n : \\N, n \\ge 0".chars() {
        t.input(&c.to_string());
    }
    assert_eq!(t.text(), "∀ n : ℕ, n ≥ 0");
    t.keys("escape").command("e doc.txt").keys("i").input("\\").input("a").input(" ");
    assert_eq!(t.text(), "\\a ", "only in Lean");
}

fn changed_elsewhere(t: &mut Setup, paths: &[&str]) {
    let paths: Vec<PathBuf> = paths.iter().map(|p| t.dir.join(p)).collect();
    t.editor.update(t.cx, |e, cx| e.disk_changed(&paths, cx));
    t.cx.run_until_parked();
}

#[gpui::test]
fn changes_from_other_programs_show_up(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("j l");
    std::fs::write(t.dir.join("doc.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(t.dir.join("new.txt"), "").unwrap();
    changed_elsewhere(&mut t, &["doc.txt", "new.txt"]);
    assert_eq!(t.text(), "one\nTWO\nthree\n", "a clean file reloads");
    assert_eq!(t.cursor(), 5, "where it was in the text that changed");
    t.keys("u");
    assert_eq!(t.text(), "one\ntwo\n", "the reload is an undo step");
    t.keys("U");
    assert!(t.read(|e| e.project.tree.index_of(&e.project.root.join("new.txt")).is_some()), "the tree lists the new file");

    t.keys("x d");
    std::fs::write(t.dir.join("doc.txt"), "theirs\n").unwrap();
    changed_elsewhere(&mut t, &["doc.txt"]);
    assert_eq!(t.text(), "one\nthree\n", "unsaved changes are kept");
    assert!(t.status().unwrap().contains("changed on disk"));
    assert!(t.read(|e| e.tab().unwrap().doc.dirty));
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
fn space_question_mark_shows_the_cheatsheet_and_any_key_closes_it(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("space ?");
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
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree, "space e opens the tree");
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "and closes it");
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

/// Live: needs `tinymist` installed.
#[gpui::test]
#[ignore]
fn a_renamed_file_moves_to_its_new_languages_server(cx: &mut TestAppContext) {
    let mut t = open_with("= Title\n", &[], true, cx);
    let server = |t: &mut Setup| t.read(|e| e.tab().unwrap().doc.lsp.as_ref().map(|l| l.server()));
    assert_eq!(server(&mut t), None, "plain text has no server");
    t.command("w paper.typ");
    assert_eq!(t.active().as_deref(), Some("paper.typ"));
    assert_eq!(server(&mut t), Some("tinymist"), "writing it as .typ starts tinymist");
    t.keys("space e r").keys("cmd-a").input("paper.md").keys("enter");
    assert_eq!(t.active().as_deref(), Some("paper.md"));
    assert_eq!(server(&mut t), None, "renaming it to .md leaves tinymist");
}

/// Live: needs `lake` (elan) installed.
#[gpui::test]
#[ignore]
fn a_lean_file_shows_the_goals_at_the_cursor(cx: &mut TestAppContext) {
    let mut t = open_with("", &[], true, cx);
    let text = "theorem t (n : Nat) : n + 0 = n := by\n  rfl\n\n#eval 6 * 7\n";
    let path = t.dir.join("Main.lean");
    std::fs::write(&path, text).unwrap();
    t.editor.update(t.cx, |e, cx| {
        e.project.previews = true;
        e.open_paths(vec![path], cx);
    });
    let wait_for = |t: &mut Setup, what: &dyn Fn(&Editor) -> bool| {
        for _ in 0..600 {
            t.cx.run_until_parked();
            if t.read(what) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            t.cx.update(|window, _| window.refresh());
        }
        panic!("timed out");
    };
    let goals = |e: &Editor| e.tab().and_then(|t| t.doc.goals.as_ref()?.goals.clone());
    t.keys("j w");
    wait_for(&mut t, &|e| goals(e).is_some());
    assert_eq!(t.read(goals), Some(vec!["n : Nat\n⊢ n + 0 = n".to_string()]));
    t.keys("j j");
    wait_for(&mut t, &|e| e.tab().is_some_and(|t| t.doc.diagnostics.iter().any(|d| d.message == "42")));
}

/// Live: needs `rust-analyzer` installed.
#[gpui::test]
#[ignore]
fn the_language_server_formats_renames_and_lists_symbols(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open_with("", &[], true, cx);
    std::fs::write(t.dir.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/main.rs"), "fn add(a: i32, b: i32) -> i32 {   a + b }\nfn main() { let x = add(1, 2); println!(\"{x}\"); }\n").unwrap();
    t.command("e src/main.rs");
    let wait = |t: &mut Setup, what: &str, done: &dyn Fn(&Editor) -> bool, again: &dyn Fn(&mut Setup)| {
        for round in 0..300 {
            t.cx.run_until_parked();
            if t.read(done) {
                return;
            }
            if round % 25 == 0 {
                again(t);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("timed out waiting for {what}: {}", t.text());
    };
    wait(&mut t, "formatting", &|e| e.tab().unwrap().doc.text().contains("i32 {\n    a + b\n}"), &|t| {
        t.command("format");
    });
    wait(&mut t, "renaming", &|e| e.tab().unwrap().doc.text().matches("plus").count() == 2, &|t| {
        t.keys("escape g g f d space r");
        assert_eq!(
            t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| t.cx.update(|_, cx| f.read(cx).text().to_string())).as_deref(),
            Some("add"),
            "the prompt starts with its name"
        );
        t.keys("cmd-a").input("plus").keys("enter");
    });
    wait(&mut t, "symbols", &|e| e.picker.is_some(), &|t| {
        t.keys("escape space s");
    });
    t.input("main").keys("enter");
    assert_eq!(t.read(|e| e.tab().map(|t| t.doc.buffer.line_of(t.cursor()))), Some(3), "main, picked");
    t.keys("g g f s");
    wait(&mut t, "references", &|e| e.story.is_some(), &|t| {
        t.keys("g r");
    });
    assert!(t.status().is_none_or(|s| !s.contains("no references")));
}

const ADD: &str = "def add(a, b):\n    pass\n";
const DONE: &str = "<done>\n</done>\n";

/// A scripted edit in `file`: `at` becomes `new` (lines joined with `\n`).
fn edit(file: &str, at: &str, new: &str) -> String {
    format!("<file>\n{file}\n</file>\n<at>\n{at}\n</at>\n<say>\nBecause.\n</say>\n<new>\n{new}\n</new>\n")
}

/// A scripted note in `file`, about `at` (empty: the file as a whole).
fn note(file: &str, at: &str, say: &str) -> String {
    let at = if at.is_empty() { String::new() } else { format!("{at}\n") };
    format!("<file>\n{file}\n</file>\n<at>\n{at}</at>\n<say>\n{say}\n</say>\n")
}

fn fix() -> String {
    edit("doc.txt", "    pass", "    return a + b")
}

fn hints() -> String {
    edit("doc.txt", "def add(a, b):", "def add(a: int, b: int) -> int:")
}

#[gpui::test]
fn each_item_is_asked_for_once_the_last_is_decided(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.keys("j space a").input("implement it");
    assert_eq!(t.text(), ADD, "typing in the prompt doesn't drive normal mode");
    t.keys("enter");
    assert_eq!(t.text(), ADD, "nothing changes before accepting");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    assert_eq!(t.current(), Some(1), "the next item came");
    t.keys("shift-tab");
    assert_eq!(t.current(), Some(1), "a story only goes forward");
    // Not that one: saying so asks for more, and <done> finishes.
    t.keys("enter").input("no type hints").keys("enter");
    assert!(!t.reviewing());
    assert_eq!(t.status().as_deref(), Some("applied 1 of 2 steps"));
    t.keys("u");
    assert_eq!(t.text(), ADD, "each accepted edit is its own undo");
}

#[gpui::test]
fn the_next_items_are_asked_for_ahead(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it");
    let wanted = t.read(|e| {
        let story = e.story.as_ref().unwrap().story.as_ref().unwrap();
        (story.wants(0), story.wants(1), story.wants(2))
    });
    assert_eq!(wanted, (true, true, false), "the next item and the one after, up to <done>");
    let item = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().current_item().unwrap());
    let ahead = t.read(|e| e.story.as_ref().unwrap().request_ahead(&e.project, &[item]).unwrap().text);
    assert_eq!(ahead, "def add(a, b):\n    return a + b\n", "what's sent ahead shows the file as if the edit were accepted");
    t.keys("tab");
    assert_eq!(t.current(), Some(1), "accepting brings the next item, asked ahead");
    t.keys("tab");
    assert!(!t.reviewing());
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn escape_stops_early_and_keeps_what_was_accepted(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("types and body").keys("tab escape");
    assert!(t.reviewing(), "one esc doesn't end it");
    t.keys("escape");
    assert!(!t.reviewing());
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn notes_are_read_one_at_a_time(cx: &mut TestAppContext) {
    let first = note("doc.txt", "def add(a, b):", "It adds two numbers.");
    let mut t = open(ADD, &[&first, &note("doc.txt", "    pass", "It doesn't do it yet."), DONE], cx);
    t.ask("what does this do");
    assert_eq!(t.current(), Some(0));
    t.keys("tab");
    assert_eq!(t.current(), Some(1));
    t.keys("tab");
    assert!(!t.reviewing(), "<done> finishes");
    assert_eq!(t.text(), ADD, "notes never edit");
}

/// Live: needs `typst` installed, to make the PDF.
#[gpui::test]
#[ignore]
fn a_pdf_is_read_only_text_claude_leaves_be(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    std::fs::write(t.dir.join("paper.typ"), "#set page(width: 300pt, height: 200pt)\nHello world\n\nSecond line").unwrap();
    let made = std::process::Command::new("typst").arg("compile").arg(t.dir.join("paper.typ")).status().unwrap();
    assert!(made.success());
    let pdf = t.dir.join("paper.pdf");
    let original = std::fs::read(&pdf).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![pdf.clone()], cx)));
    t.cx.run_until_parked();
    assert_eq!(t.active().as_deref(), Some("paper.pdf"));
    assert_eq!(t.text(), "── page 1 ──\nHello world\nSecond line", "its text, after the page's line");

    t.keys("j x d");
    assert_eq!(t.text(), "── page 1 ──\nHello world\nSecond line", "an edit is undone");
    assert_eq!(t.status().as_deref(), Some("a PDF is read-only"));
    t.keys("i");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "no inserting");
    t.command("w");
    assert_eq!(std::fs::read(&pdf).unwrap(), original, "and it's never written");

    t.keys("space a");
    assert!(!t.reviewing(), "Claude doesn't comment on a PDF");
    assert!(t.status().is_some_and(|s| s.contains("doesn't comment on PDFs")), "{:?}", t.status());
}

#[gpui::test]
fn a_typst_files_pages_are_beside_it_and_follow_it(cx: &mut TestAppContext) {
    let mut t = open("", &[], cx);
    let source = "#set page(width: 300pt, height: 200pt)\n= Intro\nThe cat sat on the mat.\n";
    let path = t.dir.join("paper.typ");
    std::fs::write(&path, source).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.project.previews = true;
            e.open_paths(vec![path.clone()], cx);
        })
    });
    let compiled = |t: &mut Setup| {
        t.cx.update(|_, cx| editor.update(cx, |e, cx| e.refresh_preview(cx)));
        t.cx.run_until_parked();
        t.read(|e| {
            let p = e.tab().unwrap().doc.preview.as_ref().unwrap();
            (p.document.as_ref().map(|d| d.pages.len()), p.error.clone(), p.line)
        })
    };
    assert_eq!(compiled(&mut t), (Some(1), None, None), "its page, as it opens, left where it was");
    let cat = source.find("cat").unwrap();
    let glyph = t.read(|e| e.tab().unwrap().doc.preview.as_ref().unwrap().document.as_ref().unwrap().pages[0].glyphs.clone());
    assert!(glyph.iter().any(|g| g.source == Some(cat..cat + 1)), "the c of cat is on the page, from the text");

    t.keys("j j A").input(" Again.").keys("escape");
    assert_eq!(compiled(&mut t), (Some(1), None, Some(2)), "compiled as typed, unsaved; and an edit on another line brings the page to it");
    t.keys("i").input("#oops(").keys("escape");
    let (_, error, _) = compiled(&mut t);
    assert!(error.is_some_and(|e| e.contains("paper.typ:3: error")), "why it doesn't compile, and where");

    t.keys("g g space j");
    assert_eq!(t.read(|e| e.tab().unwrap().doc.preview.as_ref().unwrap().line), Some(0), "space j brings the pages to the cursor");
    assert_eq!(t.status().as_deref(), Some("no problems in this file"), "it has no server here to say");
}

#[gpui::test]
fn notes_jump_through_the_code(cx: &mut TestAppContext) {
    let mut text = String::from("def first():\n    return 1\n");
    text.extend((0..120).map(|i| format!("# filler {i}\n")));
    text.push_str("def second():\n    return first() + 1\n");
    let first = note("doc.txt", "def first():", "first returns one.");
    let second = note("doc.txt", "    return first() + 1", "second builds on it.");
    let mut t = open(&text, &[&first, &second, DONE], cx);
    t.ask("how do these relate");
    assert_eq!(t.cursor(), 0, "the first note is about `def first`");
    t.keys("tab");
    assert_eq!(t.cursor(), text.find("    return first() + 1").unwrap(), "Tab jumps to the second note's line");
}

#[gpui::test]
fn what_you_say_shapes_the_next_item(cx: &mut TestAppContext) {
    let sum = edit("doc.txt", "    pass", "    return sum((a, b))");
    let mut t = open(ADD, &[&fix(), &sum, DONE], cx);
    t.ask("implement it").keys("enter").input("use sum()");
    assert_eq!(t.text(), ADD, "typing the redirect doesn't edit");
    t.keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return sum((a, b))\n");
    assert!(!t.reviewing());
}

#[gpui::test]
fn escape_leaves_a_redirect_but_keeps_reviewing(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it").keys("enter").input("never mind").keys("escape");
    assert!(t.reviewing() && t.read(|e| e.prompt.is_none()));
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_story_can_rename_and_remove_files(cx: &mut TestAppContext) {
    let rename = "<rename>\nsrc/util.rs\nsrc/helpers.rs\n</rename>\n<say>\nA clearer name.\n</say>\n";
    let remove = "<remove>\nold.txt\n</remove>\n<say>\nNothing uses it.\n</say>\n";
    let mut t = open("x\n", &[rename, remove, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/util.rs"), "pub fn one() {}\n").unwrap();
    std::fs::write(t.dir.join("old.txt"), "old\n").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    t.ask("tidy up the files");
    let proposal = t.read(|e| e.project.tree.proposal().cloned());
    assert_eq!(proposal, Some(Proposal::Rename(t.dir.join("src/util.rs"), "src/helpers.rs".into())), "shown in the tree first");
    assert!(t.dir.join("src/util.rs").is_file(), "nothing moves before Tab");
    t.keys("tab");
    assert!(!t.dir.join("src/util.rs").exists() && t.dir.join("src/helpers.rs").is_file(), "Tab renames it");
    assert_eq!(t.read(|e| e.project.tree.proposal().cloned()), Some(Proposal::Remove(t.dir.join("old.txt"))));
    t.keys("tab");
    assert!(!t.dir.join("old.txt").exists(), "and moves the other to the Trash");
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn a_story_can_create_a_file(cx: &mut TestAppContext) {
    let create = "<create>\nsrc/util.rs\n</create>\n<say>\nA home for helpers.\n</say>\n<new>\npub fn one() -> u32 { 1 }\n</new>\n";
    let mut t = open("fn main() {}\n", &[create, &edit("doc.txt", "fn main() {}", "mod util;\nfn main() {}"), DONE], cx);
    t.ask("move a helper into util");
    assert!(!t.dir.join("src/util.rs").exists(), "nothing is created before Tab");
    assert!(t.read(|e| e.project.tree.ghost().is_some()), "the new file shows in the tree");
    t.keys("tab");
    assert!(t.dir.join("src/util.rs").is_file(), "Tab creates it, folders included");
    let util = t.dir.join("src/util.rs");
    let util_text = t.read(|e| e.project.tabs.text_of(&util).map(str::to_string));
    assert_eq!(util_text.as_deref(), Some("pub fn one() -> u32 { 1 }\n"), "with its first lines");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "then to the next item's file");
    t.keys("tab");
    assert_eq!(t.text(), "mod util;\nfn main() {}\n");
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn a_finished_story_is_remembered(cx: &mut TestAppContext) {
    let done = "<done>\n<memory>\nThe user is learning Python.\n</memory>\n</done>\n";
    let mut t = open(ADD, &[&note("doc.txt", "def add(a, b):", "It adds."), done], cx);
    t.ask("what is this").keys("tab");
    assert!(!t.reviewing());
    assert_eq!(store::load_memory(&t.dir), "The user is learning Python.");
    let logged = store::recent_interactions(&t.dir, 5);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["request"], "what is this");
    assert_eq!(logged[0]["outcome"], "answered");
    assert_eq!(logged[0]["followups"], serde_json::json!([]), "asking for the next item isn't something the user said");
}

#[gpui::test]
fn docs_notes_leave_the_cursor_alone(cx: &mut TestAppContext) {
    let mut t = open("def f():\n    return compute(1, 2)\n", &[DONE], cx);
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
        if t.read(|e| e.shell.as_ref().is_none_or(|s| !s.running())) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        t.cx.run_until_parked();
    }
    panic!("`{command}` never finished");
}

const TWO_ERRORS: &str = "printf 'error: bad\\n  --> a.py:2:1\\n\\nerror: worse\\n --> b.py:1\\n'; exit 1";

#[gpui::test]
fn sh_runs_in_the_background(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("a.py"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(t.dir.join("b.py"), "uno\n").unwrap();
    run_sh(&mut t, TWO_ERRORS);
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "nothing takes over");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "nothing jumps");
    assert_eq!(t.status().as_deref(), Some(&*format!("`{}`: ✗ exit 1, 2 locations · :output to go through", TWO_ERRORS)));
    t.keys("i").input("still ").keys("escape");
    assert_eq!(t.text(), "still x\n", "editing goes on");
}

#[gpui::test]
fn output_goes_through_the_last_commands_errors(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("a.py"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(t.dir.join("b.py"), "uno\n").unwrap();
    run_sh(&mut t, TWO_ERRORS);
    t.command("output");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "a story like any other");
    assert_eq!(t.active().as_deref(), Some("a.py"), "the first error opens its file");
    assert_eq!(t.cursor(), 4, "at its line");
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("b.py"));
    t.keys("tab");
    assert!(!t.reviewing(), "the last Tab finishes");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
}

#[gpui::test]
fn claude_is_told_the_last_command(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[&note("doc.txt", "x", "It failed."), DONE], cx);
    run_sh(&mut t, "echo one; echo two; exit 2");
    t.ask("why did that fail?");
    let command = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap().surroundings.command);
    assert_eq!(command.as_deref(), Some("$ echo one; echo two; exit 2 (✗ exit 2)\none\ntwo"));
}

#[gpui::test]
fn space_slash_goes_through_the_projects_matches(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("let total = 1;\n", &[], cx);
    std::fs::write(t.dir.join("b.txt"), "nothing\nprint(total)\n").unwrap();
    t.keys("space /").input("TOTAL").keys("enter");
    t.cx.run_until_parked();
    assert_eq!(t.status().as_deref(), Some("no matches for TOTAL"), "a capital makes it exact");
    t.keys("space /").input("total").keys("enter");
    t.cx.run_until_parked();
    assert_eq!((t.active(), t.cursor()), (Some("b.txt".into()), "nothing\n".len()), "files in order, at the match's line");
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("tab");
    assert!(!t.reviewing(), "the last Tab finishes");
}

#[gpui::test]
fn a_tour_walks_through_other_files(cx: &mut TestAppContext) {
    let start = note("doc.txt", "fn main() { run(); }", "It starts here.");
    let run = note("src/lib.rs", "pub fn run() {", "The work happens in run.");
    let helper = note("src/lib.rs", "    helper();", "Which calls the helper.");
    let back = note("doc.txt", "fn main() { run(); }", "And that's all main does.");
    let mut t = open("fn main() { run(); }\n", &[&start, &run, &helper, &back, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {\n    helper();\n}\n").unwrap();
    t.ask("give me a tour");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "a note names its file");
    assert_eq!(t.cursor(), 0);
    t.keys("tab");
    assert_eq!(t.cursor(), "pub fn run() {\n".len());
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    assert_eq!(t.read(|e| e.project.tabs.len()), 1, "the tab the tour opened closes as it moves on");
    t.keys("tab");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_change_edits_other_existing_files(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let main = edit("doc.txt", "fn main() { run(); }", "fn main() { run(true); }");
    let mut t = open("fn main() { run(); }\n", &[&lib, &main, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    // Unsaved changes elsewhere are written before the model reads the project.
    t.command("e src/lib.rs").keys("x y p").command("e doc.txt").keys("u");
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "the first edit is in another file");
    let shown = |req: ghostwrite_ai::Request| req.others.into_iter().map(|o| (o.path, o.text)).collect::<Vec<_>>();
    let ahead = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let item = state.story.as_ref().unwrap().current_item().unwrap();
        state.request_ahead(&e.project, &[item]).unwrap()
    });
    assert_eq!(
        shown(ahead),
        [("src/lib.rs".to_string(), "pub fn run(verbose: bool) {}\npub fn run() {}\n".to_string())],
        "asked ahead, Claude sees the other file as if the edit there were accepted"
    );
    t.keys("tab");
    let now = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap());
    assert_eq!(shown(now).len(), 1, "and goes on seeing the other files the story is in");
    assert_eq!(
        t.read(|e| e.project.tabs.text_of(&e.project.root.join("src/lib.rs")).map(str::to_string)).as_deref(),
        Some("pub fn run(verbose: bool) {}\npub fn run() {}\n")
    );
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "back for the next edit");
    t.keys("tab");
    assert_eq!(t.text(), "fn main() { run(true); }\n");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_quoted_line_number_picks_the_copy_meant(cx: &mut TestAppContext) {
    let text = "if a {\n    done();\n}\nif b {\n    done();\n}\n";
    let mut t = open(text, &[&edit("doc.txt", "5│    done();", "    finish();"), DONE], cx);
    t.ask("finish b");
    t.keys("tab");
    assert_eq!(t.text(), "if a {\n    done();\n}\nif b {\n    finish();\n}\n", "not the copy nearer the cursor");
}

#[gpui::test]
fn ai_mode_reviews_and_goes_back_to_where_you_were(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    // From select mode: the selection is the context, and comes back after.
    t.keys("v j");
    let selected = t.read(|e| e.tab().unwrap().view.selection);
    t.keys("space a");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter");
    // Looking around works; editing doesn't.
    t.keys("g g d x");
    assert_eq!(t.text(), ADD, "no edits in AI mode");
    t.keys("escape escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Select, "back to select mode");
    assert_eq!(t.read(|e| e.tab().unwrap().view.selection), selected, "with the selection");
}

#[gpui::test]
fn ai_mode_from_the_tree_goes_back_to_the_tree(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
    t.keys("space a");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
}

#[gpui::test]
fn asking_about_a_stop_hands_over_to_the_model(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    // The first reply is the stop the editor gives; the model's come after.
    let mut t = open("x\n", &[DONE, &note("doc.txt", "x", "It prints lines."), DONE], cx);
    run_sh(&mut t, "echo one; echo two");
    t.command("output");
    assert!(t.reviewing(), "the output, as a note");
    t.keys("enter").input("what is this?").keys("enter");
    let said = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups()[0].said.clone());
    assert_eq!(said, "what is this?");
    t.keys("tab");
    assert!(!t.reviewing(), "the model's answer, then <done>");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
}

#[gpui::test]
fn a_follow_up_is_answered_where_it_was_asked(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let mut t = open("fn main() { run(); }\n", &[&lib, &note("src/lib.rs", "", "So callers can ask for detail."), DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "\n\npub fn run() {}\n").unwrap();
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"));
    // Asked about the edit in lib.rs: the answer shows there, under it.
    t.keys("enter").input("why?").keys("enter");
    let at = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let story = state.story.as_ref().unwrap();
        let located = state.locate(&e.project, story.current().unwrap()).unwrap();
        (located.tab.map(|ix| e.project.tabs.get(ix).unwrap().doc.name().to_string()), located.near)
    });
    assert_eq!(at, (Some("lib.rs".into()), 2..3), "under the edit's line");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "and the view stays there");
}

#[gpui::test]
fn a_comment_changes_what_comes_next(cx: &mut TestAppContext) {
    let types = edit("doc.txt", "def add(a, b):", "def add(a: int, b: int):");
    let mut t = open(ADD, &[&note("doc.txt", "def add(a, b):", "It adds."), &types, DONE], cx);
    t.ask("walk me through this");
    t.keys("enter").input("I care about the types, not the body").keys("enter");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a: int, b: int):\n    pass\n", "the next item followed what was said");
    assert!(!t.reviewing());
}

#[gpui::test]
fn the_tree_marks_where_the_story_is_waiting(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let main = edit("doc.txt", "fn main() { run(); }", "fn main() { run(true); }");
    let mut t = open("fn main() { run(); }\n", &[&lib, &main, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let waiting = |t: &mut Setup| {
        let dir = t.dir.clone();
        let mut files: Vec<(String, super::story::Waiting)> = t
            .read(|e| e.story.as_ref().map(|s| s.files(&e.project)).unwrap_or_default())
            .into_iter()
            .map(|(p, w)| (p.strip_prefix(&dir).unwrap().display().to_string(), w))
            .collect();
        files.sort();
        files
    };
    t.keys("space a");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), super::story::Waiting::Here)], "asking about this file");
    t.input("add a verbose flag").keys("enter");
    assert_eq!(waiting(&mut t), [("src/lib.rs".into(), super::story::Waiting::Here)]);
    t.keys("tab");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), super::story::Waiting::Here)], "on to the next item");
}

#[gpui::test]
fn claude_hears_the_whole_conversation_and_the_editor(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), &note("doc.txt", "", "OK."), DONE], cx);
    std::fs::write(t.dir.join("other.py"), "x = 1\n").unwrap();
    t.command("e other.py").command("e doc.txt");
    t.ask("types and body").keys("tab");
    t.keys("enter").input("why int?").keys("enter");
    let req = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap());
    assert_eq!(req.surroundings.open_files, ["doc.txt", "other.py"], "what is open, in tab order");
    assert_eq!(req.followups[0].reply, fix(), "each reply, whole");
    assert_eq!(req.followups[0].decided.len(), 1, "what was accepted");
    assert_eq!(req.followups[0].said, "", "then the next item was asked for");
    assert_eq!(req.followups[1].said, "why int?");
}

#[gpui::test]
fn an_edit_that_doesnt_apply_is_rewritten_first(cx: &mut TestAppContext) {
    let wrong = edit("doc.txt", "    return None", "    return a + b");
    let mut t = open(ADD, &[&wrong, &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert_eq!(followups.len(), 1, "the model was told, without the user");
    assert!(followups[0].correction && followups[0].said.contains("aren't in doc.txt"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the rewritten edit is the one shown");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_note_whose_lines_arent_there_is_rewritten_first(cx: &mut TestAppContext) {
    let wrong = note("doc.txt", "def add(a, b): ...", "It adds.");
    let mut t = open(ADD, &[&wrong, &note("doc.txt", "def add(a, b):", "It adds."), DONE], cx);
    t.ask("what is this");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups.len() == 1 && followups[0].correction, "{followups:?}");
    assert_eq!(t.cursor(), 0, "the corrected note is at its line");
}

#[gpui::test]
fn an_item_in_a_missing_file_is_rewritten_first(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&edit("src/doc.txt", "    pass", "    return a + b"), &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("no file src/doc.txt"), "{:?}", followups[0]);
    assert!(!t.dir.join("src/doc.txt").exists(), "nothing was made for it");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn an_item_about_a_pdf_is_rewritten_about_another_file(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&note("paper.pdf", "", "It says hello."), &fix(), DONE], cx);
    std::fs::write(t.dir.join("paper.pdf"), "%PDF-1.4").unwrap();
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("a PDF"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the next reply is used");
}

#[gpui::test]
fn a_reply_without_the_format_is_asked_again(cx: &mut TestAppContext) {
    let mut t = open(ADD, &["Sure, I'd return a + b.", &fix(), DONE], cx);
    t.ask("implement it");
    assert!(t.reviewing(), "the story didn't just end");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_reply_that_breaks_the_format_is_stopped_and_asked_again(cx: &mut TestAppContext) {
    let broken = "<file>\ndoc.txt\n</file>\n<say>\nNo <at>.\n</say>\n";
    let mut t = open(ADD, &[broken, &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("<at>"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn corrections_stop_after_a_few_tries(cx: &mut TestAppContext) {
    let wrong = edit("doc.txt", "    return None", "    return a + b");
    let mut t = open(ADD, &[&wrong, &wrong, &wrong, &wrong, DONE], cx);
    t.ask("implement it");
    let n = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().len());
    assert_eq!(n, ghostwrite_ai::story::MAX_REVISIONS);
    assert!(t.reviewing(), "the edit is left to redirect");
    t.keys("enter").input("never mind").keys("enter");
    assert!(!t.reviewing());
}

#[gpui::test]
fn switching_to_a_storys_file_keeps_it_centered_on_the_item(cx: &mut TestAppContext) {
    let t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("other.txt"), "a\nb\nc\n").unwrap();
    let path = t.dir.join("other.txt");
    let scroll = t.editor.update(t.cx, |e, _| {
        let ix = e.project.open(&path).unwrap();
        e.project.tabs.get_mut(ix).unwrap().select(ghostwrite_core::Range::point(2), super::Autoscroll::Center(1..2));
        e.project.activate(ix);
        e.tab().unwrap().view.autoscroll.clone()
    });
    assert_eq!(scroll, Some(super::Autoscroll::Center(1..2)));
}

#[gpui::test]
fn the_watcher_brings_in_other_programs_changes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("one\n", &[], cx);
    std::fs::write(t.dir.join("doc.txt"), "changed elsewhere\n").unwrap();
    std::fs::write(t.dir.join("new.txt"), "").unwrap();
    let new = t.dir.join("new.txt");
    for _ in 0..500 {
        if t.text() == "changed elsewhere\n" && t.read(|e| e.project.tree.index_of(&new).is_some()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.cx.run_until_parked();
    }
    panic!("the change never arrived: {:?}", t.text());
}

#[gpui::test]
fn claude_hears_about_commands_run_mid_story(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open(ADD, &[&fix(), &note("doc.txt", "", "OK."), DONE], cx);
    run_sh(&mut t, "echo before");
    t.ask("types and body");
    run_sh(&mut t, "echo during");
    t.keys("enter").input("does it pass now?").keys("enter");
    let req = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        state.request(&e.project).unwrap()
    });
    let followup = &req.followups[0];
    assert_eq!(followup.command.as_deref(), Some("$ echo during (✓ exit 0)\nduring"), "the newest turn says what ran since");
    let first = t.read(|e| e.story.as_ref().unwrap().command.first.clone());
    assert_eq!(first, Some(Some("$ echo before (✓ exit 0)\nbefore".into())), "the start of the conversation stays as it was");
}

#[gpui::test]
fn claude_reads_the_terminal_but_only_the_user_types_in_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[&note("doc.txt", "x", "It printed ok."), DONE], cx);
    t.keys("space t");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (true, Mode::Terminal), "space t is terminal mode");
    t.keys("e c h o space o k enter");
    let printed = |e: &Editor| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ok"));
    for _ in 0..400 {
        if t.read(printed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.cx.run_until_parked();
    }
    assert!(t.read(printed), "the shell ran it: {:?}", t.read(|e| e.terminal_context()));
    assert_eq!(t.text(), "x\n", "the keys went to the shell, not the file");
    t.keys("escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "one Esc is the shell's");
    t.keys("escape");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (true, Mode::Normal), "a second leaves, and the terminal stays up");
    t.cx.update(|_, cx| cx.bind_keys([gpui::KeyBinding::new("cmd-j", crate::actions::ToggleTerminal, Some("Editor"))]));
    t.keys("cmd-j");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "cmd-j goes back in");
    t.keys("cmd-j");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (false, Mode::Normal), "and from there hides it");
    t.ask("what did it print?");
    let told = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap().surroundings.terminal);
    assert!(told.is_some_and(|text| text.lines().any(|l| l.trim() == "ok")), "Claude is shown the terminal");
}
