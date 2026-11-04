//! The editor's stops (`space /`, `space d`), and the terminal: what Claude is told of it.

use super::*;

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
fn asking_about_a_stop_hands_over_to_the_model(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    // The first reply is the stop the editor gives; the model's come after.
    let mut t = open("x\n", &[DONE, &note("doc.txt", "x", "It prints lines."), DONE], cx);
    t.command("grep x");
    t.cx.run_until_parked();
    assert!(t.reviewing(), "the match, as a note");
    t.keys("enter").input("what is this?").keys("enter");
    let said = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups()[0].said.clone());
    assert_eq!(said, "what is this?");
    t.keys("tab");
    assert!(!t.reviewing(), "the model's answer, then <done>");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal);
}

#[gpui::test]
fn claude_reads_the_terminal_but_only_the_user_types_in_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[&note("doc.txt", "x", "It printed ok."), DONE], cx);
    t.keys("space t");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (true, Mode::Terminal), "space t is terminal mode");
    t.keys("e c h o space o k enter");
    let printed = |e: &Editor| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ok"));
    for _ in 0..3000 {
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
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode, e.reading())), (true, Mode::Normal, true), "a second reads it in normal mode");
    t.keys("escape");
    assert_eq!(t.read(|e| (e.terminal.shown, e.reading())), (true, false), "and Esc from there is back in the code, the terminal still up");
    t.cx.update(|_, cx| cx.bind_keys([gpui::KeyBinding::new("cmd-j", crate::actions::ToggleTerminal, Some("Editor"))]));
    t.keys("cmd-j");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "cmd-j goes back in");
    t.keys("cmd-j");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (false, Mode::Normal), "and from there hides it");
    t.ask("what did it print?");
    let told = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap().surroundings.terminal);
    assert!(told.is_some_and(|text| text.lines().any(|l| l.trim() == "ok")), "Claude is shown the terminal");
}

#[gpui::test]
fn an_accepted_command_runs_in_the_terminal_and_the_story_goes_straight_on(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho ran-$((40 + 2))\n</run>\n<say>\nCheck the sum.\n</say>\n";
    let mut t = open("x\n", &[run, &note("doc.txt", "x", "It printed 42."), DONE], cx);
    t.ask("does it add up?");
    assert!(t.read(|e| e.terminal.shown), "a command to run: the terminal shows");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "the keys stay with the review");
    t.keys("tab");
    let printed = |e: &Editor| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ran-42"));
    for _ in 0..3000 {
        if t.read(printed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.cx.run_until_parked();
    }
    assert!(t.read(printed), "Tab ran it: {:?}", t.read(|e| e.terminal_context()));
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "the keys stay with the story");
    assert!(
        t.read(|e| matches!(e.story.as_ref().unwrap().story.as_ref().unwrap().current_item(), Some(ghostwrite_ai::story::Item::Note(_)))),
        "which went straight on"
    );
    let followup = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups()[0].clone());
    assert_eq!(followup.decided, ["ran in the terminal: `echo ran-$((40 + 2))`"]);
    t.keys("tab");
    assert!(!t.reviewing());
    assert_eq!(t.text(), "x\n", "the command went to the shell, not the file");
}

