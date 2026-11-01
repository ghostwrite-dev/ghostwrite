//! What Claude is told. A story is a conversation: the first message carries
//! the context (Claude's own notes from earlier sessions, the user's recent
//! interactions and what Claude said in them, the project's files, the
//! editor around the request, the user's terminal, uncommitted changes),
//! then the file with the lines marked and the request. Each reply is one item; each turn after it says what the user
//! did with it and what they said, if anything (or, when Ghostwrite couldn't
//! use the reply, why), and asks for the next item. The newest turn also
//! shows the file asked about and the other files the story is in, as they
//! are now. Every file is shown the same way, its lines numbered
//! ([`contents`]), so the lines Claude quotes say where they are.
//!
//! [`Request::messages`] gives the turns for a backend that holds a
//! conversation; [`Request::prompt`] flattens them into one message; and
//! [`Request::latest`] is just the newest turn, for resuming a session that
//! has the rest. After each story, an [`Interaction`] is logged.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ghostwrite_project::files;
use serde_json::{Value, json};

use crate::lines;
use crate::story::FollowUp;

/// Files listed in the prompt.
const MAX_LISTED: usize = 400;
/// Earlier interactions shown in the prompt, and how many of the newest
/// come with what Claude said in them.
const RECENT: usize = 12;
const RECAPPED: usize = 4;

/// What's around the request in the editor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Surroundings {
    /// The files open in tabs, relative to the project.
    pub open_files: Vec<String>,
    /// Diagnostics near the marked lines (`line 12: error: …`).
    pub diagnostics: Vec<String>,
    /// The user's terminal: its scrollback and screen, as text.
    pub terminal: Option<String>,
}

/// What's sent to a model with a small context: all a small step needs, as it can
/// read more. Every part has its share, so none can crowd out the rest (a data
/// project's file list can be longer than the whole context).
const SMALL_CONTEXT: usize = 8_000;
const SMALL_INSTRUCTIONS: usize = 1_000;
/// The files a project says how it wants things done in, and how much of each is read.
const INSTRUCTIONS: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".ghostwrite.md"];
const MAX_INSTRUCTIONS: usize = 20_000;
const SMALL_DIFF: usize = 1_500;
const SMALL_FILES: usize = 1_500;
const SMALL_NOTES: usize = 800;
const SMALL_RECENT: usize = 800;
const SMALL_TERMINAL: usize = 1_000;
const SMALL_SURROUNDINGS: usize = 600;

/// The start of `text`, in about `bytes`, at a line's end, saying how to see the rest.
fn start_of(text: &str, bytes: usize, more: &str) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let cut = text.floor_char_boundary(bytes);
    let kept = text[..cut].rfind('\n').map_or(&text[..cut], |i| &text[..i]);
    format!("{kept}\n({more})")
}

/// The end of `text`, in about `bytes`, from a line's start: what's newest.
fn end_of(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let cut = text.ceil_char_boundary(text.len() - bytes);
    text[cut..].find('\n').map_or(&text[cut..], |i| &text[cut + i + 1..])
}

#[derive(Clone)]
pub struct Request {
    pub path: Option<PathBuf>,
    pub language: String,
    pub text: String,
    /// The lines pointed at, in bytes, whole lines.
    pub range: Range<usize>,
    pub instruction: String,
    /// The conversation since: each of Claude's replies and what the user did and said next.
    pub followups: Vec<FollowUp>,
    /// The other files the story is in, as the editor has them (or as a
    /// request sent ahead assumes they'll be).
    pub others: Vec<Shown>,
    /// The project, which paths are relative to.
    pub root: Option<PathBuf>,
    pub surroundings: Surroundings,
    /// A Claude Code session that already holds the story's earlier turns.
    pub session: Option<String>,
    /// Resume `session` as a new one, leaving it as it was (a request sent ahead of time).
    pub fork: bool,
    /// For a model with a small context, how far to shrink what's sent: not (0),
    /// or to its share (`SMALL_`…, divided by this).
    pub small: usize,
}

/// A file shown to Claude besides the one asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Relative to the project.
    pub path: String,
    pub text: String,
    /// The lines the story was last at in it, which a large file is shown around.
    pub focus: Range<usize>,
}

/// Who says a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// How the terminal is introduced: Claude can read it, not type in it.
const TERMINAL: &str = "The user's terminal in the editor (you can read it; nothing you say is typed into it), as it is";

