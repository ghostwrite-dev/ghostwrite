//! A real program, in the real debugger that comes with Xcode's tools. It's skipped
//! where there's neither a C compiler nor `lldb-dap`.

use std::collections::{BTreeSet, HashMap};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ghostwrite_dap::{Debugger, Event};

const SOURCE: &str = "#include <stdio.h>\n\nint twice(int n) {\n    int doubled = n * 2;\n    return doubled;\n}\n\nint main(void) {\n    int start = 21;\n    int result = twice(start);\n    printf(\"got %d\\n\", result);\n    return 3;\n}\n";

fn adapter() -> Option<String> {
    let out = Command::new("xcrun").args(["--find", "lldb-dap"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn until(events: &Receiver<Event>, wanted: impl Fn(&Event) -> bool) -> Event {
    loop {
        let event = events.recv_timeout(Duration::from_secs(30)).expect("the debugger said what was expected in time");
        if wanted(&event) {
            return event;
        }
    }
}

#[test]
fn a_program_stops_where_asked_and_can_be_looked_at_and_stepped() {
    let Some(adapter) = adapter() else { return eprintln!("no lldb-dap: skipped") };
    let dir = std::env::temp_dir().join(format!("ghostwrite-dap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The real path: on macOS the temporary folder is reached through a symlink, and the debug info has what was compiled.
    let dir = dir.canonicalize().unwrap();
    let (source, program) = (dir.join("main.c"), dir.join("main"));
    std::fs::write(&source, SOURCE).unwrap();
    let built = Command::new("cc").args(["-g", "-O0", "-o"]).arg(&program).arg(&source).status();
    if !built.is_ok_and(|status| status.success()) {
        return eprintln!("no C compiler: skipped");
    }
    let source = source.canonicalize().unwrap();

    let (tell, events) = channel();
    let debugger = Debugger::start(&[adapter], move |event| drop(tell.send(event))).unwrap();
    // The line `int doubled = n * 2;`.
    let breakpoints = HashMap::from([(source.clone(), BTreeSet::from([3]))]);
    debugger.launch(&program, &[], &dir, &breakpoints).unwrap();

    let Event::Stopped { reason, thread } = until(&events, |event| matches!(event, Event::Stopped { .. })) else { unreachable!() };
    assert_eq!(reason, "breakpoint");
    let frames = debugger.frames(thread).unwrap();
    assert_eq!(frames[0].name.split('(').next(), Some("twice"), "{frames:?}");
    assert_eq!(frames[0].line, Some(3));
    assert_eq!(frames[0].path.as_ref().and_then(|path| path.canonicalize().ok()).as_ref(), Some(&source));
    assert!(frames.iter().any(|frame| frame.name.starts_with("main")), "and what called it");
    let locals = debugger.locals(frames[0].id).unwrap();
    assert_eq!(locals.iter().find(|variable| variable.name == "n").map(|variable| variable.value.as_str()), Some("21"), "{locals:?}");

    debugger.go("next", thread).unwrap();
    let Event::Stopped { thread, .. } = until(&events, |event| matches!(event, Event::Stopped { .. })) else { unreachable!() };
    let frames = debugger.frames(thread).unwrap();
    assert_eq!(frames[0].line, Some(4), "one line on");
    let locals = debugger.locals(frames[0].id).unwrap();
    assert_eq!(locals.iter().find(|variable| variable.name == "doubled").map(|variable| variable.value.as_str()), Some("42"));

    debugger.go("continue", thread).unwrap();
    assert_eq!(until(&events, |event| matches!(event, Event::Exited(_))), Event::Exited(Some(3)), "it ran to its end");
    std::fs::remove_dir_all(&dir).ok();
}