#[gpui::test]
fn a_command_waits_in_the_terminal_with_its_note_above_and_its_keys_below(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho ran-$((40 + 2))\n</run>\n<say>\nCheck the sum.\n</say>\n";
    let mut t = open("x\n", &[run, DONE], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    t.ask("does it add up?");
    assert!(t.read(|e| e.command_proposed()));
    let draw = |t: &mut Setup| {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    };
    wait_for(&mut t, |e| e.terminal_context().is_some());
    draw(&mut t);
    let bounds = |t: &mut Setup, name: &'static str| t.cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} isn't drawn"));
    let (screen, note, keys) = (bounds(&mut t, "terminal-screen"), bounds(&mut t, "terminal-note"), bounds(&mut t, "terminal-keys"));
    assert!(note.bottom() <= keys.top(), "the note's above the keys: {note:?} {keys:?}");
    assert!(note.top() >= screen.top() && keys.bottom() <= screen.bottom(), "and both are on the screen: {note:?} {keys:?} {screen:?}");
    assert!(!t.read(|e| e.terminal_context().unwrap_or_default().contains("ran-42")), "it hasn't run");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "the keys are the story's");

    let screen = screen.center();
    t.cx.simulate_click(screen, gpui::Modifiers::none());
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "a click on the terminal goes in");
    t.keys("tab");
    assert!(!t.reviewing(), "Tab runs it from there too, and that was the last of it");
    assert_ne!(t.read(|e| e.modal.mode), Mode::Terminal, "out of the terminal, for what comes next");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ran-42")));
    assert!(!t.read(|e| e.command_proposed()), "once it's running it's the terminal's own");
}

#[gpui::test]
fn a_command_can_go_in_a_new_tab_and_a_ctrl_c_stops_what_the_story_started(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let says = |text: &str, e: &Editor| e.terminal_context().is_some_and(|screen| screen.lines().any(|line| line.trim() == text));
    let first = "<run>\nsleep 30; echo slept-through\n</run>\n<say>\nA long one.\n</say>\n";
    let second = "<run-tab>\necho in-a-tab-$((40 + 2))\n</run-tab>\n<say>\nBeside it.\n</say>\n";
    let stop = "<interrupt>\nthe long one\n</interrupt>\n<say>\nEnough.\n</say>\n";
    let then = "<run>\necho after-$((40 + 2))\n</run>\n<say>\nIt's free again.\n</say>\n";
    let mut t = open("x\n", &[first, second, stop, then, DONE], cx);
    bind_tab_keys(&mut t);
    t.ask("go");
    t.keys("tab");
    assert_eq!(shells(&mut t), (vec![1], 0));
    wait_for(&mut t, |e| e.command_proposed());

    t.keys("tab");
    assert_eq!(shells(&mut t), (vec![1, 2], 1), "a new tab, in front");
    wait_for(&mut t, |e| says("in-a-tab-42", e));
    wait_for(&mut t, |e| e.command_proposed());
    assert_eq!(shells(&mut t), (vec![1, 2], 1), "a command in the new tab: it's the one the story last ran in");

    // The Ctrl-C goes where the story last ran.
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    let decided = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().iter().flat_map(|f| f.decided.clone()).collect::<Vec<_>>());
    assert!(decided.iter().any(|d| d == "ran in a new tab of the terminal: `echo in-a-tab-$((40 + 2))`"), "{decided:?}");
    assert!(decided.iter().any(|d| d == "pressed Ctrl-C in the terminal, to stop the long one"), "{decided:?}");
    t.keys("tab");
    wait_for(&mut t, |e| says("after-42", e));
}

#[gpui::test]
fn a_ctrl_c_stops_a_program_and_the_shell_is_free_again(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let says = |text: &str, e: &Editor| e.terminal_context().is_some_and(|screen| screen.lines().any(|line| line.trim() == text));
    let long = "<run>\nsleep 30; echo slept-through\n</run>\n<say>\nA long one.\n</say>\n";
    let stop = "<interrupt>\nthe sleep\n</interrupt>\n<say>\nEnough.\n</say>\n";
    let then = "<run>\necho free-$((40 + 2))\n</run>\n<say>\nIt's free again.\n</say>\n";
    let mut t = open("x\n", &[long, stop, then, DONE], cx);
    t.ask("go");
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    t.keys("tab");
    wait_for(&mut t, |e| says("free-42", e));
    assert!(!t.read(|e| says("slept-through", e)), "it was stopped, long before it was through");
}

