//! Files: saving (and not over another program's changes), the `:` line and its history, other programs' changes, the file tree.

use super::*;

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

#[cfg(unix)]
#[gpui::test]
fn the_same_file_opens_once_however_its_spelled(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/a.rs"), "").unwrap();
    std::os::unix::fs::symlink(t.dir.join("src"), t.dir.join("link")).unwrap();
    t.command("e src/a.rs").command("e src/../src/a.rs").command("e link/a.rs");
    assert_eq!(t.read(|e| e.project.tabs.len()), 2);
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
fn another_programs_changes_come_in_with_unsaved_ones(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\nthree\nfour\nfive\n", &[], cx);
    let path = t.dir.join("doc.txt");
    // Unsaved: on the second line, the cursor after what was typed.
    t.keys("j i").input("my ").keys("escape");
    std::fs::write(&path, "zero\none\ntwo\nthree\nfour\nFIVE\n").unwrap();
    changed_elsewhere(&mut t, &["doc.txt"]);
    assert_eq!(t.text(), "zero\none\nmy two\nthree\nfour\nFIVE\n", "theirs in, with mine");
    assert_eq!(t.read(|e| e.tab().unwrap().doc.buffer.line_of(e.tab().unwrap().cursor())), 2, "the cursor stays on its line");
    assert!(t.read(|e| e.tab().unwrap().doc.dirty), "mine still unsaved");
    t.command("w");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "zero\none\nmy two\nthree\nfour\nFIVE\n", "and saving keeps both");
    t.keys("u");
    assert_eq!(t.text(), "one\nmy two\nthree\nfour\nfive\n", "theirs came in as one undo step");
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
    t.editor.update(t.cx, |e, _| e.history = Default::default());
    t.command("n").command("bp");
    let typed = |t: &mut Setup| t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| f.read_with(t.cx, |f, _| f.text().to_string()));
    t.keys(": up");
    assert_eq!(typed(&mut t).as_deref(), Some("bp"));
    t.keys("up");
    assert_eq!(typed(&mut t).as_deref(), Some("n"));
    t.keys("down down");
    assert_eq!(typed(&mut t).as_deref(), Some(""));
}

/// Three files open, the second active: their order.
fn three_tabs(t: &mut Setup) -> Vec<String> {
    let editor = t.editor.clone();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(t.dir.join(name), name).unwrap();
    }
    let paths = ["a.txt", "b.txt", "c.txt"].map(|n| t.dir.join(n)).to_vec();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(paths, cx)));
    t.cx.run_until_parked();
    tab_names(t)
}

fn tab_names(t: &mut Setup) -> Vec<String> {
    t.read(|e| e.project.tabs.iter().map(|tab| tab.doc.name().to_string()).collect())
}

#[gpui::test]
fn tabs_move_with_commands_and_the_active_one_stays(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let names = three_tabs(&mut t);
    let last = names.len() - 1;
    let c = names[last].clone();
    assert_eq!(t.read(|e| e.project.tabs.active_index()), last);
    t.command("tab-left");
    let moved = tab_names(&mut t);
    assert_eq!(moved[last - 1], c, "{moved:?}");
    assert_eq!(t.active().as_deref(), Some(c.as_str()), "it's still the active one");
    t.command("tab-right").command("tab-right");
    assert_eq!(tab_names(&mut t), names, "and no further than the end");
    assert_eq!(t.active().as_deref(), Some(c.as_str()));
    t.keys("g P");
    assert_eq!(tab_names(&mut t), moved, "gP moves it back");
    t.keys("g N");
    assert_eq!(tab_names(&mut t), names, "and gN on");
    // As a keyboard sends them: the letter, with shift.
    t.keys("g shift-p");
    assert_eq!(tab_names(&mut t), moved, "with shift too");
    t.keys("g shift-n");
    assert_eq!(tab_names(&mut t), names);
    assert_eq!(t.active().as_deref(), Some(c.as_str()));
}

