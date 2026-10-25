//! The reply format: what the model is asked to write, and parsing it as it
//! streams. A story goes one item at a time: every reply is exactly one item
//! (a note, an edit, or a new file) or `<done>`. Its parts come in a fixed
//! order, each closed before the next opens, so an item can be checked part by
//! part as it arrives. Quoted lines may carry the numbers they were shown with
//! ([`crate::lines`]); those come off here, as where to look.

use std::ops::Range;

use crate::lines;

pub const SYSTEM: &str = "You are an assistant inside a text editor. The user points at some lines of a file and asks \
a question or asks for a change.

Files are shown to you with their lines numbered: each line is its number, │, and its text. The lines the user \
points at are between <cursor> and </cursor>, which aren't in the file:

<contents path=\"src/lib.rs\">
1│pub fn run() {
<cursor>
2│    start();
</cursor>
3│}
</contents>

You can look around the project to understand it, briefly: your tools read, search and list, and the shell runs \
read-only commands (ls, cat, head, grep, git log and the like). You can't change files that way. You and the user \
go through your answer together, one item at a time: each reply is exactly one item, the next thing to show them. \
They review it, then ask for the next one, telling you what they did with it or what they said.

An item is a place in the code and what you say about it:

<file>
src/lib.rs
</file>
<at>
2│    start();
</at>
<say>
one to three short sentences
</say>

That is a note. A question (explain, why, how, is this right, a tour of the code) is answered with notes, one \
per reply, in the order you'd walk someone through the code, across files as needed. To change code, add what \
replaces the <at> lines:

<new>
    start_logging();
    start();
</new>

That is an edit, and its <say> says why. Make each edit small, about one line: the next thing the user would \
type, or one line changed. The user accepts it with Tab and it's applied at once, so your next item sees the \
file with it. Order edits the way the user would make them, usually a definition before its uses.

A change often spans files: a function and its callers, a type and where it's built, code and its tests. Make it \
file by file, each edit in the file its lines are in, named in <file>: when one file has what it needs (or what \
the next depends on), go on to the next. Don't stop at the file the user pointed at while other files still need \
changing, and read a file before you edit it.

To make a new file, write <create> in place of <file> and <at>, with the file's first lines in <new>; later \
items add to it with <file>:

<create>
src/util.rs
</create>
<say>
why it's needed
</say>
<new>
its first lines
</new>

Rules:
- <file> is always there: the path of the file the item is in, relative to the project, of a file that exists.
- <at> is consecutive lines copied from that file as it is now, each with its number as shown (`2│` and then \
the text), the text exactly, indentation included, never shortened or paraphrased. One to three lines are \
usually enough. A note about the file as a whole leaves <at> empty.
- <new> is code as it will be in the file, without numbers.
- To insert, put a neighbouring line in <at> and repeat it in <new> before or after the new lines. An empty \
<new> deletes the <at> lines.
- Each tag on its own line, in this order, and nothing else in the reply: no markdown, no code fences, no \
narration. Never copy the <cursor> tags.
- When there's nothing more to show or change, reply with only:

<done>
</done>

You keep notes about this project and this user between sessions; they're shown to you in <memory>. If you \
learned something worth knowing next time (a preference, a convention, a decision), your <done> may carry the \
complete updated notes, condensed, under 300 words:

<done>
<memory>
the notes
</memory>
</done>

When the user says something (a question, a correction, a change of direction), your next item follows it: \
answer with a note, or change course. What they accepted is already in the files.";

/// Bytes of a file shown when an item's lines aren't found in it.
const MISPLACED_WINDOW: usize = 12_000;

/// What the model is told when an item doesn't place: its `<at>` lines
/// (`quoted`, looked for near the lines `near`) aren't in `file`, whose text
/// is `text`. It's shown the file around where they should be.
pub fn misplaced(file: &str, text: &str, quoted: &[String], near: Range<usize>) -> String {
    let closest = lines::closest(text, quoted);
    let like = match &closest {
        Some(l) if l.len() == 1 => format!(" (the most like them is line {})", l.start + 1),
        Some(l) => format!(" (the most like them are lines {}-{})", l.start + 1, l.end),
        None => String::new(),
    };
    let shown = crate::context::contents(file, text, closest.unwrap_or(near), MISPLACED_WINDOW, false);
    format!(
        "your item's <at> lines aren't in {file} as it is now{like}. Copy them from {file} as shown here, each with its \
         number and the text exactly, and reply with the corrected item.\n\n{shown}"
    )
}

/// What the model is told when an item names a file that isn't in the project.
pub fn missing_file(file: &str) -> String {
    format!("there's no file {file} in the project. Name an existing file in <file> (or make one with <create>), and reply with the corrected item.")
}