#[gpui::test]
fn a_last_command_ends_the_story_and_then_tab_is_the_shells(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho hi\n</run>\n<say>\nCheck.\n</say>\n";
    let mut t = open("x\n", &[run, DONE], cx);
    t.ask("does it work?");
    t.keys("tab");
    assert!(!t.reviewing(), "it was the last thing to do");
    assert!(t.read(|e| e.reading() && e.modal.mode == Mode::Normal), "the keys stay on the terminal, in normal mode");
    t.keys("escape");
    assert!(!t.read(|e| e.reading()), "Esc goes back to the code");

    t.keys("space t");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "no story waiting: Tab completes in the shell");
}

/// The ⌘ keys, as the app binds them.
fn bind_tab_keys(t: &mut Setup) {
    use crate::actions::{CloseTab, MoveTabLeft, MoveTabRight, NewTab, NextTab, PrevTab};
    t.cx.update(|_, cx| {
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-t", NewTab, Some("Editor")),
            gpui::KeyBinding::new("cmd-w", CloseTab, Some("Editor")),
            gpui::KeyBinding::new("cmd-shift-]", NextTab, Some("Editor")),
            gpui::KeyBinding::new("cmd-shift-[", PrevTab, Some("Editor")),
            gpui::KeyBinding::new("cmd-alt-left", MoveTabLeft, Some("Editor")),
            gpui::KeyBinding::new("cmd-alt-right", MoveTabRight, Some("Editor")),
        ])
    });
}

/// The terminal's tabs by number, and which is in front.
fn shells(t: &mut Setup) -> (Vec<usize>, usize) {
    t.read(|e| (e.terminal.shells.iter().map(|shell| shell.number).collect(), e.terminal.active))
}

fn wait_for(t: &mut Setup, what: impl Fn(&Editor) -> bool) {
    for _ in 0..3000 {
        if t.read(&what) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        t.cx.run_until_parked();
    }
    panic!("never happened; the terminal says {:?}", t.read(|e| e.terminal_context()));
}

#[gpui::test]
fn the_terminal_has_tabs_and_the_tab_keys_are_theirs_in_terminal_mode(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    bind_tab_keys(&mut t);
    let files = t.read(|e| e.project.tabs.len());
    t.keys("space t");
    assert_eq!(shells(&mut t), (vec![1], 0));
    t.keys("cmd-t cmd-t");
    assert_eq!(shells(&mut t), (vec![1, 2, 3], 2), "a new one opens right of the one in front, in front");
    t.keys("cmd-shift-]");
    assert_eq!(shells(&mut t), (vec![1, 2, 3], 0), "around the end");
    t.keys("cmd-shift-[");
    assert_eq!(shells(&mut t), (vec![1, 2, 3], 2));
    t.keys("cmd-alt-left");
    assert_eq!(shells(&mut t), (vec![1, 3, 2], 1), "it moves, and it's still in front");
    t.keys("cmd-alt-left cmd-alt-left");
    assert_eq!(shells(&mut t), (vec![3, 1, 2], 0), "and stops at the end");

    t.keys("e c h o space o n l y - h e r e enter");
    let printed = |e: &Editor| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "only-here"));
    wait_for(&mut t, printed);
    t.keys("cmd-shift-]");
    assert!(!t.read(printed), "each is a shell of its own, and Claude reads the one in front");

    t.keys("cmd-w");
    assert_eq!(shells(&mut t), (vec![3, 2], 1), "⌘W closes the shell in front");
    assert_eq!(t.read(|e| e.project.tabs.len()), files, "the file tabs are left alone");
    t.keys("e x i t enter");
    wait_for(&mut t, |e| e.terminal.shells.len() == 1);
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (true, Mode::Terminal), "one of two exits: the other is still here");
    t.keys("cmd-w");
    assert_eq!(t.read(|e| (e.terminal.shown, e.modal.mode)), (false, Mode::Normal), "the last goes: so does the pane");

    t.keys("cmd-t");
    assert_eq!(t.read(|e| e.project.tabs.len()), files + 1, "out of the terminal ⌘T is a file tab");
}