/// Dragged with the mouse, on the tab bar as it's laid out.
#[gpui::test]
fn a_count_says_how_many_places(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let names = three_tabs(&mut t);
    let last = names.len() - 1;
    let c = names[last].clone();
    t.keys("2 g shift-p");
    let moved = tab_names(&mut t);
    assert_eq!(moved[last - 2], c, "2gP moves it two places: {moved:?}");
    assert_eq!(t.active().as_deref(), Some(c.as_str()));
    t.keys("9 g shift-n");
    assert_eq!(tab_names(&mut t), names, "9gN only as far as the end");
    assert_eq!(t.read(|e| e.project.tabs.active_index()), last);

    t.keys("2 g p");
    assert_eq!(t.read(|e| e.project.tabs.active_index()), last - 2, "2gp goes two tabs back");
    assert_eq!(tab_names(&mut t), names, "without moving any");
    t.keys("2 g n");
    assert_eq!(t.read(|e| e.project.tabs.active_index()), last, "2gn two on");
    t.keys("g n");
    assert_eq!(t.read(|e| e.project.tabs.active_index()), 0, "and round the end");
}

#[gpui::test]
fn a_tab_is_dragged_to_another_place(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let names = three_tabs(&mut t);
    let active = t.active();
    let at = |t: &mut Setup, ix: usize| {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
        t.cx.debug_bounds(Box::leak(format!("tab-{ix}").into_boxed_str())).unwrap_or_else(|| panic!("tab {ix} is drawn")).center()
    };
    let (from, to) = (at(&mut t, 0), at(&mut t, names.len() - 1));
    t.cx.simulate_mouse_down(from, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_move(gpui::point(from.x + gpui::px(10.), from.y), gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_move(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_up(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.run_until_parked();
    let moved = tab_names(&mut t);
    assert_eq!(moved.last(), names.first(), "the first is now the last: {moved:?}");
    assert_eq!(moved[0], names[1]);
    assert_eq!(t.active(), active, "dragging doesn't change which is active");
}

#[gpui::test]
fn choosing_a_model_is_said_and_sticks(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.editor.update(t.cx, |e, _| e.choose_model(ghostwrite_ai::Backend::Local { model: "/no/such.gguf".into(), label: "tiny".into() })).unwrap();
    assert_eq!(t.status().as_deref(), Some("asking tiny from now on"));
    assert_eq!(t.read(|e| e.backend.as_ref().map(|b| b.name().to_string())).as_deref(), Some("tiny"));
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
fn files_drives_the_file_tree(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    // Rows: src/, doc.txt (the active file, under the cursor).
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal));
    t.keys("k l");
    assert_eq!(t.read(|e| e.project.tree.rows().len()), 3, "l opens the folder");
    t.keys("j l");
    assert_eq!(t.active().as_deref(), Some("src/main.rs"), "l on a file previews it");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal), "and the keys stay in Files");
    t.keys("j enter");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "enter opens it and goes to the code");
    t.keys("space e escape");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal));
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Files, Mode::Normal), "space e goes to Files");
    t.keys("space e");
    assert_eq!(t.at(), (ViewKind::Code, Mode::Normal), "and back");
}

#[gpui::test]
fn files_adds_and_renames(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.keys("space e a").input("notes/todo.md").keys("enter");
    assert!(t.dir.join("notes/todo.md").is_file());
    assert_eq!(t.active().as_deref(), Some("notes/todo.md"), "a new file opens");
    t.keys("space e r").keys(&crate::keys::in_field("cmd-a")).input("done.md").keys("enter");
    assert!(t.dir.join("notes/done.md").is_file());
    assert_eq!(t.active().as_deref(), Some("notes/done.md"), "the tab follows the rename");
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

/// Time for the unsaved changes to be copied.
fn let_it_copy(t: &mut Setup) {
    t.cx.executor().advance_clock(std::time::Duration::from_secs(3));
    t.cx.run_until_parked();
}

/// A new editor on the same project, as after a crash.
fn reopened<'a>(t: &mut Setup<'a>) -> Entity<Editor> {
    let root = t.dir.clone();
    t.cx.update(|window, cx| {
        gpui::AppContext::new(cx, |cx| {
            let mut editor = Editor::new(root.clone(), Default::default(), window, cx);
            editor.recover(cx);
            editor
        })
    })
}

