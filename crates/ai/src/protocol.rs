//! The reply format: what Claude is asked to write, and parsing it as it
//! streams. Steps and notes appear as soon as enough of them has arrived to
//! act on; [`locate`] and [`change`] place a step in the text.

use std::ops::Range;

pub const SYSTEM: &str = "You are an assistant inside a text editor. The user points at some lines of their \
file, marked with <cursor> tags, and either asks a question or asks for a change. Decide which it is.

You can read the project with your tools (read, search, list) to understand it: look at what the request \
needs, briefly, then reply. You can't change files with tools. Every change, in any file, is a step in your \
reply, which the user reviews and accepts one at a time. Don't narrate what you're doing; reply only in the \
format below.

If it's a question (explain, why, what, how, is this right, a tour of the code, ...), don't change anything. \
Answer as a few short notes, each pinned to the lines it talks about, in the order you'd walk someone through \
the code; the user jumps from note to note, across files:

<answer>
<note>
<file>
src/other.rs
</file>
<at>
the exact existing lines this note is about, copied exactly, usually one or two
</at>
one to three sentences
</note>
</answer>

Notes go wherever the code they explain is: when the answer involves other files (a tour, how something \
flows, where a value comes from), read them and pin notes there, naming the file with <file> (relative to the \
project). Leave out <file> for notes in the marked file, and <at> for a note about the marked lines as a whole. Be brief and direct, plain text without \
markdown headings, bold or bullets. Put a short code snippet on its own lines, indented four spaces, \
without fences.

If it asks for a change, make only that change, matching the surrounding style and indentation, as steps \
(and no <answer>). A change can span files: when it needs edits elsewhere in the project (a caller, an import, \
a test, a definition), make them too, reading each file first so its <old> lines are exact, and name the file \
in those steps (see <file> below).

Your steps are shown to the user like autocomplete suggestions, one at a time, and accepted with Tab, so \
each step is about one line: the next line they would type, or one line changed. Use two or three lines only \
for something that can't be split, like a short closing block. Write the steps in the order the user would \
type them, top to bottom. A step may build on the steps before it: its <old> can be a line an earlier step \
added. Each step is one edit of whole lines:

<step>
<old>
the existing lines to replace, copied exactly, enough of them to be unique
</old>
<new>
the lines that replace them
</new>
</step>

A step in a file other than the marked one names it, relative to the project:

<step>
<file>
src/other.rs
</file>
<old>
the existing lines there
</old>
<new>
what replaces them
</new>
</step>

Order steps across files the way the user would make the change: usually the definition before its uses. \
Put each tag on its own line. To insert, include one neighbouring existing line in <old> and repeat it in \
<new>, followed or preceded by the new lines. An empty <old> means the marked lines themselves. Never copy the \
<cursor> tags into a step. Reply with steps only: no explanation, no markdown fences.

If the change needs a new file or folder, propose it as its own step, in the order it would be made, with a \
path relative to the project (end it with / for a folder):

<create>
src/util.rs
</create>

Then write the new file's contents as steps in that file, a line or two at a time, naming it the same way:

<step>
<file>
src/util.rs
</file>
<old>
</old>
<new>
its first lines
</new>
</step>

In another file an empty <old> means: append at the end. Leave out <file> for steps in the marked file.

To delete a file or folder the change makes unnecessary, propose it the same way:

<remove>
src/old_util.rs
</remove>

To rename or move one, the old path then the new:

<rename>
src/util.rs
src/helpers.rs
</rename>

Steps after a rename name the file by its new path.

You keep notes about this project and this user between sessions; they're shown to you in <memory>, \
along with the user's recent requests. The notes are yours. When you learn something worth knowing next \
time (a preference, a convention of the codebase, a decision, what the user is working toward), end your \
reply with the complete updated notes, condensed: merge, rewrite and drop what no longer matters, and keep \
them under 300 words.

<memory>
the notes
</memory>

Most replies don't need it.

The user reviews your reply one step or note at a time, and can say something at any point: a question, a \
comment, a correction, a change of direction. Then they'll tell you where they are and what they said. Reply in \
two parts, both optional:

<answer>
what answers them now, shown right where they are: notes (or steps for something to add right there)
</answer>
<rest>
the new plan from where they are on, in the same format as a first reply: steps, file operations, or an \
<answer> with notes
</rest>

