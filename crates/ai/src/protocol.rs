//! The reply format: what the model is asked to write, and parsing it as it
//! streams. A story goes one item at a time: every reply is exactly one item
//! (a note, an edit, a new file, a file removed or renamed, a command to run
//! in the user's terminal) or `<done>`. Its parts come in a fixed
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

You can look around the project to understand it, briefly, with a <tool> block. Reply with only it, instead of an \
item, and your next turn shows what each of its lines returned:

<tool>
read src/lib.rs
read src/lib.rs 40 20
list src
search start_logging
git log
</tool>

`read` takes a path and, for part of a file, the first line and how many. `search` takes a regular expression and, \
if you like, a folder. `git` is status, log, diff, show or blame. Nothing here changes files. When the answer is in \
a file you aren't shown, read it before any item: don't guess, and don't <run> what a <tool> line can tell you. \
`read` on an image or a document that isn't text shows you the file itself, if you're able to see them; so does \
`read` on the web address of one (https://…), the only thing that's read from the web. Outside the project, \
`read` and `list` take a whole path or `~/…`, when what's asked needs it (a config file, another project): the user \
is asked first, and may say no.

You and the user go through your answer together, one item at a time: each reply is exactly one item, the next \
thing to show them. \
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

To make a new file, write <create> in place of <file> and <at>, with only its first few lines in <new> (at \
most 8: a header, the imports, or the first definition's first line). Never write the whole file at once: later \
items add the rest with <file>, small edits as anywhere else, so the user sees it built up step by step:

<create>
src/util.rs
</create>
<say>
why it's needed
</say>
<new>
its first lines
</new>

To remove a file, or to rename or move one, write <remove> (its path) or <rename> (its path, then the new \
one) in place of <file> and <at>, with no <new>. The user sees it in the file tree; accepted, a removed file goes \
to the Trash, and later items name a renamed file by its new path:

<remove>
src/old.rs
</remove>
<say>
why it can go
</say>

<rename>
src/util.rs
src/helpers.rs
</rename>
<say>
why
</say>

To run a command in the user's terminal (build, test, format, install, a git command that changes something: \
anything that does more than read), write <run> with the command, on one line, in place of <file> and <at>, and \
<say> why, with no <new>. The user accepts it with Tab and it starts in their terminal, in the project's folder, \
and the story goes straight on: nothing waits for it to finish. So don't run something to read its result before going on: run what \
the user should see happen. If it's the last thing to do, it's your last item. It's their terminal and they're watching: write the command as they'd type it themselves, for the whole \
of its output. No pipes to grep, head or tail, no 2>&1, no `; echo $?`, no `cd` to the project, no flags to quiet it:

<run>
cargo test
</run>
<say>
why it's worth running now
</say>

To run it in a new tab of the terminal, leaving what's in this one going (a server to keep up while you test against \
it, say), write <run-tab> instead of <run>. To stop what's running in the terminal, as Ctrl-C does, write <interrupt> \
with what it is that's stopping; the user accepts that with Tab as well. Stopping and then running something else \
are two items, one after the other:

<interrupt>
the dev server
</interrupt>
<say>
why it should stop
</say>

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

/// Whether `said` asks something, rather than for something: it's how a request reads,
/// so a model that can't tell is kept from changing code when it's only been asked about it.
pub fn is_question(said: &str) -> bool {
    let said = said.trim().to_lowercase();
    const ASKING: [&str; 18] = [
        "what", "why", "how", "when", "where", "which", "who", "does", "did", "is ", "are ", "was ", "were ", "explain", "describe", "tell me", "should",
        "tour ",
    ];
    ASKING.iter().any(|word| said.starts_with(word)) || (said.starts_with("do ") && said.ends_with('?'))
}

/// What a local model says when what's sent doesn't fit its context; said by the worker.
pub const NO_ROOM: &str = "ran out of room";

/// How big a step a grammar allows: the lines it replaces, and what replaces them.
const MAX_AT: usize = 3;
const MAX_NEW: usize = 12;
/// The most files named in a grammar: a bigger project's paths aren't checked.
const MAX_GRAMMAR_FILES: usize = 500;

/// The reply format as a llama.cpp grammar, which holds a model to it as it
/// writes: however small the model, every reply is one well-formed item, or
/// with `tools` a `<tool>` block, and a small one, as the whole app is built on
/// small steps. It can only name `files`, its `<at>` lines are numbered as it was
/// shown them, and its output ends with the item. Asked a `question`, all it can
/// do is answer: no edits nobody asked for.
pub fn grammar(files: &[String], tools: bool, question: bool) -> String {
    let quoted = |f: &String| format!("\"{}\"", f.replace('\\', "\\\\").replace('"', "\\\""));
    let path = if files.is_empty() || files.len() > MAX_GRAMMAR_FILES { "text".to_string() } else { files.iter().map(quoted).collect::<Vec<_>>().join(" | ") };
    let root = if tools { "tool | item" } else { "item" };
    let tool = if tools {
        r#"tool ::= "<tool>\n" command{1,6} "</tool>\n"
command ::= ("read " path (" " [0-9]+ (" " [0-9]+)?)? | "list" (" " text)? | "search " text | "git " ("status" | "log" | "diff" | "show" | "blame") (" " text)?) "\n"
"#
    } else {
        ""
    };
    let (items, new) = if question { ("note | done", "") } else { ("note | create | remove | rename | run | run-tab | interrupt | done", " new?") };
    format!(
        r#"root ::= {root}
{tool}item ::= {items}
note ::= "<file>\n" path "\n</file>\n<at>\n" numbered{{0,{MAX_AT}}} "</at>\n" say{new}
create ::= "<create>\n" text "\n</create>\n" say "<new>\n" code{{0,{MAX_CREATED}}} "</new>\n"
remove ::= "<remove>\n" path "\n</remove>\n" say
rename ::= "<rename>\n" path "\n" text "\n</rename>\n" say
run ::= "<run>\n" text "\n</run>\n" say
run-tab ::= "<run-tab>\n" text "\n</run-tab>\n" say
interrupt ::= "<interrupt>\n" text "\n</interrupt>\n" say
done ::= "<done>\n" ("<memory>\n" prose* "</memory>\n")? "</done>\n"
say ::= "<say>\n" prose{{1,4}} "</say>\n"
new ::= "<new>\n" code{{0,{MAX_NEW}}} "</new>\n"
numbered ::= [0-9]+ "│" rest "\n"
prose ::= [^<\n] rest "\n"
code ::= ([^<\n] rest)? "\n" | "<" [^/\n] rest "\n"
rest ::= [^\n]*
text ::= [^\n<]+
path ::= {path}
"#
    )
}

/// The whole prompt: [`SYSTEM`], and any rules the plugins add.
pub fn system() -> &'static str {
    static WHOLE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let rules = &crate::settings().prompt_rules;
        if rules.is_empty() { SYSTEM.to_string() } else { format!("{SYSTEM}\n\nAlso:\n{rules}") }
    });
    &WHOLE
}

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