#[gpui::test]
fn unsaved_changes_come_back_after_a_crash(cx: &mut TestAppContext) {
    let mut t = open("on disk\n", &[], cx);
    t.keys("i");
    t.input("typed ");
    t.keys("escape");
    assert_eq!(t.text(), "typed on disk\n");
    let_it_copy(&mut t);

    let after = reopened(&mut t);
    let found = t.cx.update(|_, cx| {
        let editor = after.read(cx);
        let tab = editor.project.tab().unwrap();
        (tab.doc.buffer.to_string(), tab.doc.dirty, editor.project.status.clone().map(|s| s.to_string()))
    });
    assert_eq!(found.0, "typed on disk\n");
    assert!(found.1, "unsaved, not written over the file");
    assert!(found.2.is_some_and(|s| s.contains("brought back")), "and it's said");
    assert_eq!(std::fs::read_to_string(t.dir.join("doc.txt")).unwrap(), "on disk\n");
}

#[gpui::test]
fn what_was_saved_or_given_up_on_doesnt_come_back(cx: &mut TestAppContext) {
    let mut t = open("on disk\n", &[], cx);
    t.keys("i");
    t.input("typed ");
    t.keys("escape");
    let_it_copy(&mut t);
    t.command("w");
    let_it_copy(&mut t);
    let after = reopened(&mut t);
    assert!(t.cx.update(|_, cx| after.read(cx).project.tabs.iter().all(|tab| !tab.doc.dirty)), "saved: nothing to bring back");

    t.keys("i");
    t.input("more ");
    t.keys("escape");
    let_it_copy(&mut t);
    t.editor.update(t.cx, |e, _| e.forget_unsaved());
    let after = reopened(&mut t);
    assert!(t.cx.update(|_, cx| after.read(cx).project.tabs.iter().all(|tab| !tab.doc.dirty)), "left without saving: gone");
}

#[gpui::test]
fn a_new_file_never_saved_comes_back_too(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.command("n");
    t.keys("i");
    t.input("a thought");
    t.keys("escape");
    let_it_copy(&mut t);
    let after = reopened(&mut t);
    let kept =
        t.cx.update(|_, cx| after.read(cx).project.tabs.iter().any(|tab| tab.doc.path.is_none() && tab.doc.dirty && tab.doc.buffer.to_string() == "a thought"));
    assert!(kept);
}

#[gpui::test]
fn an_api_key_is_not_kept_in_the_history(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.editor.update(t.cx, |e, _| e.history = Default::default());
    t.command("n");
    // Not a key, so the Keychain isn't touched; it's typed all the same.
    t.command("api-key not-a-key");
    t.keys(": up");
    let typed = t.read(|e| e.prompt.as_ref().map(|p| p.field.clone())).map(|f| f.read_with(t.cx, |f, _| f.text().to_string()));
    assert_eq!(typed.as_deref(), Some("n"));
}

#[gpui::test]
fn the_gutter_knows_where_a_file_differs_from_git(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("one\ntwo\n", &[], cx);
    let git = |dir: &std::path::Path, args: &[&str]| {
        let done = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    };
    git(&t.dir, &["init", "-q"]);
    git(&t.dir, &["add", "."]);
    git(&t.dir, &["commit", "-qm", "first"]);
    t.editor.update(t.cx, |e, _| e.forget_git_base());
    t.keys("i").input("new\n").keys("escape");
    let marked = |t: &mut Setup| t.read(|e| e.tab().unwrap().doc.git.changes.clone());
    let wanted = vec![ghostwrite_project::git::Change { lines: 0..1, kind: ghostwrite_project::git::ChangeKind::Added }];
    for _ in 0..300 {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
        if marked(&mut t) == wanted {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(marked(&mut t), wanted);
}

#[gpui::test]
fn a_file_thats_not_utf8_is_written_back_the_same(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    let path = t.dir.join("latin.txt");
    std::fs::write(&path, [b'b', b'l', 0xE5, b'\n']).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![path.clone()], cx)));
    t.cx.run_until_parked();
    assert_eq!(t.text(), "bl\u{e5}\n");
    t.keys("i").input("x").keys("escape");
    t.command("w");
    assert_eq!(std::fs::read(&path).unwrap(), [b'x', b'b', b'l', 0xE5, b'\n']);
    t.keys("i").input("\u{2192}").keys("escape");
    t.command("w");
    assert!(t.status().is_some_and(|s| s.contains("can't hold")), "{:?}", t.status());
    assert_eq!(std::fs::read(&path).unwrap(), [b'x', b'b', b'l', 0xE5, b'\n'], "and nothing's lost on disk");
}

fn draw(t: &mut Setup) {
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
}