#[gpui::test]
fn a_tab_of_the_terminal_is_drawn_and_dragged_and_the_screen_keeps_its_room(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    bind_tab_keys(&mut t);
    t.keys("space t");
    let draw = |t: &mut Setup| {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    };
    draw(&mut t);
    assert!(t.cx.debug_bounds("shell-tab-0").is_none(), "one shell: no tabs");
    let alone = t.cx.debug_bounds("terminal-screen").expect("the screen is drawn").size.height;
    t.keys("cmd-t cmd-t");
    draw(&mut t);
    let beside = t.cx.debug_bounds("terminal-screen").expect("the screen is drawn").size.height;
    assert!(beside > gpui::px(100.) && beside < alone, "the tabs take their room from the screen: {beside:?} of {alone:?}");

    let (from, to) = (t.cx.debug_bounds("shell-tab-0").unwrap().center(), t.cx.debug_bounds("shell-tab-2").unwrap().center());
    t.cx.simulate_mouse_down(from, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_move(gpui::point(from.x + gpui::px(10.), from.y), gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_move(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.simulate_mouse_up(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    t.cx.run_until_parked();
    assert_eq!(shells(&mut t), (vec![2, 3, 1], 1), "dragged to the end, and shell 3 is still in front");

    draw(&mut t);
    let second = t.cx.debug_bounds("shell-tab-0").unwrap().center();
    t.cx.simulate_click(second, gpui::Modifiers::none());
    assert_eq!(shells(&mut t), (vec![2, 3, 1], 0), "a click brings it to the front");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal);
}

/// A wheel event at `at`, with the window drawn before and after.
fn wheel(t: &mut Setup, at: gpui::Point<gpui::Pixels>, lines: f32) {
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    t.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Lines(gpui::point(0., lines)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
}

#[gpui::test]
fn the_wheel_scrolls_while_a_step_waits_even_over_what_folds_out(cx: &mut TestAppContext) {
    let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let new: String = (0..8).map(|i| format!("new {i}")).collect::<Vec<_>>().join("\n");
    let mut t = open(&text, &[&edit("doc.txt", "line 150", &new), DONE], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(700.)));
    t.ask("change it");
    let top = |t: &mut Setup| t.read(|e| e.tab().unwrap().view.scroll_top);
    // The step is centered in the window: this goes down through it, whichever lines it's on.
    for y in (60..640).step_by(20) {
        let before = top(&mut t);
        wheel(&mut t, gpui::point(gpui::px(500.), gpui::px(y as f32)), -1.);
        assert!(top(&mut t) > before, "the wheel at y={y} didn't scroll");
    }
    assert_eq!(t.current(), Some(0), "and the step is still waiting");
}

#[gpui::test]
fn the_wheel_scrolls_the_terminal_and_the_code_while_a_command_waits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let run = "<run>\nseq 1 200\n</run>\n<say>\nCount.\n</say>\n";
    let mut t = open(&text, &[run, DONE], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    t.ask("count");
    t.keys("tab");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "200")));
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    let screen = t.cx.debug_bounds("terminal-screen").expect("the screen is drawn").center();
    let shown = |t: &mut Setup| t.read(|e| e.terminal.current().unwrap().screen().cells.iter().map(|c| c.c).take(20).collect::<String>());
    let before = shown(&mut t);
    wheel(&mut t, screen, 10.);
    assert_ne!(shown(&mut t), before, "back through the scrollback");
    let top = t.read(|e| e.tab().unwrap().view.scroll_top);
    wheel(&mut t, gpui::point(gpui::px(500.), gpui::px(120.)), -5.);
    assert!(t.read(|e| e.tab().unwrap().view.scroll_top) > top, "and the code from there");
}