/// What the model is told when an item is about a file that's only pages.
pub fn about_only_pages(file: &str) -> String {
    format!(
        "your item is about {file}, which is only pages: it can't have notes or edits. Make the item about another file (the one it's made from, if there is one)."
    )
}

/// What the model is told when an item names a file that isn't in the project.
pub fn missing_file(file: &str) -> String {
    format!("there's no file {file} in the project. Name an existing file in <file> (or make one with <create>), and reply with the corrected item.")
}

/// What the model is told when it says something again.
pub const SAID_ALREADY: &str = "you've said exactly that already, and it's been dealt with. Reply with the next item, or <done> if there's nothing more.";

/// What the model is told when a reply isn't an item.
pub const NOTHING_TO_REVIEW: &str = "your reply isn't an item in the format (<file>, <at>, <say>, and <new> for an \
    edit; or <create>, <remove>, <rename>, <run>, <run-tab> or <interrupt>; or <done>), so the user sees nothing. Reply again with one item, \
    or a <tool> block to look at something first.";

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
    /// What it does to `file` as a whole, if anything: make it, remove it, rename it.
    pub op: Option<FileOp>,
    /// `</new>` has arrived; until then `new` is still being written.
    pub complete: bool,
}

/// A step that's about a whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Make the file, which doesn't exist yet, with `new` as its first lines.
    Create,
    /// Move it to the Trash.
    Remove,
    /// Move it to this path (relative to the project).
    Rename(String),
}