The <rest> replaces the step they're at (or, at a note they've read, what follows it) and everything after it, \
so include all of what's still to come. Whenever what they said changes what you'd do next (a correction, a \
redirect, a different approach or focus), the new plan goes in <rest>, not in the first part: corrected steps \
outside <rest> would come in addition to the old ones, not instead of them. Leave <rest> out only if everything \
after stays exactly as it was; an empty <rest></rest> drops it all.";

/// What Claude is told when a step doesn't apply: its `<old>` lines aren't
/// in `file`, shown as it is (`text`) unless it's the file asked about,
/// which every follow-up shows anyway.
pub fn misplaced_step(file: &str, text: Option<&str>) -> String {
    let mut said = format!(
        "your step's <old> lines aren't in {file} as it is now, so it can't be applied. Copy <old> exactly from the file as it \
         is (a step it builds on may have been skipped). Reply with only <rest>: the corrected step and everything after it."
    );
    if let Some(text) = text {
        let text = &text[..text.floor_char_boundary(crate::settings().context_bytes / 2)];
        said.push_str(&format!("\n\n{file} as it is now:\n<file>\n{text}\n</file>"));
    }
    said
}

/// What Claude is told when a reply has nothing in the format to review.
pub const NOTHING_TO_REVIEW: &str = "there's nothing in your reply in the reply format (no <step>, file operation or <answer> \
    with notes), so the user sees nothing. Reply again, in the format.";

/// What Claude made of the request: steps that edit, or an answer that doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Edit,
    Question,
}

/// Which kind of reply this is, once its first tag has arrived.
pub fn reply_kind(reply: &str) -> Option<Kind> {
    // From the first tag: Claude may say a word before looking at files.
    let answer = find_tag(reply, "<answer>");
    let edit = ["<step>", "<create>", "<remove>", "<rename>"].iter().filter_map(|t| find_tag(reply, t)).min();
    match (answer, edit) {
        (Some(a), Some(e)) => Some(if a < e { Kind::Question } else { Kind::Edit }),
        (Some(_), None) => Some(Kind::Question),
        (None, Some(_)) => Some(Kind::Edit),
        (None, None) => None,
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

/// A follow-up's reply in its two parts: what answers the user now, and
/// the revised rest of the plan (or walkthrough), once `<rest>` has begun.
pub fn split_rest(reply: &str) -> (String, Option<String>) {
    let Some(start) = find_tag(reply, "<rest>") else { return (reply.to_string(), None) };
    let inside = &reply[start + "<rest>".len()..];
    let (rest, after) = match find_tag(inside, "</rest>") {
        Some(end) => (&inside[..end], &inside[end + "</rest>".len()..]),
        None => (strip_partial_tag(inside, "</rest>"), ""),
    };
    (format!("{}{after}", &reply[..start]), Some(rest.to_string()))
}

/// Whether a follow-up's reply can no longer change what comes next: its
/// `<rest>` is complete, or Claude has moved on to its `<memory>` notes.
pub fn rest_settled(reply: &str) -> bool {
    find_tag(reply, "</rest>").is_some() || find_tag(reply, "<memory>").is_some()
}

/// The answer so far, without its tags.
pub fn answer_text(reply: &str) -> String {
    tagged_lines(reply, "<answer>", "</answer>").map(|(lines, _)| lines.join("\n").trim_end().to_string()).unwrap_or_default()
}

/// Claude's updated notes, once the `<memory>` block is complete.
pub fn memory_update(reply: &str) -> Option<String> {
    let (lines, closed) = tagged_lines(reply, "<memory>", "</memory>")?;
    closed.then(|| lines.join("\n").trim().to_string()).filter(|notes| !notes.is_empty())
}

/// One note of an answer: the file and lines it's about (none: the marked
/// file, and the marked lines), and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub file: Option<String>,
    pub at: Vec<String>,
    pub text: String,
}

/// The answer's notes so far; a note appears once its `<file>` and `<at>`
/// are complete, and its text grows as it streams. An answer without notes:
/// each paragraph is one.
pub fn parse_notes(reply: &str) -> Vec<Note> {
    let answer = answer_text(reply);
    if find_tag(&answer, "<note>").is_none() {
        return answer.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).map(|p| Note { file: None, at: vec![], text: p.to_string() }).collect();
    }
    tag_positions(&answer, "<note>")
        .into_iter()
        .map(|at| {
            let rest = &answer[at + "<note>".len()..];
            &rest[..find_tag(rest, "<note>").unwrap_or(rest.len())]
        })
        .filter_map(|chunk| {
            let chunk = &chunk[..find_tag(chunk, "</note>").unwrap_or(chunk.len())];
            // Each tag, once it's closed, and where the text after it starts.
            let mut body_from = 0;
            let mut tag = |open: &str, close: &str| -> Option<Option<Vec<String>>> {
                if find_tag(chunk, open).is_none() {
                    return Some(None);
                }
                let (lines, closed) = tagged_lines(chunk, open, close)?;
                if !closed {
                    return None;
                }
                body_from = body_from.max(find_tag(chunk, close)? + close.len());
                Some(Some(lines))
            };
            let file = tag("<file>", "</file>")?.and_then(|lines| lines.into_iter().map(|l| l.trim().to_string()).find(|l| !l.is_empty()));
            let at = tag("<at>", "</at>")?.unwrap_or_default();
            Some(Note { file, at, text: strip_partial_tag(&chunk[body_from..], "</note>").trim().to_string() })
        })
        .collect()
}

