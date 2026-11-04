//! Parsing a reply as it streams: the item in it so far, and whether it keeps
//! to the format.

use super::command::plainly;
use super::{FileOp, Note, Reply, Run, RunIn, Step};
use crate::lines;

/// The item in `reply`, once enough of it has arrived to show: a note once its
/// `<say>` is closed and nothing follows (`finished`: the reply is complete);
/// an edit once its `<new>` has begun.
pub fn parse(reply: &str, finished: bool) -> Reply {
    // Text before the first tag is let through (see [`check`]): what counts is which tag comes first.
    if first_tag(reply) == Some("<done>") {
        let memory = tagged_lines(reply, "<memory>", "</memory>").filter(|(_, closed)| *closed).map(|(lines, _)| lines.join("\n").trim().to_string());
        return Reply::Done { memory: memory.filter(|m| !m.is_empty()) };
    }
    let path = |open: &str, close: &str| {
        tagged_lines(reply, open, close)
            .filter(|(_, closed)| *closed)
            .and_then(|(lines, _)| lines.into_iter().map(|l| l.trim().to_string()).find(|l| !l.is_empty()))
    };
    // A whole file removed or renamed: ready once `<say>` is closed.
    let said = || tagged_lines(reply, "<say>", "</say>").filter(|(_, closed)| *closed).map(|(say, _)| say.join("\n").trim().to_string());
    let op =
        |file: String, op: FileOp, say: String| Reply::Step(Step { file: Some(file), old: vec![], line: None, new: vec![], say, op: Some(op), complete: true });
    if let Some(file) = path("<remove>", "</remove>") {
        return said().map_or(Reply::Partial, |say| op(file, FileOp::Remove, say));
    }
    // A command: ready once `<say>` is closed, too.
    if let Some(command) = path("<run>", "</run>") {
        return said().map_or(Reply::Partial, |say| Reply::Run(Run { command: plainly(&command), say, how: RunIn::Here }));
    }
    if let Some(command) = path("<run-tab>", "</run-tab>") {
        return said().map_or(Reply::Partial, |say| Reply::Run(Run { command: plainly(&command), say, how: RunIn::NewTab }));
    }
    if let Some(what) = path("<interrupt>", "</interrupt>") {
        return said().map_or(Reply::Partial, |say| Reply::Run(Run { command: what, say, how: RunIn::Interrupt }));
    }
    if let Some((paths, true)) = tagged_lines(reply, "<rename>", "</rename>") {
        let mut paths = paths.into_iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        let (Some(from), Some(to)) = (paths.next(), paths.next()) else { return Reply::Partial };
        return said().map_or(Reply::Partial, |say| op(from, FileOp::Rename(to), say));
    }
    let (file, at, create) = match path("<create>", "</create>") {
        Some(path) => (path, vec![], true),
        None => {
            let Some(file) = path("<file>", "</file>") else { return Reply::Partial };
            match tagged_lines(reply, "<at>", "</at>") {
                Some((at, true)) => (file, at, false),
                _ => return Reply::Partial,
            }
        }
    };
    let Some((say, true)) = tagged_lines(reply, "<say>", "</say>") else { return Reply::Partial };
    let say = say.join("\n").trim().to_string();
    let (at, line) = lines::unnumbered(&at);
    match tagged_lines(reply, "<new>", "</new>") {
        // New lines are code; numbers copied onto them by mistake come off too.
        Some((new, complete)) => {
            let op = create.then_some(FileOp::Create);
            let new = lines::unnumbered(&new).0;
            // What changes nothing isn't an edit, only what it says.
            if complete && !create && new == at {
                return Reply::Note(Note { file: Some(file), at, text: say, line });
            }
            Reply::Step(Step { file: Some(file), old: at, line, new, say, op, complete })
        }
        None if finished && !create => Reply::Note(Note { file: Some(file), at, text: say, line }),
        None => Reply::Partial,
    }
}