#[gpui::test]
fn a_program_is_debugged_from_the_editor(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let found = std::process::Command::new("xcrun").args(["--find", "lldb-dap"]).output();
    if !found.is_ok_and(|out| out.status.success()) {
        return eprintln!("no lldb-dap: skipped");
    }
    let source =
        "#include <stdio.h>\nint main(void) {\n    int first = 40;\n    int second = first + 2;\n    printf(\"got %d\\n\", second);\n    return 3;\n}\n";
    let mut t = open("x\n", &[], cx);
    let dir = t.dir.canonicalize().unwrap();
    let (file, program) = (dir.join("main.c"), dir.join("main"));
    std::fs::write(&file, source).unwrap();
    if !std::process::Command::new("cc").args(["-g", "-O0", "-o"]).arg(&program).arg(&file).status().is_ok_and(|status| status.success()) {
        return eprintln!("no C compiler: skipped");
    }
    let editor = t.editor.clone();
    t.cx.update(|_, cx| editor.update(cx, |e, cx| e.open_paths(vec![file.clone()], cx)));
    t.cx.run_until_parked();

    t.command("over");
    assert!(t.status().is_some_and(|s| s.contains("nothing's being debugged")), "{:?}", t.status());
    // On `int first = 40;`.
    t.keys("g g j j");
    t.command("break");
    assert_eq!(t.read(|e| e.breakpoints_in(&file).cloned()), Some([2].into()));
    t.command("break");
    assert_eq!(t.read(|e| e.breakpoints_in(&file).cloned()), Some([].into()), "again takes it off");
    t.command("break");

    t.command("debug main");
    let stopped_on = |e: &Editor| e.stopped_line(&file);
    wait_for(&mut t, |e| stopped_on(e) == Some(2));
    assert_eq!(t.read(|e| e.tab().unwrap().cursor_position().0), 2, "the cursor went there");
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    assert!(t.cx.debug_bounds("debug-pane").is_some(), "and the pane is drawn");

    t.command("over");
    wait_for(&mut t, |e| stopped_on(e) == Some(3));
    let local = |e: &Editor, name: &str| e.debug.session.as_ref()?.stopped.as_ref()?.locals.iter().find(|v| v.name == name).map(|v| v.value.clone());
    assert_eq!(t.read(|e| local(e, "first")).as_deref(), Some("40"), "the variables as they are now");

    t.command("continue");
    wait_for(&mut t, |e| e.debug.session.is_none());
    assert_eq!(t.status().as_deref(), Some("the program ended with 3"));
    assert_eq!(t.text(), source, "and the file is as it was");
}

#[gpui::test]
fn a_command_takes_the_place_of_what_the_user_had_typed(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho ran-$((40 + 2))\n</run>\n<say>\nCheck the sum.\n</say>\n";
    let mut t = open("x\n", &[run, DONE], cx);
    t.keys("space t");
    let cursor = |e: &Editor| e.terminal.current().and_then(|t| t.screen().cursor).map(|c| (c.line, c.col));
    wait_for(&mut t, |e| cursor(e).is_some_and(|(_, col)| col > 0));
    // The prompt has settled where it is.
    let mut last = None;
    while last != t.read(cursor) {
        last = t.read(cursor);
        std::thread::sleep(std::time::Duration::from_millis(300));
        t.cx.run_until_parked();
    }
    t.keys("e c h o space h a l f");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.contains("echo half")));
    t.keys("escape escape");
    t.ask("does it add up?");
    assert!(t.read(|e| e.command_proposed()), "it's in the terminal");
    let from = t.read(|e| e.terminal.current().and_then(|t| t.typed_from()));
    assert_eq!(from, last, "drawn where the typing started, over it");
    t.keys("tab");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ran-42")));
    let screen = t.read(|e| e.terminal_context()).unwrap_or_default();
    assert!(!screen.contains("half"), "what was typed went, and the command ran on its own: {screen}");
}