/// A reminder with each turn (the system prompt has the rules).
const FOLLOW_UP_GUIDE: &str = "What the user accepted is in the files as shown here, which are the editor's copies: the \
files on disk may not have it yet, so go by what's shown, and quote lines with the numbers shown here. Reply with the \
next item, in the format (in whichever file needs it next), or <done> if there's nothing more.";

impl Request {
    /// The file's name as Claude sees it: relative to the project.
    pub fn file_name(&self) -> String {
        self.path.as_ref().map_or("untitled".into(), |p| self.root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string())
    }

    /// The conversation so far, oldest first; the last turn is the user's.
    pub fn messages(&self) -> Vec<(Role, String)> {
        let mut turns = vec![(Role::User, self.first_message())];
        for (i, followup) in self.followups.iter().enumerate() {
            let reply = if followup.reply.trim().is_empty() { "(no reply)".to_string() } else { followup.reply.clone() };
            turns.push((Role::Assistant, reply));
            turns.push((Role::User, self.follow_up_message(followup, i + 1 == self.followups.len())));
        }
        turns
    }

    fn context_bytes(&self) -> usize {
        SMALL_CONTEXT.checked_div(self.small).unwrap_or(crate::settings().context_bytes)
    }

    /// `text`, in its share for a small context.
    fn share(&self, text: &str, bytes: usize, more: &str) -> String {
        bytes.checked_div(self.small).map_or_else(|| text.to_string(), |bytes| start_of(text, bytes, more))
    }