/// One step of a change: whole lines out, whole lines in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The file it's in, relative to the project; `None`: the file being edited.
    pub file: Option<String>,
    pub old: Vec<String>,
    pub new: Vec<String>,
    /// `</new>` has arrived; until then `new` is still being written.
    pub complete: bool,
}

/// A piece of a change, in the order Claude wrote them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Step(Step),
    /// A new file, or a folder if the path ends in `/`.
    Create(String),
    /// A file or folder to move to the Trash.
    Remove(String),
    /// A file or folder to move to a new path.
    Rename {
        from: String,
        to: String,
    },
}

/// The pieces of a change so far. A step shows up once its `<old>` is
/// complete; a file operation once its path lines are.
pub fn parse_pieces(reply: &str) -> Vec<Piece> {
    let mut starts: Vec<(usize, &str)> =
        ["<step>", "<create>", "<remove>", "<rename>"].iter().flat_map(|tag| tag_positions(reply, tag).into_iter().map(move |i| (i, *tag))).collect();
    starts.sort();
    starts
        .iter()
        .enumerate()
        .filter_map(|(n, &(at, tag))| {
            let end = starts.get(n + 1).map_or(reply.len(), |(next, _)| *next);
            let chunk = &reply[at..end];
            let close = format!("</{}", &tag[1..]);
            match tag {
                "<rename>" => {
                    let (lines, done) = tagged_lines(chunk, tag, &close)?;
                    let mut paths = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty());
                    let (from, to) = (paths.next()?, paths.next()?);
                    (done || lines.len() > 2).then(|| Piece::Rename { from: from.to_string(), to: to.to_string() })
                }
                "<create>" | "<remove>" => {
                    let (lines, done) = tagged_lines(chunk, tag, &close)?;
                    let path = lines.iter().map(|l| l.trim()).find(|l| !l.is_empty())?.to_string();
                    (done || lines.len() > 1).then(|| if tag == "<create>" { Piece::Create(path) } else { Piece::Remove(path) })
                }
                _ => {
                    let file = tagged_lines(chunk, "<file>", "</file>")
                        .filter(|(_, done)| *done)
                        .and_then(|(lines, _)| lines.into_iter().map(|l| l.trim().to_string()).find(|l| !l.is_empty()));
                    let (old, old_done) = tagged_lines(chunk, "<old>", "</old>")?;
                    if !old_done {
                        return None;
                    }
                    let after_old = &chunk[find_tag(chunk, "</old>")?..];
                    let (new, complete) = tagged_lines(after_old, "<new>", "</new>").unwrap_or((vec![], false));
                    Some(Piece::Step(Step { file, old, new, complete }))
                }
            }
        })
        .collect()
}

/// Where `old` is in `text`, as a range of whole lines: the occurrence
/// nearest the lines `near`, ignoring trailing spaces (failing that,
/// indentation). An empty `old` is `near` itself.
pub fn locate(text: &str, old: &[String], near: Range<usize>) -> Option<Range<usize>> {
    if old.is_empty() {
        return Some(near);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let find = |same: &dyn Fn(&str, &str) -> bool| {
        (0..lines.len().saturating_sub(old.len() - 1))
            .filter(|&i| old.iter().enumerate().all(|(j, o)| same(lines[i + j], o)))
            .min_by_key(|&i| i.abs_diff(near.start))
            .map(|i| i..i + old.len())
    };
    find(&|a, b| a.trim_end() == b.trim_end()).or_else(|| find(&|a, b| a.trim() == b.trim()))
}

/// The visible part of a step: the lines that change, after dropping the
/// lines `old` and `new` share at either end.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// Lines struck out.
    pub removed: Range<usize>,
    /// Lines shown after `removed` (or after the line above it).
    pub added: Vec<String>,
}