#[gpui::test]
fn a_long_command_doesnt_hold_the_story_up(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\nsleep 30\n</run>\n<say>\nA long one.\n</say>\n";
    let mut t = open("x\n", &[run, &note("doc.txt", "x", "Meanwhile."), DONE], cx);
    t.ask("go");
    t.keys("tab");
    assert!(
        t.read(|e| matches!(e.story.as_ref().unwrap().story.as_ref().unwrap().current_item(), Some(ghostwrite_ai::story::Item::Note(_)))),
        "the next step's up while it runs"
    );
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.keys("tab");
    assert!(!t.reviewing());
}

#[gpui::test]
fn control_keys_reach_a_program_in_the_terminal(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space t");
    wait_for(&mut t, |e| e.terminal.current().and_then(|t| t.screen().cursor).is_some_and(|c| c.col > 0));
    std::thread::sleep(std::time::Duration::from_millis(500));
    t.keys("s t t y space - i x o n space r a w space - e c h o enter");
    std::thread::sleep(std::time::Duration::from_millis(300));
    t.keys("o d space - c enter");
    std::thread::sleep(std::time::Duration::from_millis(300));
    t.keys("ctrl-x ctrl-s ctrl-d ctrl-d");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.contains("030") && text.contains("023")));
}

#[gpui::test]
fn a_program_that_asks_for_kittys_keyboard_gets_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space t");
    wait_for(&mut t, |e| e.terminal.current().and_then(|t| t.screen().cursor).is_some_and(|c| c.col > 0));
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Ask for the protocol (flag 1), then show what the keys send.
    t.input("printf '\\033[>1u'; stty -ixon raw -echo; od -c").keys("enter");
    wait_for(&mut t, |e| e.terminal.current().is_some_and(|t| t.modes().keyboard == 1));
    std::thread::sleep(std::time::Duration::from_millis(300));
    t.keys("ctrl-x ctrl-d ctrl-d");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.contains("1   2   0   ;   5   u")));
}

/// A line `text` on the terminal.
fn printed(text: &'static str) -> impl Fn(&Editor) -> bool {
    move |e: &Editor| e.terminal_context().is_some_and(|screen| screen.lines().any(|line| line.trim() == text))
}

/// What's selected where the keys are reading.
fn read_selected(e: &Editor) -> String {
    let tab = e.keyed().unwrap();
    tab.doc.text()[tab.view.selection.bytes()].to_string()
}

#[gpui::test]
fn esc_esc_reads_the_terminal_with_the_editors_keys(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space t");
    t.keys("s e q space 3 space 5 enter");
    wait_for(&mut t, printed("5"));
    t.keys("escape escape");
    assert_eq!(t.read(|e| (e.modal.mode, e.reading())), (Mode::Normal, true), "reading it, in normal mode");

    t.keys("/").input("(?m)^4$").keys("enter");
    assert_eq!(t.read(read_selected), "4", "/ searches what's on it");
    t.keys("x y");
    assert_eq!(t.read(|e| e.registers.yanked.clone()), "4\n", "x takes the line, y yanks it");
    let marked = t.read(|e| e.terminal.current().unwrap().selected());
    assert!(marked.is_some_and(|text| text.starts_with('4')), "and the selection shows on the terminal");
    assert!(t.read(|e| e.reading_cursor()).is_some(), "with the cursor on it");

    t.keys("d");
    assert_eq!(t.read(|e| e.keyed().unwrap().doc.text().lines().filter(|l| l.trim() == "4").count()), 1, "it can't be edited");
    assert_eq!(t.text(), "x\n");

    t.keys("i");
    assert_eq!(t.read(|e| (e.modal.mode, e.reading())), (Mode::Terminal, false), "i types on the shell's line");
    t.keys("escape escape");
    assert!(t.read(|e| e.reading()), "and Esc Esc is back to reading");
    t.keys("escape");
    assert_eq!(t.read(|e| (e.modal.mode, e.reading(), e.terminal.shown)), (Mode::Normal, false, true), "Esc goes back to the code");
    assert_eq!(t.read(|e| e.terminal.current().unwrap().selected()), None, "and nothing's marked on the terminal");
}