    /// The terminal: for a small context, only its end.
    fn terminal<'a>(&self, text: &'a str) -> &'a str {
        SMALL_TERMINAL.checked_div(self.small).map_or(text, |bytes| end_of(text, bytes))
    }

    /// The whole conversation as one message, for a backend that starts fresh.
    pub fn prompt(&self) -> String {
        let mut out = self.first_message();
        for (i, followup) in self.followups.iter().enumerate() {
            out.push_str(&format!("\n\nYour reply:\n<earlier>\n{}\n</earlier>\n\n", followup.reply.trim()));
            out.push_str(&self.follow_up_message(followup, i + 1 == self.followups.len()));
        }
        out
    }

    /// Only the newest turn: what's new since Claude's last reply.
    pub fn latest(&self) -> String {
        self.messages().pop().map(|(_, text)| text).unwrap_or_default()
    }

    /// Context, the file with the lines marked, and the request.
    fn first_message(&self) -> String {
        let mut out = String::new();
        if let Some(root) = &self.root {
            out.push_str(&self.project_context(root));
        }
        out.push_str(&self.surroundings_block());
        out.push_str(&format!("File: {} ({})\n\n{}\n", self.file_name(), self.language, self.asked_contents(true)));
        out.push_str(&format!("Request: {}", self.instruction.trim()));
        out
    }

    /// A turn after a reply: what the user decided, and what they said. The
    /// newest also has the files as they are now, and the guide.
    fn follow_up_message(&self, followup: &FollowUp, latest: bool) -> String {
        let mut out = String::new();
        if !followup.decided.is_empty() {
            let decided: Vec<String> = followup.decided.iter().map(|d| format!("- {}", d.trim())).collect();
            out.push_str(&format!("The user decided:\n{}\n\n", decided.join("\n")));
        }
        if let Some(terminal) = &followup.terminal {
            out.push_str(&format!("{TERMINAL} now (it has changed since your reply):\n<terminal>\n{}\n</terminal>\n\n", self.terminal(terminal).trim_end()));
        }
        if followup.correction {
            out.push_str(&format!("Ghostwrite couldn't use your reply: {}", followup.said.trim()));
        } else if followup.said.trim().is_empty() {
            out.push_str("Next item, please.");
        } else {
            out.push_str(&format!("The user says: {}", followup.said.trim()));
        }
        if latest {
            out.push_str(&format!("\n\nThe files as they are now:\n\n{}\n", self.asked_contents(false)));
            // The other files share half as much room as the one asked about.
            let room = self.context_bytes() / 2 / self.others.len().max(1);
            for other in &self.others {
                out.push_str(&format!("{}\n", contents(&other.path, &other.text, other.focus.clone(), room, false)));
            }
            out.push_str(FOLLOW_UP_GUIDE);
            // The request can be far back by now, and a small model loses sight of it.
            out.push_str(&format!(
                "\n\nThe request was: {}\nGo through the files as they are now: reply with the next item it still needs. Only when all of it is finished, reply with <done>.",
                self.instruction.trim()
            ));
        }
        out
    }

    /// The file asked about, around the lines pointed at (marked, if `marked`).
    fn asked_contents(&self, marked: bool) -> String {
        // By the bytes: a place in the text needn't be between two of its characters.
        let at = |byte: usize| self.text.as_bytes().iter().take(byte).filter(|b| **b == b'\n').count();
        let (start, end) = (self.range.start, self.range.end.max(self.range.start + 1));
        contents(&self.file_name(), &self.text, at(start)..at(end - 1) + 1, self.context_bytes(), marked)
    }

    /// The model's notes, the recent interactions, and the file list.
    fn project_context(&self, root: &Path) -> String {
        let mut out = String::new();
        let instructions = project_instructions(root);
        if !instructions.is_empty() {
            let told = self.share(&instructions, SMALL_INSTRUCTIONS, "and more: `read` the file to see it");
            out.push_str(&format!("How this project wants things done, from its own files:\n<instructions>\n{told}\n</instructions>\n\n"));
        }
        let notes = crate::memory::notes(root);
        if !notes.trim().is_empty() {
            out.push_str(&format!("Your notes from earlier sessions:\n<memory>\n{}\n</memory>\n\n", self.share(notes.trim(), SMALL_NOTES, "and more")));
        }
        let recent: Vec<Interaction> = crate::memory::recent(root, RECENT).iter().filter_map(Interaction::from_json).collect();
        if !recent.is_empty() {
            let lines: Vec<String> = recent.iter().enumerate().map(|(i, it)| it.describe(i + RECAPPED >= recent.len())).collect();
            let lines = lines.join("\n");
            let lines = SMALL_RECENT.checked_div(self.small).map_or_else(|| lines.clone(), |bytes| end_of(&lines, bytes).to_string());
            out.push_str(&format!("The user's recent requests in this project, oldest first:\n{lines}\n\n"));
        }
        let listing = files::listing(root, MAX_LISTED);
        out.push_str(&format!("Project files:\n{}\n\n", self.share(&listing, SMALL_FILES, "and more: `list` a folder to see them")));
        out
    }

    /// The editor around the request: open files, diagnostics, uncommitted changes.
    fn surroundings_block(&self) -> String {
        let mut out = String::new();
        let s = &self.surroundings;
        if !s.open_files.is_empty() {
            out.push_str(&format!("Open in the editor: {}\n\n", self.share(&s.open_files.join(", "), SMALL_SURROUNDINGS, "and more")));
        }
        if !s.diagnostics.is_empty() {
            let diagnostics = s.diagnostics.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n");
            out.push_str(&format!("Diagnostics near the marked lines:\n{}\n\n", self.share(&diagnostics, SMALL_SURROUNDINGS, "and more")));
        }
        if let Some(terminal) = &s.terminal {
            out.push_str(&format!("{TERMINAL}, for context:\n<terminal>\n{}\n</terminal>\n\n", self.terminal(terminal).trim_end()));
        }
        if let Some(diff) =
            self.root.as_deref().and_then(|root| files::uncommitted(root, SMALL_DIFF.checked_div(self.small).unwrap_or(crate::settings().max_diff)))
        {
            out.push_str(&format!("Uncommitted changes in the project:\n<diff>\n{}\n</diff>\n\n", diff.trim_end()));
        }
        out
    }
}

/// A file as Claude is shown it: its lines numbered, around the lines
/// `focus` if it's more than `bytes` long, and those lines marked with
/// `<cursor>` if `marked`.
pub fn contents(path: &str, text: &str, focus: Range<usize>, bytes: usize, marked: bool) -> String {
    format!("<contents path=\"{path}\">\n{}</contents>\n", lines::window(text, focus, bytes, marked))
}

/// What the project's own instruction files say.
fn project_instructions(root: &Path) -> String {
    let read = |name: &&str| {
        let text = std::fs::read_to_string(root.join(name)).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| format!("{name}:\n{}", &text[..text.floor_char_boundary(MAX_INSTRUCTIONS)]))
    };
    INSTRUCTIONS.iter().filter_map(read).collect::<Vec<_>>().join("\n\n")
}

/// A finished story, as remembered for the next ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interaction {
    /// Seconds since the epoch.
    pub at: u64,
    pub file: String,
    pub request: String,
    /// What the user said during the review, in order.
    pub followups: Vec<String>,
    /// What came of it: "answered", "applied 2 of 3 steps", "stopped".
    pub outcome: String,
    /// What Claude said, in brief.
    pub recap: String,
}