#[gpui::test]
fn two_panes_show_two_tabs_and_the_keys_move_between_them(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    t.keys("space w v");
    assert!(t.status().is_some_and(|s| s.contains("open another file")), "one tab: nothing to show beside it");
    let names = three_tabs(&mut t);
    let start = t.active();
    t.keys("space w v");
    draw(&mut t);
    let other = t.read(|e| e.other_pane_doc()).expect("a second pane");
    assert!(t.cx.debug_bounds("other-pane").is_some(), "and it's drawn");
    let id = |t: &mut Setup| t.read(|e| e.tab().unwrap().doc.id);
    let here = id(&mut t);
    assert_ne!(here, other);

    t.keys("space w w");
    assert_eq!(id(&mut t), other, "the keys went over");
    assert_eq!(t.read(|e| e.other_pane_doc()), Some(here), "and what they left stays in its pane");
    assert!(!t.read(|e| e.split.as_ref().unwrap().other_on_right), "on the left now");
    t.keys("space w w");
    assert_eq!(id(&mut t), here);
    assert!(t.read(|e| e.split.as_ref().unwrap().other_on_right), "back where it was");

    t.keys("space w q");
    draw(&mut t);
    assert_eq!(t.read(|e| e.other_pane_doc()), None);
    assert_eq!(t.active(), start);
    assert_eq!(tab_names(&mut t), names, "no tabs came or went");
}

#[gpui::test]
fn typing_goes_to_the_pane_with_the_keys_and_a_click_moves_them(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    three_tabs(&mut t);
    t.cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(600.)));
    t.command("vsplit");
    draw(&mut t);
    let other = t.read(|e| e.other_pane_doc()).unwrap();
    let text_of = |t: &mut Setup, id: usize| t.read(|e| e.project.tabs.by_id(id).unwrap().doc.buffer.to_string());
    let before = text_of(&mut t, other);
    t.keys("i").input("typed").keys("escape");
    assert!(t.text().starts_with("typed"));
    assert_eq!(text_of(&mut t, other), before, "not in the other pane");

    let pane = t.cx.debug_bounds("other-pane").expect("drawn");
    t.cx.simulate_click(gpui::point(pane.center().x, pane.origin.y + gpui::px(8.)), gpui::Modifiers::none());
    t.cx.run_until_parked();
    assert_eq!(t.read(|e| e.tab().unwrap().doc.id), other, "a click there brings the keys");
    t.keys("i").input("there").keys("escape");
    assert!(text_of(&mut t, other).contains("there"));
}

#[gpui::test]
fn the_wheel_scrolls_the_pane_under_it(cx: &mut TestAppContext) {
    let long: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let mut t = open(&long, &[], cx);
    let (a, b) = (t.dir.join("a.txt"), t.dir.join("b.txt"));
    std::fs::write(&a, &long).unwrap();
    std::fs::write(&b, &long).unwrap();
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![a.clone(), b.clone()], cx)));
    t.cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(600.)));
    t.command("vsplit");
    draw(&mut t);
    let other = t.read(|e| e.other_pane_doc()).unwrap();
    let here = t.read(|e| e.tab().unwrap().doc.id);
    let top = |t: &mut Setup, id: usize| t.read(|e| e.project.tabs.by_id(id).unwrap().view.scroll_top);
    let (here_top, other_top) = (top(&mut t, here), top(&mut t, other));
    let pane = t.cx.debug_bounds("other-pane").expect("drawn");
    t.cx.simulate_event(gpui::ScrollWheelEvent {
        position: pane.center(),
        delta: gpui::ScrollDelta::Lines(gpui::point(0., -5.)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(&mut t);
    assert!(top(&mut t, other) > other_top, "the pane under the mouse");
    assert_eq!(top(&mut t, here), here_top, "not the one with the keys");
    assert_eq!(t.read(|e| e.tab().unwrap().doc.id), here, "and the keys stay");
}

#[gpui::test]
fn a_tab_closed_in_the_other_pane_takes_the_pane_with_it(cx: &mut TestAppContext) {
    let mut t = open("x\n", &[], cx);
    three_tabs(&mut t);
    t.command("vsplit");
    let other = t.read(|e| e.other_pane_doc()).unwrap();
    t.editor.update(t.cx, |e, cx| {
        let ix = e.project.tabs.index_of_id(other).unwrap();
        e.project.close(ix);
        cx.notify();
    });
    draw(&mut t);
    assert_eq!(t.read(|e| e.other_pane_doc()), None);
}
