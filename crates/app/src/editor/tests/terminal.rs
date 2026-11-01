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

#[gpui::test]
fn an_accepted_command_runs_in_the_terminal_and_claude_sees_what_it_printed(cx: &mut TestAppContext) {
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
    let running = t.read(|e| e.story.as_ref().and_then(|s| Some(s.story.as_ref()?.running(s.story.as_ref()?.current()?))));
    assert_eq!(running, Some(true), "and it's up until it's done");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Terminal, "the keyboard went with it");
    assert!(t.read(|e| e.terminal_hint()).is_some_and(|hint| hint.contains("⇥")), "and the terminal says what's next");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "Tab came back to the story");
    assert_eq!(t.read(|e| e.terminal_hint()), None);
    let followup = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups()[0].clone());
    assert_eq!(followup.decided, ["ran in the terminal: `echo ran-$((40 + 2))`"]);
    assert!(followup.terminal.is_some_and(|text| text.contains("ran-42")), "the next turn shows what it printed");
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
    assert!(t.read(|e| e.command_running()), "and Tab still runs it from there");
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
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());

    t.keys("tab");
    assert_eq!(shells(&mut t), (vec![1, 2], 1), "a new tab, in front");
    wait_for(&mut t, |e| says("in-a-tab-42", e));
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    assert_eq!(shells(&mut t), (vec![1, 2], 1), "a command in the new tab: it's the one the story last ran in");

    // The Ctrl-C goes where the story last ran.
    t.keys("tab");
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    t.keys("tab");
    wait_for(&mut t, |e| says("after-42", e));
    let decided = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().iter().flat_map(|f| f.decided.clone()).collect::<Vec<_>>());
    assert!(decided.iter().any(|d| d == "ran in a new tab of the terminal: `echo in-a-tab-$((40 + 2))`"), "{decided:?}");
    assert!(decided.iter().any(|d| d == "pressed Ctrl-C in the terminal, to stop the long one"), "{decided:?}");
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
    assert!(t.read(|e| e.command_running()));
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    t.cx.update(|window, _| window.refresh());
    t.cx.run_until_parked();
    t.keys("tab");
    assert!(t.read(|e| e.command_running()), "the Ctrl-C is up until Tab goes on from it");
    t.keys("tab");
    wait_for(&mut t, |e| e.command_proposed());
    t.keys("tab");
    wait_for(&mut t, |e| says("free-42", e));
    assert!(!t.read(|e| says("slept-through", e)), "it was stopped, long before it was through");
}

#[gpui::test]
fn out_of_the_terminal_tab_still_goes_on_and_without_a_story_tab_is_the_shells(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho hi\n</run>\n<say>\nCheck.\n</say>\n";
    let mut t = open("x\n", &[run, DONE], cx);
    t.ask("does it work?");
    t.keys("tab");
    t.keys("escape escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "esc esc: back in the code, still running");
    assert!(t.read(|e| e.command_running()));
    t.keys("tab");
    assert!(!t.read(|e| e.command_running()), "Tab goes on from there too");
    t.keys("tab");
    assert!(!t.reviewing());

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

#[gpui::test]
fn a_command_is_followed_back_to_the_shell_it_ran_in(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho ran-$((40 + 2))\n</run>\n<say>\nCheck.\n</say>\n";
    let mut t = open("x\n", &[run, &note("doc.txt", "x", "It printed 42."), DONE], cx);
    bind_tab_keys(&mut t);
    t.ask("does it add up?");
    t.keys("tab");
    wait_for(&mut t, |e| e.terminal_context().is_some_and(|text| text.lines().any(|l| l.trim() == "ran-42")));
    t.keys("cmd-t");
    assert_eq!(shells(&mut t), (vec![1, 2], 1));
    t.keys("tab");
    assert_eq!(shells(&mut t), (vec![1, 2], 0), "its shell is back in front");
    let followup = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups()[0].clone());
    assert!(followup.terminal.is_some_and(|text| text.contains("ran-42")), "so the model sees what it printed");
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