impl Interaction {
    pub fn new(file: String, request: String, followups: Vec<String>, outcome: String, recap: String) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self { at, file, request, followups, outcome, recap }
    }

    /// Add it to `root`'s log.
    pub fn log(&self, root: &Path) {
        crate::memory::log(root, &self.to_json());
    }

    fn to_json(&self) -> Value {
        json!({ "at": self.at, "file": self.file, "request": self.request, "followups": self.followups, "outcome": self.outcome, "recap": self.recap })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            at: value["at"].as_u64().unwrap_or(0),
            file: value["file"].as_str()?.to_string(),
            request: value["request"].as_str()?.to_string(),
            followups: value["followups"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            outcome: value["outcome"].as_str().unwrap_or_default().to_string(),
            recap: value["recap"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// A line for the prompt, with the follow-ups, and (if `recapped`) what Claude said.
    fn describe(&self, recapped: bool) -> String {
        let mut line = format!("- {} in {}: \"{}\" ({})", ago(self.at), self.file, self.request.trim(), self.outcome);
        for said in &self.followups {
            line.push_str(&format!("\n  then: \"{}\"", said.trim()));
        }
        if recapped && !self.recap.trim().is_empty() {
            line.push_str(&format!("\n  you said: {}", self.recap.trim()));
        }
        line
    }
}

/// How long ago `at` was, roughly.
fn ago(at: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(at) {
        s if s < 120 => "just now".into(),
        s if s < 7200 => format!("{} min ago", s / 60),
        s if s < 172_800 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: Option<PathBuf>) -> Request {
        Request {
            path: root.as_ref().map(|r| r.join("lib.rs")),
            language: "rust".into(),
            text: "fn a() {}\nfn b() {}\n".into(),
            range: 10..20,
            instruction: "rename to c".into(),
            followups: vec![],
            root,
            others: vec![],
            surroundings: Surroundings::default(),
            session: None,
            fork: false,
            small: 0,
        }
    }

    #[test]
    fn the_projects_own_instructions_are_told() {
        let root = ghostwrite_project::scratch_dir("instructions");
        std::fs::write(root.join("CLAUDE.md"), "Always use tabs.\n").unwrap();
        let told = request(Some(root.clone())).prompt();
        assert!(told.contains("<instructions>\nCLAUDE.md:\nAlways use tabs.\n</instructions>"), "{told}");
        std::fs::write(root.join("AGENTS.md"), "x".repeat(50_000)).unwrap();
        let small = Request { small: 1, ..request(Some(root)) }.prompt();
        assert!(small.len() < 45_000, "kept to what a small model has room for");
    }

    #[test]
    fn a_place_inside_a_character_is_still_a_line() {
        // The range ends just after an arrow, the last thing in the file: its last byte is inside it.
        let text = "one\n→".to_string();
        let base = Request { range: 4..text.len(), text, ..request(None) };
        assert!(base.asked_contents(true).contains("→"));
        let inside = Request { range: 5..6, ..base };
        assert!(inside.asked_contents(true).contains("→"), "nor does a range that doesn't line up with the text panic");
    }

    #[test]
    fn the_first_message_marks_the_lines() {
        let prompt = request(None).prompt();
        assert!(prompt.contains("<contents path=\"untitled\">\n1│fn a() {}\n<cursor>\n2│fn b() {}\n</cursor>\n</contents>"), "{prompt}");
        assert!(prompt.ends_with("Request: rename to c"));
    }

    #[test]
    fn a_story_is_a_conversation() {
        let followup = FollowUp {
            reply: "<file>…</new>".into(),
            decided: vec!["accepted: an edit".into()],
            said: "keep the old name".into(),
            correction: false,
            terminal: None,
        };
        let req = Request { followups: vec![followup.clone(), FollowUp { decided: vec![], said: String::new(), ..followup.clone() }], ..request(None) };
        let turns = req.messages();
        let roles: Vec<Role> = turns.iter().map(|(r, _)| *r).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant, Role::User]);
        assert_eq!(turns[1].1, "<file>…</new>");
        assert!(turns[2].1.starts_with("The user decided:\n- accepted: an edit\n\nThe user says: keep the old name"), "{}", turns[2].1);
        assert!(!turns[2].1.contains("<done>"), "the guide is in the newest turn only");
        assert!(
            turns[4].1.contains("Next item, please.")
                && turns[4].1.contains("as they are now:\n\n<contents path=\"untitled\">\n1│fn a() {}\n2│")
                && turns[4].1.contains("<done>"),
            "{}",
            turns[4].1
        );
        assert_eq!(req.latest(), turns[4].1, "resuming a session sends only that");
        let flat = req.prompt();
        assert!(flat.contains("Your reply:\n<earlier>\n<file>…</new>\n</earlier>") && flat.ends_with(&turns[4].1));
        let fix = FollowUp { said: "its <at> lines aren't in lib.rs".into(), correction: true, terminal: Some("$ ls\nsrc".into()), ..followup };
        let turns = Request { followups: vec![fix], ..request(None) }.messages();
        assert!(turns[2].1.contains("Ghostwrite couldn't use your reply: its <at> lines aren't in lib.rs"), "{}", turns[2].1);
        assert!(turns[2].1.contains("as it is now (it has changed since your reply):\n<terminal>\n$ ls\nsrc\n</terminal>"), "{}", turns[2].1);
    }

    #[test]
    fn the_newest_turn_shows_the_other_files_in_the_story() {
        let other = Shown { path: "util.rs".into(), text: "pub fn one() {}\npub fn two() {}\n".into(), focus: 1..2 };
        let followup = FollowUp { reply: "<file>…</new>".into(), decided: vec![], said: String::new(), correction: false, terminal: None };
        let req = Request { followups: vec![followup.clone(), followup], others: vec![other], ..request(None) };
        let turns = req.messages();
        assert!(turns[4].1.contains("<contents path=\"util.rs\">\n1│pub fn one() {}\n2│pub fn two() {}\n</contents>"), "{}", turns[4].1);
        assert!(!turns[2].1.contains("util.rs"), "only in the newest turn");
    }

    #[test]
    fn the_first_message_carries_notes_interactions_and_the_editor() {
        ghostwrite_project::store::redirect_for_tests();
        let root = ghostwrite_project::scratch_dir("context");
        crate::memory::save_notes(&root, "The user likes early returns.");
        Interaction::new(
            "main.rs".into(),
            "add logging".into(),
            vec!["use tracing".into()],
            "applied 2 of 2 steps".into(),
            "proposed 2 change(s) in main.rs, 2 applied".into(),
        )
        .log(&root);
        let surroundings = Surroundings {
            open_files: vec!["lib.rs".into(), "main.rs".into()],
            diagnostics: vec!["line 2: error: unused".into()],
            terminal: Some("$ git status\nclean".into()),
        };
        let prompt = Request { surroundings, ..request(Some(root.clone())) }.prompt();
        assert!(prompt.contains("<memory>\nThe user likes early returns.\n</memory>"), "{prompt}");
        assert!(
            prompt.contains(
                "just now in main.rs: \"add logging\" (applied 2 of 2 steps)\n  then: \"use tracing\"\n  you said: proposed 2 change(s) in main.rs, 2 applied"
            ),
            "{prompt}"
        );
        assert!(prompt.contains("Open in the editor: lib.rs, main.rs") && prompt.contains("Diagnostics near the marked lines:\n- line 2: error: unused"));
        assert!(prompt.contains("nothing you say is typed into it), as it is, for context:\n<terminal>\n$ git status\nclean\n</terminal>"), "{prompt}");
        assert!(prompt.contains("File: lib.rs (rust)"));
    }

    #[test]
    fn every_part_keeps_to_its_share_for_a_small_context() {
        let root = ghostwrite_project::scratch_dir("context-small");
        for i in 0..5000 {
            std::fs::write(root.join(format!("mesh_{i:05}.off")), "x").unwrap();
        }
        let text: String = (0..50_000).map(|i| format!("line {i} of a long file\n")).collect();
        let surroundings = Surroundings {
            open_files: (0..2000).map(|i| format!("open_{i}.py")).collect(),
            diagnostics: (0..2000).map(|i| format!("line {i}: error: nope")).collect(),
            terminal: Some((0..50_000).map(|i| format!("output {i}\n")).collect()),
        };
        let at = text.len() / 2;
        let base = Request { text, range: at..at, surroundings, ..request(Some(root.clone())) };
        let whole = base.clone().prompt();
        let small = Request { small: 1, ..base.clone() }.prompt();
        assert!(whole.len() > 100_000, "{}", whole.len());
        assert!(small.len() < 16_000, "{}", small.len());
        assert!(small.contains("output 49999") && !small.contains("output 100\n"), "the terminal's end");
        assert!(small.contains("`list` a folder"), "it knows there's more");
        let smaller = Request { small: 8, ..base }.prompt();
        assert!(smaller.len() < 3_000, "{}", smaller.len());
    }
}