/// What the model is told when a reply isn't an item.
pub const NOTHING_TO_REVIEW: &str = "your reply isn't an item in the format (<file>, <at>, <say>, and <new> for an \
    edit; or <create>; or <done>), so the user sees nothing. Reply again with one item.";

/// One note: the file and lines it's about (no lines: the file as a whole),
/// and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub file: Option<String>,
    pub at: Vec<String>,
    pub text: String,
    /// Where `at` starts (zero-based), as its numbers (or the editor, for a
    /// note it gives) said: where to look for it.
    pub line: Option<usize>,
}

impl Note {
    /// The note in the reply format, as the model would have written it.
    pub fn render(&self) -> String {
        let file = self.file.as_ref().map_or(String::new(), |f| format!("<file>\n{f}\n</file>\n"));
        let at: String = match self.line {
            Some(first) => self.at.iter().zip(first + 1..).map(|(l, n)| format!("{n}{}{l}\n", lines::SEP)).collect(),
            None => self.at.iter().map(|l| format!("{l}\n")).collect(),
        };
        format!("{file}<at>\n{at}</at>\n<say>\n{}\n</say>\n", self.text.trim())
    }
}

/// One edit: whole lines out, whole lines in, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The file it's in, relative to the project; `None`: the file asked about.
    pub file: Option<String>,
    pub old: Vec<String>,
    /// Where `old` starts (zero-based), as its numbers said: where to look for it.
    pub line: Option<usize>,
    pub new: Vec<String>,
    pub say: String,
    /// It makes `file`, which doesn't exist yet.
    pub create: bool,
    /// `</new>` has arrived; until then `new` is still being written.
    pub complete: bool,
}

/// What a reply is, as far as it has arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Nothing to act on yet (or, once complete, nothing in the format).
    Partial,
    Note(Note),
    Step(Step),
    /// Nothing more to show; maybe with the model's updated notes.
    Done {
        memory: Option<String>,
    },
}

/// The item in `reply`, once enough of it has arrived to show: a note once its
/// `<say>` is closed and nothing follows (`finished`: the reply is complete);
/// an edit once its `<new>` has begun.
pub fn parse(reply: &str, finished: bool) -> Reply {
    if reply.trim_start().starts_with("<done>") {
        let memory = tagged_lines(reply, "<memory>", "</memory>").filter(|(_, closed)| *closed).map(|(lines, _)| lines.join("\n").trim().to_string());
        return Reply::Done { memory: memory.filter(|m| !m.is_empty()) };
    }
    let path = |open: &str, close: &str| {
        tagged_lines(reply, open, close)
            .filter(|(_, closed)| *closed)
            .and_then(|(lines, _)| lines.into_iter().map(|l| l.trim().to_string()).find(|l| !l.is_empty()))
    };
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
        Some((new, complete)) => Reply::Step(Step { file: Some(file), old: at, line, new: lines::unnumbered(&new).0, say, create, complete }),
        None if finished && !create => Reply::Note(Note { file: Some(file), at, text: say, line }),
        None => Reply::Partial,
    }
}