#[gpui::test]
fn what_comes_in_while_reading_leaves_the_selection_on_its_text(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space t");
    t.keys("s e q space 3 space 5 enter");
    wait_for(&mut t, printed("5"));
    t.keys("escape escape");
    t.keys("/").input("(?m)^3$").keys("enter");
    t.keys("i");
    t.keys("s e q space 7 space 8 enter");
    wait_for(&mut t, printed("8"));
    t.keys("escape escape");
    t.keys("/").input("(?m)^7$").keys("enter");
    t.keys("enter");
    // Not a file: Enter says so, and the keys are still here.
    assert!(t.read(|e| e.reading()));
    // More printed while reading: the text grows under the selection.
    t.keys("i").keys("s e q space 9 space 9 enter");
    wait_for(&mut t, printed("9"));
    t.keys("escape escape");
    t.keys("/").input("(?m)^9$").keys("enter");
    let from = t.read(|e| e.keyed().unwrap().view.selection);
    let shell = t.read(|e| e.terminal.shells[e.terminal.active].number);
    t.cx.update(|_, cx| {
        t.editor.update(cx, |e, _| {
            e.terminal.current().unwrap().input(b"seq 11 12\r".to_vec());
        })
    });
    wait_for(&mut t, printed("12"));
    t.editor.update(t.cx, |e, _| e.read_again(shell));
    assert!(t.read(|e| e.reading()));
    let now = t.read(|e| e.keyed().unwrap().view.selection);
    assert_eq!(now, from, "the selection stays on its text");
    assert_eq!(t.read(read_selected), "9");
}

#[gpui::test]
fn enter_or_gf_on_the_terminal_opens_the_file_at_its_line(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("lib.rs"), "a\nb\nc\n").unwrap();
    t.keys("space t");
    t.keys("e c h o space l i b . r s : 3 enter");
    wait_for(&mut t, printed("lib.rs:3"));
    t.keys("escape escape");
    t.keys("/").input("(?m)^lib").keys("enter");
    t.keys("enter");
    assert_eq!(t.active().as_deref(), Some("lib.rs"));
    assert_eq!(t.cursor(), 4, "on its third line");
    assert!(!t.read(|e| e.reading()), "the keys are in the file");
    t.keys("space t escape escape");
    t.keys("/").input("(?m)^lib").keys("enter");
    t.keys("g f");
    assert_eq!(t.active().as_deref(), Some("lib.rs"), "gf does it too");
}

#[gpui::test]
fn a_command_proposed_is_reviewed_reading_the_terminal(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho ran\n</run>\n<say>\nCheck.\n</say>\n";
    let mut t = open("x\n", &[run, DONE], cx);
    t.ask("go");
    assert_eq!(t.read(|e| (e.modal.mode, e.reading())), (Mode::Ai, true), "AI mode, reading the terminal");
    wait_for(&mut t, |e| e.keyed().is_some_and(|tab| tab.view.selection.head > 0));
    let before = t.read(|e| e.keyed().unwrap().view.selection);
    t.keys("b");
    assert_ne!(t.read(|e| e.keyed().unwrap().view.selection), before, "the keys move through what's on it");
    assert_eq!(t.text(), "x\n");
    t.keys("tab");
    assert!(!t.reviewing(), "Tab ran it, the last of the story");
    assert_eq!(t.read(|e| (e.modal.mode, e.reading())), (Mode::Normal, true), "and the keys stay on the terminal, in normal mode");
}

#[gpui::test]
fn space_a_reading_the_terminal_asks_about_what_is_selected(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[&note("doc.txt", "x", "It's five."), DONE], cx);
    t.keys("space t");
    t.keys("s e q space 3 space 5 enter");
    wait_for(&mut t, printed("5"));
    t.keys("escape escape");
    t.keys("/").input("(?m)^5$").keys("enter");
    t.keys("space a").input("what is it?").keys("enter");
    let asked = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().instruction.clone());
    assert_eq!(asked, "About this, in the terminal:\n```\n5\n```\nwhat is it?");
}

