//! What the model is told: the prompt, the grammar that holds a local model to
//! the format, and what it's told when a reply goes wrong.

use std::ops::Range;

use super::MAX_CREATED;
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
and your next item is asked for once it's done (or has run for a while: a server, a watcher, which keep going), with what it \
printed in the terminal. Run what the user should see happen; if it was the last thing to do, reply <done> once you've seen how it went. It's their terminal and they're watching: write the command as they'd type it themselves, for the whole \
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