/// Where a reply is in the format, line by line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// `<file>`, `<create>` or `<done>` (blank lines before it are fine).
    Start,
    /// The path line inside `<file>` or `<create>`.
    Path(&'static str),
    /// The closing tag after the path.
    Close(&'static str),
    /// `<at>`, after `</file>`.
    At,
    /// Lines up to the closing tag; `need` lines at least.
    Lines { close: &'static str, need: usize },
    /// `<say>`.
    Say,
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
    let start = ["<file>", "<create>", "<done>"].iter().filter_map(|t| find_tag(reply, t)).min().unwrap_or(reply.len());
    let body = &reply[start..];
    let complete = if finished { body } else { &body[..body.rfind('\n').map_or(0, |i| i + 1)] };
    let mut expect = Expect::Start;
    let mut seen = 0;
    for line in complete.lines() {
        let tag = line.trim_end();
        expect = match (expect, tag) {
            (Expect::Start | Expect::End, "") => expect,
            (Expect::Start, "<file>") => Expect::Path("</file>"),
            (Expect::Start, "<create>") => Expect::Path("</create>"),
            (Expect::Start, "<done>") => Expect::InDone,
            (Expect::Path(close), path) if !path.trim().is_empty() && !path.starts_with('<') => Expect::Close(close),
            (Expect::Close("</file>"), "</file>") => Expect::At,
            (Expect::Close("</create>"), "</create>") => Expect::Say,
            (Expect::At, "<at>") => lines(&mut seen, "</at>", 0),
            (Expect::Say, "<say>") => lines(&mut seen, "</say>", 1),
            (Expect::New, "<new>") => lines(&mut seen, "</new>", 0),
            (Expect::InDone, "<memory>") => lines(&mut seen, "</memory>", 0),
            (Expect::InDone, "</done>") => Expect::End,
            (Expect::Lines { close, need }, line) if line.trim_end() == close => {
                if seen < need {
                    return Err(format!("{close} came with nothing before it"));
                }
                match close {
                    "</at>" => Expect::Say,
                    "</say>" => Expect::New,
                    "</memory>" => Expect::InDone,
                    _ => Expect::End,
                }
            }
            // Code and prose lines; a line starting `</` can only be the closing tag.
            (Expect::Lines { close, .. }, line) if line.starts_with("</") => return Err(format!("`{line}` came where {close} or more lines were expected")),
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

/// Lines up to `close` begin, `need` of them at least.
fn lines(seen: &mut usize, close: &'static str, need: usize) -> Expect {
    *seen = 0;
    Expect::Lines { close, need }
}

fn describe(expect: Expect) -> String {
    match expect {
        Expect::Start => "<file>, <create> or <done>".into(),
        Expect::Path(_) => "a path".into(),
        Expect::Close(close) => close.into(),
        Expect::At => "<at>".into(),
        Expect::Lines { close, .. } => close.into(),
        Expect::Say => "<say>".into(),
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

/// Word-wrap prose to `cols` columns, keeping its blank lines and indentation.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(20);
    let mut rows = vec![];
    for line in text.trim_end().split('\n') {
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let mut row = indent.clone();
        for word in line.split_whitespace() {
            let len = row.chars().count();
            if len > indent.len() && len + 1 + word.chars().count() > cols {
                rows.push(std::mem::replace(&mut row, indent.clone()));
            }
            if row.chars().count() > indent.len() {
                row.push(' ');
            }
            row.push_str(word);
        }
        rows.push(row);
    }
    rows
}

/// A code line wrapped to `cols`: at a space where there is one, never
/// touching the spacing inside it; continuations indent one level deeper.
pub fn wrap_code(line: &str, cols: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= cols {
        return vec![line.to_string()];
    }
    let indent = chars.iter().take_while(|c| **c == ' ').count();
    let more = " ".repeat(indent + 4);
    let mut rows = vec![];
    let mut rest: &[char] = &chars;
    let mut prefix = String::new();
    while prefix.chars().count() + rest.len() > cols {
        let room = cols.saturating_sub(prefix.chars().count()).max(8).min(rest.len());
        let lead = if rows.is_empty() { indent } else { 0 };
        let cut = rest[..room].iter().rposition(|c| *c == ' ').filter(|&i| i > lead).map_or(room, |i| i + 1);
        rows.push(format!("{prefix}{}", rest[..cut].iter().collect::<String>().trim_end()));
        rest = &rest[cut..];
        prefix.clone_from(&more);
    }
    rows.push(format!("{prefix}{}", rest.iter().collect::<String>()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    const NOTE: &str = "<file>\nsrc/lib.rs\n</file>\n<at>\npub fn run() {\n</at>\n<say>\nIt starts here.\n</say>\n";
    const EDIT: &str = "<file>\nsrc/lib.rs\n</file>\n<at>\n    a - b\n</at>\n<say>\nIt should add.\n</say>\n<new>\n    a + b\n</new>\n";

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
        assert!(step.complete && !step.create);
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
        assert!(step.create && step.complete && step.old.is_empty());
        assert_eq!(step.file.as_deref(), Some("src/util.rs"));
    }

    #[test]
    fn done_may_carry_notes() {
        assert_eq!(parse("<done>\n</done>\n", true), Reply::Done { memory: None });
        assert_eq!(parse("<done>\n<memory>\nPrefers tabs.\n</memory>\n</done>\n", true), Reply::Done { memory: Some("Prefers tabs.".into()) });
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
    }

    #[test]
    fn tags_inside_code_are_just_text() {
        let reply = "<file>\na.rs\n</file>\n<at>\nlet s = \"<new>\";\n</at>\n<say>\nx\n</say>\n<new>\nlet s = \"</new>\";\n</new>\n";
        let Reply::Step(step) = parse(reply, true) else { panic!() };
        assert_eq!(step.old, lines(&["let s = \"<new>\";"]));
        assert_eq!(step.new, lines(&["let s = \"</new>\";"]));
    }

    #[test]
    fn wraps_prose_and_code() {
        assert_eq!(wrap("one two three four", 20), ["one two three four"]);
        assert_eq!(wrap("aaaa bbbb cccc dddd eeee ffff\n\n    code(x)", 20), ["aaaa bbbb cccc dddd", "eeee ffff", "", "    code(x)"]);
        assert_eq!(wrap_code("    short", 40), ["    short"]);
        let rows = wrap_code("    result = compute(alpha, beta, gamma, delta)", 28);
        assert_eq!(rows, ["    result = compute(alpha,", "        beta, gamma, delta)"]);
        assert_eq!(wrap_code("abcdefghijklmnopqrstuvwxyz0123456789", 30).len(), 2);
    }
}