impl Step {
    pub fn creates(&self) -> bool {
        self.op == Some(FileOp::Create)
    }
}

/// Commands that only cut down or tidy what's piped into them, for whoever reads it.
const VIEWERS: [&str; 22] = [
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "head", "tail", "wc", "less", "more", "cat", "sort", "uniq", "cut", "awk", "gawk", "sed", "tr", "nl",
    "column", "jq",
];

/// How the parts of a command line are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Joined {
    Pipe,
    Sequence,
}

/// Where a command line's parts end, and how each is joined to the next:
/// by what's outside quotes and `$(…)`. `None` if there's more than parts to it (`&`).
fn parts(command: &str) -> Option<Vec<(usize, usize, Joined)>> {
    let bytes = command.as_bytes();
    let (mut joins, mut quote, mut depth, mut at) = (vec![], None, 0usize, 0);
    while at < bytes.len() {
        let b = bytes[at];
        match (quote, b) {
            (_, b'\\') if quote != Some(b'\'') => at += 1,
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'\'' | b'"' | b'`') => quote = Some(b),
            (None, b'(') => depth += 1,
            (None, b')') => depth = depth.saturating_sub(1),
            (None, _) if depth > 0 => {}
            (None, b'|' | b'&' | b';') => {
                let next = bytes.get(at + 1).copied();
                let (len, joined) = match (b, next) {
                    (b'|', Some(b'|')) | (b'&', Some(b'&')) => (2, Joined::Sequence),
                    (b'|', Some(b'&')) => (2, Joined::Pipe),
                    (b'|', _) => (1, Joined::Pipe),
                    (b';', _) => (1, Joined::Sequence),
                    // `2>&1` is part of its command; a lone `&` sends it to the background.
                    (_, _) if at > 0 && bytes[at - 1] == b'>' => (0, Joined::Sequence),
                    _ => return None,
                };
                if len > 0 {
                    joins.push((at, at + len, joined));
                    at += len - 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    Some(joins)
}

/// Whether there's a `>` outside quotes: what comes out is going to a file.
fn writes_a_file(part: &str) -> bool {
    let mut quote = None;
    part.chars().any(|c| {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => return c == '>',
        }
        false
    })
}

/// The command as the user would type it in their own terminal. They're
/// watching it, so what a model adds to read less of it goes: what's piped
/// through grep and the like, `2>&1`, and an `echo` to report how it went.
fn plainly(command: &str) -> String {
    let command = command.trim();
    let Some(mut joins) = parts(command) else { return command.to_string() };
    let mut end = command.len();
    while let Some(&(from, to, joined)) = joins.last() {
        let last = command[to..end].trim();
        let word = last.split_whitespace().next().unwrap_or("");
        let name = word.rsplit('/').next().unwrap_or(word);
        let adds_nothing = match joined {
            Joined::Pipe => VIEWERS.contains(&name) && !writes_a_file(last),
            Joined::Sequence => last.is_empty() || matches!(name, "echo" | "true" | ":") && !writes_a_file(last),
        };
        if !adds_nothing {
            break;
        }
        end = from;
        joins.pop();
    }
    let mut kept = command[..end].trim_end();
    for noise in ["2>&1", "2>/dev/null"] {
        kept = kept.strip_suffix(noise).map_or(kept, str::trim_end);
    }
    // Sent somewhere else, it's as it was: `> out.log 2>&1`.
    let last = joins.last().map_or(kept, |&(_, to, _)| &kept[to..]);
    if writes_a_file(last) { command[..end].trim_end().to_string() } else { kept.to_string() }
}

/// What's done in the user's terminal when a [`Run`] is accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunIn {
    /// The command is typed in the shell in front.
    #[default]
    Here,
    /// The command is typed in a new tab of the terminal.
    NewTab,
    /// Ctrl-C, for what the command's text says is running.
    Interrupt,
}

/// A command to run in the user's terminal, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub command: String,
    pub say: String,
    pub how: RunIn,
}