impl Change {
    /// The line the added lines show under.
    pub fn anchor_line(&self) -> usize {
        if self.removed.is_empty() { self.removed.start.saturating_sub(1) } else { self.removed.end - 1 }
    }
}

/// `old` (found at lines `at`) becoming `new`.
pub fn change(at: Range<usize>, old: &[String], new: &[String]) -> Change {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a.trim_end() == b.trim_end()).count();
    let rest = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(rest).take_while(|(a, b)| a.trim_end() == b.trim_end()).count();
    let (keep_old, keep_new) = if old.is_empty() { (0, 0) } else { (prefix, suffix) };
    let removed_len = if old.is_empty() { at.len() } else { old.len() - keep_old - keep_new };
    let start = at.start + keep_old;
    Change { removed: start..start + removed_len, added: new[prefix.min(new.len())..new.len() - suffix.min(new.len() - prefix.min(new.len()))].to_vec() }
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

    fn steps(reply: &str) -> Vec<Step> {
        parse_pieces(reply).into_iter().filter_map(|p| if let Piece::Step(s) = p { Some(s) } else { None }).collect()
    }

    #[test]
    fn notes_pin_to_lines_as_they_stream() {
        let reply = "<answer>\n<note>\n<at>\n    return a - b\n</at>\nThis subtracts.\n</note>\n<note>\nIt should add.\n</no";
        assert_eq!(
            parse_notes(reply),
            vec![
                Note { file: None, at: lines(&["    return a - b"]), text: "This subtracts.".into() },
                Note { file: None, at: vec![], text: "It should add.".into() }
            ]
        );
        assert_eq!(parse_notes("<answer>\n<note>\n<at>\n    ret"), vec![]);
        assert_eq!(
            parse_notes("<answer>\nPlain answer.\n\nSecond part.\n</answer>"),
            vec![Note { file: None, at: vec![], text: "Plain answer.".into() }, Note { file: None, at: vec![], text: "Second part.".into() }]
        );
    }

    #[test]
    fn notes_can_be_in_other_files() {
        let reply = "<answer>\n<note>\n<file>\nsrc/lib.rs\n</file>\n<at>\npub fn run() {\n</at>\nIt starts here.\n</note>\n<note>\n<file>\nsrc/ma";
        assert_eq!(
            parse_notes(reply),
            vec![Note { file: Some("src/lib.rs".into()), at: lines(&["pub fn run() {"]), text: "It starts here.".into() }],
            "a note waits for its file"
        );
    }

    #[test]
    fn tells_answers_from_edits() {
        assert_eq!(reply_kind(""), None);
        assert_eq!(reply_kind("\n<answ"), None);
        assert_eq!(reply_kind("<answer>\nIt adds."), Some(Kind::Question));
        assert_eq!(reply_kind("<step>\n<old>"), Some(Kind::Edit));
        assert_eq!(reply_kind("<remove>\nx\n</remove>"), Some(Kind::Edit));
        assert_eq!(answer_text("<answer>\nIt adds\ntwo numbers.\n</ans"), "It adds\ntwo numbers.");
    }

    #[test]
    fn memory_arrives_whole() {
        let reply = "<answer>\nok\n</answer>\n<memory>\nPrefers tabs.\n";
        assert_eq!(memory_update(reply), None, "not until it's closed");
        assert_eq!(memory_update(&format!("{reply}</memory>")).as_deref(), Some("Prefers tabs."));
        assert_eq!(parse_notes(&format!("{reply}</memory>")), vec![Note { file: None, at: vec![], text: "ok".into() }], "the notes aren't part of the answer");
    }

    #[test]
    fn steps_appear_as_they_stream() {
        let full =
            "<step>\n<old>\n    pass\n</old>\n<new>\n    total = a + b\n    return total\n</new>\n</step>\n<step>\n<old>\n</old>\n<new>\nx\n</new>\n</step>";
        assert_eq!(steps(&full[..20]), vec![]);
        let partial = &full[..full.find("    return").unwrap() + 6];
        assert_eq!(steps(partial), vec![Step { file: None, old: lines(&["    pass"]), new: lines(&["    total = a + b", "    re"]), complete: false }]);
        let cut_tag = &full[..full.find("</new>").unwrap() + 3];
        assert_eq!(steps(cut_tag)[0].new, lines(&["    total = a + b", "    return total", ""]));
        let all = steps(full);
        assert!(all[0].complete && all[1].complete);
        assert_eq!(all[1], Step { file: None, old: vec![], new: lines(&["x"]), complete: true });
    }

    #[test]
    fn file_operations_parse_in_order() {
        let reply = "<create>\nsrc/util.rs\n</create>\n<step>\n<file>\nsrc/util.rs\n</file>\n<old>\n</old>\n<new>\npub fn one() {}\n</new>\n</step>\n<rename>\na.rs\nb.rs\n</rename>\n<remove>\nold.rs\n</remove>\n";
        let pieces = parse_pieces(reply);
        assert_eq!(pieces[0], Piece::Create("src/util.rs".into()));
        assert!(matches!(&pieces[1], Piece::Step(s) if s.file.as_deref() == Some("src/util.rs") && s.old.is_empty()));
        assert_eq!(pieces[2], Piece::Rename { from: "a.rs".into(), to: "b.rs".into() });
        assert_eq!(pieces[3], Piece::Remove("old.rs".into()));
        assert_eq!(parse_pieces("<create>\nsrc/ut"), vec![], "a path appears once its line is done");
        assert_eq!(parse_pieces("<rename>\nsrc/util.rs\n"), vec![], "both paths first");
    }

    #[test]
    fn tags_inside_code_are_just_text() {
        let reply = "<step>\n<old>\nlet s = \"<old>\";\n</old>\n<new>\n// see <create> and </new> in the docs\nlet s = \"<step>\";\n</new>\n</step>\n";
        let all = steps(reply);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].old, lines(&["let s = \"<old>\";"]));
        assert_eq!(all[0].new, lines(&["// see <create> and </new> in the docs", "let s = \"<step>\";"]));
        assert!(all[0].complete);
        assert_eq!(reply_kind("Looking at <answer> handling first.\n<step>\n"), Some(Kind::Edit));
    }

    #[test]
    fn a_follow_up_splits_into_now_and_the_rest() {
        let reply = "<answer>\nBecause.\n</answer>\n<rest>\n<answer>\n<note>\nNext.\n</note>\n</answer>\n</rest>\n";
        let (now, rest) = split_rest(reply);
        assert_eq!(parse_notes(&now), vec![Note { file: None, at: vec![], text: "Because.".into() }]);
        assert_eq!(parse_notes(&rest.unwrap()), vec![Note { file: None, at: vec![], text: "Next.".into() }]);
        assert_eq!(split_rest("<answer>\nok\n</answer>\n").1, None, "no <rest>: the rest stays");
        assert!(rest_settled("<answer>\nok\n</answer>\n<memory>\n") && rest_settled(reply) && !rest_settled("<answer>\nok\n</answer>\n"));
        assert_eq!(split_rest("<rest>\n<step>\n<old>\na\n</re").1.as_deref(), Some("\n<step>\n<old>\na\n"), "streaming in");
    }

    #[test]
    fn locates_the_nearest_copy() {
        let text = "a\nx\nb\nx\nc\n";
        assert_eq!(locate(text, &lines(&["x"]), 3..4), Some(3..4));
        assert_eq!(locate(text, &lines(&["x  "]), 0..1), Some(1..2));
        assert_eq!(locate(text, &lines(&["b", "x"]), 0..1), Some(2..4));
        assert_eq!(locate(text, &lines(&["nope"]), 0..1), None);
        assert_eq!(locate("def f():\n\treturn 1\n", &lines(&["    return 1"]), 0..1), Some(1..2), "indentation may differ");
        assert_eq!(locate(text, &[], 4..5), Some(4..5));
    }

    #[test]
    fn change_hides_shared_lines() {
        let c = change(5..6, &lines(&["fn a() {"]), &lines(&["fn a() {", "    x();"]));
        assert_eq!(c, Change { removed: 6..6, added: lines(&["    x();"]) });
        assert_eq!(c.anchor_line(), 5);
        let c = change(2..5, &lines(&["a", "b", "c"]), &lines(&["a", "B", "c"]));
        assert_eq!(c, Change { removed: 3..4, added: lines(&["B"]) });
        let c = change(7..8, &[], &lines(&["new"]));
        assert_eq!(c, Change { removed: 7..8, added: lines(&["new"]) });
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
