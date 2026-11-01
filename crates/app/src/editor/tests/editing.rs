//! Editing with keys and the mouse: modal keys, typing, searching, jumps, pickers, repeat and macros, colors, the cheatsheet.

use super::*;

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

/// Laid out for real, in a window too short for it: there's somewhere to scroll to.
#[gpui::test]
fn the_cheatsheet_really_scrolls(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("space ?");
    t.cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(300.)));
    let draw = |t: &mut Setup| {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    };
    draw(&mut t);
    let most = t.read(|e| e.cheatsheet_scroll.max_offset().height);
    assert!(most > gpui::px(0.), "it overflows: {most:?}");
    let top = |t: &mut Setup| t.read(|e| e.cheatsheet_scroll.offset().y);
    assert_eq!(top(&mut t), gpui::px(0.));
    t.keys("j");
    draw(&mut t);
    assert!(top(&mut t) < gpui::px(0.), "j goes down: {:?}", top(&mut t));
    t.keys("k");
    draw(&mut t);
    assert_eq!(top(&mut t), gpui::px(0.), "k comes back");
    t.keys("k");
    assert_eq!(top(&mut t), gpui::px(0.), "and no further");
    for _ in 0..200 {
        t.keys("j");
    }
    draw(&mut t);
    assert_eq!(top(&mut t), -most, "and only as far as there is");
}

#[gpui::test]
fn space_question_mark_shows_the_cheatsheet_j_k_scroll_it_and_any_other_key_closes_it(cx: &mut TestAppContext) {
    let mut t = open("one\ntwo\n", &[], cx);
    t.keys("space ?");
    assert!(t.read(|e| e.cheatsheet));
    t.keys("j k down up");
    assert!(t.read(|e| e.cheatsheet), "j and k scroll it");
    assert_eq!(t.cursor(), 0, "and only it");
    t.keys("l");
    assert!(!t.read(|e| e.cheatsheet));
    assert_eq!(t.cursor(), 0, "the closing key does nothing else");
}

fn cursors(t: &mut Setup) -> usize {
    t.read(|e| e.tab().unwrap().view.others.len() + 1)
}

#[gpui::test]
fn typing_happens_at_every_cursor_as_one_undo(cx: &mut TestAppContext) {
    let mut t = open("aa\nbb\ncc\n", &[], cx);
    t.keys("C C");
    assert_eq!(cursors(&mut t), 3);
    t.keys("i").input("XY").keys("escape");
    assert_eq!(t.text(), "XYaa\nXYbb\nXYcc\n");
    assert_eq!(cursors(&mut t), 3, "still three, on what they were on");
    t.keys("u");
    assert_eq!(t.text(), "aa\nbb\ncc\n", "one undo for all of it");
    assert_eq!(cursors(&mut t), 1);
}

#[gpui::test]
fn a_key_is_carried_out_at_every_cursor(cx: &mut TestAppContext) {
    let mut t = open("one two\nthree four\n", &[], cx);
    t.keys("C");
    t.keys("w d");
    assert_eq!(t.text(), "two\nfour\n");
    t.keys("u");
    assert_eq!(t.text(), "one two\nthree four\n");
}

#[gpui::test]
fn lines_of_different_lengths_and_multibyte_text_stay_right(cx: &mut TestAppContext) {
    let mut t = open("→→→x\nab\n→y\n", &[], cx);
    t.keys("l C C");
    t.keys("i").input("|").keys("escape");
    let text = t.text();
    assert_eq!(text.matches('|').count(), 3, "{text}");
    assert_eq!(text.replace('|', ""), "→→→x\nab\n→y\n", "nothing else moved or broke");
}

#[gpui::test]
fn one_cursor_comes_back(cx: &mut TestAppContext) {
    let mut t = open("aa\nbb\ncc\n", &[], cx);
    t.keys("C C");
    t.keys(",");
    assert_eq!(cursors(&mut t), 1);
    t.keys("C");
    t.keys("escape");
    assert_eq!(cursors(&mut t), 1, "esc too");
    t.keys("C").keys("g g");
    t.keys("alt-C");
    let before = cursors(&mut t);
    t.keys("i").input("x").keys("escape");
    assert_eq!(t.text().matches('x').count(), before);
}

#[gpui::test]
fn s_makes_a_cursor_of_each_match(cx: &mut TestAppContext) {
    let mut t = open("cat dog cat\nbird cat\n", &[], cx);
    t.keys("s").input("cat").keys("enter");
    assert_eq!(cursors(&mut t), 3);
    t.keys("c").input("cow").keys("escape");
    assert_eq!(t.text(), "cow dog cow\nbird cow\n");
    t.keys("u");
    assert_eq!(t.text(), "cat dog cat\nbird cat\n");
}

#[gpui::test]
fn what_happens_once_happens_once(cx: &mut TestAppContext) {
    let mut t = open("aa\nbb\n", &[], cx);
    t.keys("C");
    t.command("n");
    assert_eq!(t.read(|e| e.project.tabs.len()), 2, "one new tab, not one for each cursor");
}

#[gpui::test]
fn a_cursor_only_goes_where_there_is_a_line(cx: &mut TestAppContext) {
    let mut t = open("only", &[], cx);
    t.keys("C");
    assert_eq!(cursors(&mut t), 1);
    t.keys("alt-C");
    assert_eq!(cursors(&mut t), 1);
}

#[gpui::test]
fn a_story_isnt_given_more_than_one_cursor(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.keys("C");
    t.ask("implement it");
    t.keys("C");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the step, once");
}