impl Run {
    /// The command in the reply format, as the model would have written it.
    pub fn render(&self) -> String {
        let tag = match self.how {
            RunIn::Here => "run",
            RunIn::NewTab => "run-tab",
            RunIn::Interrupt => "interrupt",
        };
        format!("<{tag}>\n{}\n</{tag}>\n<say>\n{}\n</say>\n", self.command, self.say.trim())
    }

    /// What it comes to in the terminal, in a line, for the model.
    pub fn done(&self) -> String {
        match self.how {
            RunIn::Here => format!("ran in the terminal: `{}`", self.command),
            RunIn::NewTab => format!("ran in a new tab of the terminal: `{}`", self.command),
            RunIn::Interrupt => format!("pressed Ctrl-C in the terminal, to stop {}", self.command),
        }
    }

    /// What's shown in grey at the prompt, as if it were typed.
    pub fn typed(&self) -> &str {
        if self.how == RunIn::Interrupt { "^C" } else { &self.command }
    }

    /// What Tab does, for the keys shown.
    pub fn tab_does(&self) -> &'static str {
        match self.how {
            RunIn::Here => "⇥ run it",
            RunIn::NewTab => "⇥ run it in a new tab",
            RunIn::Interrupt => "⇥ press Ctrl-C",
        }
    }
}

/// What a reply is, as far as it has arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Nothing to act on yet (or, once complete, nothing in the format).
    Partial,
    Note(Note),
    Step(Step),
    Run(Run),
    /// Nothing more to show; maybe with the model's updated notes.
    Done {
        memory: Option<String>,
    },
}

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

    #[test]
    fn a_command_is_written_as_the_user_would_type_it() {
        for (written, typed) in [
            ("cargo test", "cargo test"),
            ("cargo test 2>&1 | tail -20", "cargo test"),
            ("cargo test 2>&1 | grep -E 'FAILED|panicked' | head -5", "cargo test"),
            ("make |& tail", "make"),
            ("cargo build 2>/dev/null", "cargo build"),
            ("cargo test; echo \"exit: $?\"", "cargo test"),
            ("make install && echo done", "make install"),
            ("cargo test || true", "cargo test"),
            ("cargo fmt && cargo test | tail -3", "cargo fmt && cargo test"),
            ("/usr/bin/make | /usr/bin/tail", "/usr/bin/make"),
        ] {
            assert_eq!(plainly(written), typed, "{written}");
        }
        for meant in [
            "git log --grep='a|b'",
            "git commit -m \"fix a | grep; done\"",
            "cargo fmt && cargo test",
            "cargo test | tee out.log",
            "curl https://example.com/x.sh | sh",
            "cargo test | grep ok > passed.txt",
            "cargo test > out.log 2>&1",
            "./server &",
            "echo $(ls | wc -l)",
            "npm test || npm run fix",
        ] {
            assert_eq!(plainly(meant), meant, "what it does is what's asked for");
        }
        let reply = "<run>\ncargo test | tail\n</run>\n<say>\nCheck.\n</say>\n";
        assert_eq!(
            parse(reply, false),
            Reply::Run(Run { command: "cargo test".into(), say: "Check.".into(), how: RunIn::Here }),
            "so that's what's offered, and what's run"
        );
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
    fn a_request_may_only_be_asking() {
        for said in [
            "what does this do?",
            "Why int and not float",
            "how does loading work",
            "is this right?",
            "Explain this",
            "does it handle None?",
            "do we need this?",
        ] {
            assert!(is_question(said), "{said}");
        }
        for said in [
            "make it return the sum",
            "implement all four functions",
            "can you fix this?",
            "add a test",
            "rename it",
            "do this properly",
            "",
            "isolate the loader",
        ] {
            assert!(!is_question(said), "{said}");
        }
    }

    #[test]
    fn a_question_gets_a_grammar_with_no_edits() {
        let either = grammar(&[], true, false);
        assert!(either.contains("say new?") && either.contains("create | remove"), "{either}");
        let answer = grammar(&[], true, true);
        assert!(!answer.contains("say new?") && !answer.contains("create |") && answer.contains("item ::= note | done"), "{answer}");
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