#[gpui::test]
fn a_lone_esc_reaches_the_shell_and_esc_esc_doesnt_eat_the_next_key(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("x\n", &[], cx);
    t.keys("space t");
    t.keys("c a t space - v enter");
    std::thread::sleep(std::time::Duration::from_millis(500));
    t.keys("escape");
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        t.cx.executor().advance_clock(std::time::Duration::from_millis(100));
        t.cx.run_until_parked();
    }
    t.keys("x enter");
    wait_for(&mut t, printed("^[x"));
    t.keys("ctrl-c");
    t.keys("escape escape");
    assert!(t.read(|e| e.reading()), "two leave");
    t.keys("i e c h o space s a f e enter");
    wait_for(&mut t, printed("safe"));
}

#[test]
fn links_on_the_terminal_are_web_addresses_and_files_that_are_there() {
    use crate::editor::terminal::{LinkTo, link_in};
    let root = ghostwrite_project::scratch_dir("terminal-links");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let row = |text: &str| text.chars().enumerate().collect::<Vec<_>>();
    let line = row("see (https://example.com/a_(b)), then");
    let (cols, to) = link_in(&line, 10, &root).unwrap();
    assert_eq!(to, LinkTo::Web("https://example.com/a_(b)".into()), "the brackets it opens, not the one around it");
    assert_eq!(cols, 5..30);
    assert_eq!(link_in(&line, 2, &root), None, "a word that's neither");
    assert_eq!(link_in(&line, 3, &root), None, "a space");
    let line = row("error: --> src/main.rs:1:4: here");
    let (cols, to) = link_in(&line, 14, &root).unwrap();
    assert_eq!(to, LinkTo::File(root.join("src/main.rs"), 0, 3), "where the compiler said");
    assert_eq!(cols, 11..26, "the path and its numbers, not the colon after");
    assert_eq!(link_in(&row("src/nothere.rs"), 3, &root), None, "a file that isn't there isn't one");
}

#[gpui::test]
fn command_click_on_a_file_in_the_terminal_opens_it(cx: &mut TestAppContext) {
    let mut t = open("one\n", &[], cx);
    std::fs::write(t.dir.join("other.txt"), "a\nb\nc\n").unwrap();
    t.keys("space t").input("echo other.txt:3").keys("enter");
    // What echo printed: a line of its own starting with the name.
    wait_for(&mut t, |e| e.terminal.current().unwrap().screen().cells.iter().any(|c| c.c == 'o' && c.col == 0));
    // Where the printed line is on screen: its row, and the cell of its first char.
    let (row, origin, cw, lh) = t.read(|e| {
        let screen = e.terminal.current().unwrap().screen();
        let row = screen.cells.iter().filter(|c| c.c == 'o' && c.col == 0).map(|c| c.line).max().unwrap();
        let (origin, cw, lh) = e.terminal.cells.unwrap();
        (row, origin, cw, lh)
    });
    let at = gpui::point(origin.x + cw * 2.5, origin.y + lh * (row as f32 + 0.5));
    t.editor.update(t.cx, |e, cx| e.hover_link(at, true, cx));
    assert!(t.read(|e| e.terminal.link.as_ref().is_some_and(|link| link.row == row && link.cols == (0..11))), "⌘ underlines it");
    t.editor.update(t.cx, |e, cx| e.hover_link(at, false, cx));
    assert!(t.read(|e| e.terminal.link.is_none()), "and letting go doesn't");
    t.cx.simulate_mouse_down(at, gpui::MouseButton::Left, gpui::Modifiers::command());
    t.cx.run_until_parked();
    assert_eq!(t.active().as_deref(), Some("other.txt"));
    assert_eq!(t.cursor(), 4, "at its line");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Normal, "the keys on the code");
}