/// Where a reply is in the format, line by line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// An item's first tag (see [`STARTS`]) (blank lines before it are fine).
    Start,
    /// The path line inside `<file>`, `<create>`, `<remove>` or `<rename>`.
    Path(&'static str),
    /// The new path inside `<rename>`.
    To,
    /// The closing tag after the path.
    Close(&'static str),
    /// `<at>`, after `</file>`.
    At,
    /// Lines up to the closing tag; `need` lines at least.
    Lines { close: &'static str, need: usize },
    /// `<say>`.
    Say,
    /// `<say>`, or `<new>` before it (small models write them either way round).
    SayOrNew,
    /// `<new>`, or the end of a note.
    New,
    /// `<memory>` or `</done>`.
    InDone,
    /// Nothing more.
    End,
}

/// Whether `reply`, as far as it has arrived, keeps to the format: its tags in
/// order, each closed before the next opens, and nothing else. Text before the
/// first tag is let through (a word before looking at files); the line still
/// being written isn't judged yet. `finished`: the reply is complete, so it
/// must have ended where an item can.
pub fn check(reply: &str, finished: bool) -> Result<(), String> {
    let start = first_tag(reply).and_then(|tag| find_tag(reply, tag)).unwrap_or(reply.len());
    let body = &reply[start..];
    let complete = if finished { body } else { &body[..body.rfind('\n').map_or(0, |i| i + 1)] };
    let mut expect = Expect::Start;
    let mut seen = 0;
    // A removal, rename or command: no `<new>` after its `<say>`.
    let mut no_new = false;
    let mut creating = false;
    // `<new>` came before `<say>`.
    let mut new_first = false;
    for line in complete.lines() {
        let tag = line.trim_end();
        expect = match (expect, tag) {
            (Expect::Start | Expect::End, "") => expect,
            (Expect::Start, "<file>") => Expect::Path("</file>"),
            (Expect::Start, "<create>") => Expect::Path("</create>"),
            (Expect::Start, "<remove>") => Expect::Path("</remove>"),
            (Expect::Start, "<rename>") => Expect::Path("</rename>"),
            (Expect::Start, "<run>") => Expect::Path("</run>"),
            (Expect::Start, "<run-tab>") => Expect::Path("</run-tab>"),
            (Expect::Start, "<interrupt>") => Expect::Path("</interrupt>"),
            (Expect::Path("</rename>"), path) if !path.trim().is_empty() && !path.starts_with('<') => Expect::To,
            (Expect::To, path) if !path.trim().is_empty() && !path.starts_with('<') => Expect::Close("</rename>"),
            (Expect::Start, "<done>") => Expect::InDone,
            (Expect::Path(close), path) if !path.trim().is_empty() && !path.starts_with('<') => Expect::Close(close),
            (Expect::Close("</file>"), "</file>") => Expect::At,
            (Expect::Close("</create>"), "</create>") => {
                creating = true;
                Expect::SayOrNew
            }
            (Expect::Close("</remove>"), "</remove>")
            | (Expect::Close("</rename>"), "</rename>")
            | (Expect::Close("</run>"), "</run>")
            | (Expect::Close("</run-tab>"), "</run-tab>")
            | (Expect::Close("</interrupt>"), "</interrupt>") => {
                no_new = true;
                Expect::Say
            }
            (Expect::At, "<at>") => lines(&mut seen, "</at>", 0),
            (Expect::Say | Expect::SayOrNew, "<say>") => lines(&mut seen, "</say>", 1),
            (Expect::SayOrNew, "<new>") => {
                new_first = true;
                lines(&mut seen, "</new>", 0)
            }
            (Expect::New, "<new>") => lines(&mut seen, "</new>", 0),
            (Expect::InDone, "<memory>") => lines(&mut seen, "</memory>", 0),
            (Expect::InDone, "</done>") => Expect::End,
            (Expect::Lines { close, need }, line) if line.trim_end() == close => {
                if seen < need {
                    return Err(format!("{close} came with nothing before it"));
                }
                match close {
                    "</at>" => Expect::SayOrNew,
                    "</new>" if new_first => Expect::Say,
                    "</say>" if no_new || new_first => Expect::End,
                    "</say>" => Expect::New,
                    "</memory>" => Expect::InDone,
                    _ => Expect::End,
                }
            }
            // Code and prose lines; a line starting `</` can only be the closing tag.
            (Expect::Lines { close, .. }, line) if line.starts_with("</") => return Err(format!("`{line}` came where {close} or more lines were expected")),
            (Expect::Lines { close: "</new>", .. }, _) if creating && seen == MAX_CREATED => {
                return Err(format!(
                    "a new file starts with at most {MAX_CREATED} lines in <new>; later items add the rest to it with <file>, one small edit at a time"
                ));
            }
            (Expect::Lines { .. }, _) => {
                seen += 1;
                expect
            }
            (expected, line) => return Err(format!("`{line}` came where {} was expected", describe(expected))),
        };
    }
    match expect {
        _ if !finished => Ok(()),
        Expect::New | Expect::End => Ok(()),
        Expect::Start => Err("there's no item in it".into()),
        expected => Err(format!("it ended where {} was expected", describe(expected))),
    }
}

/// The tags an item (or `<done>`) starts with.
pub(crate) const STARTS: [&str; 8] = ["<file>", "<create>", "<remove>", "<rename>", "<run>", "<run-tab>", "<interrupt>", "<done>"];

/// The tag the reply's item starts with, if it's arrived.
fn first_tag(reply: &str) -> Option<&'static str> {
    STARTS.iter().filter_map(|t| Some((find_tag(reply, t)?, *t))).min().map(|(_, t)| t)
}

/// The most lines a new file starts with: the rest comes in edits after.
pub const MAX_CREATED: usize = 8;

/// Lines up to `close` begin, `need` of them at least.
fn lines(seen: &mut usize, close: &'static str, need: usize) -> Expect {
    *seen = 0;
    Expect::Lines { close, need }
}

fn describe(expect: Expect) -> String {
    match expect {
        Expect::Start => "<file>, <create>, <remove>, <rename>, <run>, <run-tab>, <interrupt> or <done>".into(),
        Expect::Path("</run>" | "</run-tab>") => "the command".into(),
        Expect::Path("</interrupt>") => "what's being stopped".into(),
        Expect::Path(_) => "a path".into(),
        Expect::To => "the new path".into(),
        Expect::Close(close) => close.into(),
        Expect::At => "<at>".into(),
        Expect::Lines { close, .. } => close.into(),
        Expect::Say => "<say>".into(),
        Expect::SayOrNew => "<say> or <new>".into(),
        Expect::New => "<new> or the end".into(),
        Expect::InDone => "<memory> or </done>".into(),
        Expect::End => "the end".into(),
    }
}

/// The lines between `open` and `close` tag lines; `None` until `open` has
/// arrived, and the partial lines so far until `close` has.
fn tagged_lines(text: &str, open: &str, close: &str) -> Option<(Vec<String>, bool)> {
    let start = find_tag(text, open)? + open.len();
    let body = text[start..].strip_prefix('\n').unwrap_or(&text[start..]);
    let (body, closed) = match find_tag(body, close) {
        Some(end) => (&body[..end], true),
        // Hide a closing tag that's only partly written.
        None => (strip_partial_tag(body, close), false),
    };
    let body = if closed { body.strip_suffix('\n').unwrap_or(body) } else { body };
    let lines = if body.is_empty() && closed { vec![] } else { body.split('\n').map(str::to_string).collect() };
    Some((lines, closed))
}

/// Where `tag` stands on a line of its own in `text` (or at the end, still
/// streaming in). A tag mentioned inside a line of code is just text.
fn tag_positions(text: &str, tag: &str) -> Vec<usize> {
    text.match_indices(tag)
        .map(|(i, _)| i)
        .filter(|&i| {
            let before = text[..i].trim_end_matches([' ', '\t']);
            let after = text[i + tag.len()..].trim_start_matches([' ', '\t', '\r']);
            (before.is_empty() || before.ends_with('\n')) && (after.is_empty() || after.starts_with('\n'))
        })
        .collect()
}

fn find_tag(text: &str, tag: &str) -> Option<usize> {
    tag_positions(text, tag).first().copied()
}

/// `text` without a prefix of `tag` at its end (a tag still streaming in).
fn strip_partial_tag<'a>(text: &'a str, tag: &str) -> &'a str {
    let cut = (1..tag.len()).rev().find(|&n| text.ends_with(&tag[..n])).unwrap_or(0);
    &text[..text.len() - cut]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn the_terminal_can_get_a_new_tab_and_a_ctrl_c() {
        for how in [RunIn::NewTab, RunIn::Interrupt] {
            let run = Run { command: "make serve".into(), say: "Why.".into(), how };
            let reply = run.render();
            assert_eq!(parse(&reply, true), Reply::Run(run), "{reply}");
            assert_eq!(check(&reply, true), Ok(()), "{reply}");
            assert!(check(&format!("{reply}<new>\nx\n</new>\n"), true).is_err(), "it has no <new>");
        }
        let written = "<run-tab>\ncargo test | tail\n</run-tab>\n<say>\nCheck.\n</say>\n";
        assert_eq!(
            parse(written, false),
            Reply::Run(Run { command: "cargo test".into(), say: "Check.".into(), how: RunIn::NewTab }),
            "written as you'd type it there too"
        );
        let stop = "<interrupt>\nthe server | tail\n</interrupt>\n<say>\nStop.\n</say>\n";
        assert_eq!(
            parse(stop, false),
            Reply::Run(Run { command: "the server | tail".into(), say: "Stop.".into(), how: RunIn::Interrupt }),
            "that's words, not a command"
        );
    }

    const NOTE: &str = "<file>\nsrc/lib.rs\n</file>\n<at>\npub fn run() {\n</at>\n<say>\nIt starts here.\n</say>\n";
    const EDIT: &str = "<file>\nsrc/lib.rs\n</file>\n<at>\n    a - b\n</at>\n<say>\nIt should add.\n</say>\n<new>\n    a + b\n</new>\n";

    #[test]
    fn an_edit_that_changes_nothing_is_a_note() {
        let same = "<file>\na.py\n</file>\n<at>\n1│x = 1\n</at>\n<say>\nIt sets x.\n</say>\n<new>\nx = 1\n</new>\n";
        assert!(matches!(parse(same, true), Reply::Note(note) if note.text == "It sets x."), "{same}");
        let real = same.replace("<new>\nx = 1", "<new>\nx = 2");
        assert!(matches!(parse(&real, true), Reply::Step(_)));
        assert!(matches!(parse(same.trim_end_matches("</new>\n"), false), Reply::Step(_)), "it isn't known until it ends");
    }

    #[test]
    fn a_command_shows_once_its_reason_is_closed() {
        let run = "<run>\ncargo test -p core\n</run>\n<say>\nThe tests cover it.\n</say>\n";
        assert_eq!(parse(&run[..30], false), Reply::Partial);
        assert_eq!(parse(run, false), Reply::Run(Run { command: "cargo test -p core".into(), say: "The tests cover it.".into(), how: RunIn::Here }));
        assert_eq!(check(run, true), Ok(()));
        assert!(check(&format!("{run}<new>\nx\n</new>\n"), true).is_err(), "a command has no <new>");
        assert_eq!(
            parse(&Run { command: "make".into(), say: "Build it.".into(), how: RunIn::Here }.render(), true),
            Reply::Run(Run { command: "make".into(), say: "Build it.".into(), how: RunIn::Here })
        );
    }

    #[test]
    fn new_before_say_is_taken_too() {
        let edit = "<file>\nsrc/lib.rs\n</file>\n<at>\n    a - b\n</at>\n<new>\n    a + b\n</new>\n<say>\nIt should add.\n</say>\n";
        assert_eq!(check(edit, true), Ok(()));
        let Reply::Step(step) = parse(edit, true) else { panic!("{:?}", parse(edit, true)) };
        assert_eq!((step.new, step.say), (lines(&["    a + b"]), "It should add.".into()));
        assert!(check(&format!("{edit}<new>\nx\n</new>\n"), true).is_err(), "but one <new>");
    }

    #[test]
    fn done_after_a_word_is_done() {
        let reply = "Let me look.\n<done>\n</done>\n";
        assert_eq!(check(reply, true), Ok(()));
        assert_eq!(parse(reply, true), Reply::Done { memory: None });
    }

    #[test]
    fn a_note_shows_once_the_reply_is_complete() {
        assert_eq!(parse(&NOTE[..60], false), Reply::Partial);
        assert_eq!(parse(NOTE, false), Reply::Partial, "an edit's <new> may still come");
        assert_eq!(
            parse(NOTE, true),
            Reply::Note(Note { file: Some("src/lib.rs".into()), at: lines(&["pub fn run() {"]), text: "It starts here.".into(), line: None })
        );
    }

    #[test]
    fn an_edit_shows_as_its_new_lines_stream() {
        let streaming = &EDIT[..EDIT.len() - 12];
        let Reply::Step(step) = parse(streaming, false) else { panic!("{:?}", parse(streaming, false)) };
        assert!(!step.complete);
        assert_eq!(step.old, lines(&["    a - b"]));
        assert_eq!(step.say, "It should add.");
        let Reply::Step(step) = parse(EDIT, true) else { panic!() };
        assert!(step.complete && step.op.is_none());
        assert_eq!(step.new, lines(&["    a + b"]));
    }

    #[test]
    fn quoted_numbers_say_where_to_look() {
        let reply = "<file>\nsrc/lib.rs\n</file>\n<at>\n12│    a - b\n13│}\n</at>\n<say>\nAdd.\n</say>\n<new>\n12│    a + b\n13│}\n</new>\n";
        let Reply::Step(step) = parse(reply, true) else { panic!() };
        assert_eq!((step.old, step.line), (lines(&["    a - b", "}"]), Some(11)));
        assert_eq!(step.new, lines(&["    a + b", "}"]), "numbers copied onto new lines come off");
        let note = "<file>\nsrc/lib.rs\n</file>\n<at>\n3│fn run() {\n</at>\n<say>\nHere.\n</say>\n";
        let Reply::Note(note) = parse(note, true) else { panic!() };
        assert_eq!((note.at.clone(), note.line), (lines(&["fn run() {"]), Some(2)));
        assert!(note.render().contains("<at>\n3│fn run() {\n</at>"), "and go back on when it's shown to the model again");
    }

    #[test]
    fn a_new_file_is_an_edit_that_creates_it() {
        let reply = "<create>\nsrc/util.rs\n</create>\n<say>\nA home for helpers.\n</say>\n<new>\npub fn one() {}\n</new>\n";
        let Reply::Step(step) = parse(reply, true) else { panic!() };
        assert!(step.creates() && step.complete && step.old.is_empty());
        assert_eq!(step.file.as_deref(), Some("src/util.rs"));
    }

    #[test]
    fn done_may_carry_notes() {
        assert_eq!(parse("<done>\n</done>\n", true), Reply::Done { memory: None });
        assert_eq!(parse("<done>\n<memory>\nPrefers tabs.\n</memory>\n</done>\n", true), Reply::Done { memory: Some("Prefers tabs.".into()) });
    }

    #[test]
    fn a_file_can_be_removed_or_renamed() {
        let remove = "<remove>\nold.rs\n</remove>\n<say>\nNothing uses it.\n</say>\n";
        let Reply::Step(step) = parse(remove, false) else { panic!() };
        assert_eq!((step.file.as_deref(), step.op, step.complete), (Some("old.rs"), Some(FileOp::Remove), true), "ready once <say> is closed");
        let rename = "<rename>\nsrc/util.rs\nsrc/helpers.rs\n</rename>\n<say>\nA clearer name.\n</say>\n";
        let Reply::Step(step) = parse(rename, false) else { panic!() };
        assert_eq!((step.file.as_deref(), step.op), (Some("src/util.rs"), Some(FileOp::Rename("src/helpers.rs".into()))));
        assert_eq!(parse("<rename>\nsrc/util.rs\n</rename>\n<say>\nx\n</say>\n", true), Reply::Partial, "both paths");
        assert_eq!(check(remove, true), Ok(()));
        assert_eq!(check(rename, true), Ok(()));
        assert!(check("<rename>\nsrc/util.rs\n</rename>\n", false).is_err(), "the new path is missing");
        assert!(check(&format!("{remove}<new>\nx\n</new>\n"), true).is_err(), "nothing follows a removal");
    }

    #[test]
    fn the_format_is_checked_line_by_line() {
        assert_eq!(check(EDIT, true), Ok(()));
        assert_eq!(check(NOTE, true), Ok(()));
        assert_eq!(check("Let me look.\n<done>\n</done>\n", true), Ok(()), "a word before the first tag is let through");
        assert_eq!(check(&EDIT[..30], false), Ok(()), "half an item is fine while it streams");
        assert!(check("<file>\na.rs\n</file>\n<say>\n", false).unwrap_err().contains("<at>"), "a part left out");
        assert!(check("<file>\na.rs\n</file>\n<at>\nx\n</at>\n<say>\n</say>\n", false).is_err(), "<say> needs words");
        assert!(check("<file>\na.rs\n</file>\n<at>\nx\n</file>\n", false).is_err(), "a stray closing tag");
        assert!(check(&format!("{EDIT}And here's why.\n"), false).is_err(), "nothing after the item");
        assert!(check("<file>\na.rs\n</file>\n<at>\nx\n", true).unwrap_err().contains("ended"), "cut off");
        assert!(check("```\n<file>\n", false).is_ok() && check("<file>\n```\n</file>\n", false).is_ok(), "a path is any line");
        let create = |n: usize| format!("<create>\nb.rs\n</create>\n<say>\nx\n</say>\n<new>\n{}", "line\n".repeat(n));
        assert_eq!(check(&format!("{}</new>\n", create(MAX_CREATED)), true), Ok(()), "a new file starts small");
        assert!(check(&create(MAX_CREATED + 1), false).unwrap_err().contains("one small edit at a time"), "not all at once, caught as it streams");
    }

    #[test]
    fn tags_inside_code_are_just_text() {
        let reply = "<file>\na.rs\n</file>\n<at>\nlet s = \"<new>\";\n</at>\n<say>\nx\n</say>\n<new>\nlet s = \"</new>\";\n</new>\n";
        let Reply::Step(step) = parse(reply, true) else { panic!() };
        assert_eq!(step.old, lines(&["let s = \"<new>\";"]));
        assert_eq!(step.new, lines(&["let s = \"</new>\";"]));
    }
}
