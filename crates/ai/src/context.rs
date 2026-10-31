//! What Claude is told. A story is a conversation: the first message carries
//! the context (Claude's own notes from earlier sessions, the user's recent
//! interactions and what Claude said in them, the project's files, the
//! editor around the request, the last `:sh` command and its output, the
//! user's terminal, uncommitted changes), then the file with the lines marked and the
//! request. Each reply is one item; each turn after it says what the user
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

use ghostwrite_project::{files, store};
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
    /// The last command run with `:sh`: `$ command`, how it went, and its output.
    pub command: Option<String>,
    /// The user's terminal: its scrollback and screen, as text.
    pub terminal: Option<String>,
}

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
            out.push_str(&project_context(root));
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
        if let Some(command) = &followup.command {
            out.push_str(&format!("Since your reply, the user ran a command (`:sh`):\n<output>\n{}\n</output>\n\n", command.trim_end()));
        }
        if let Some(terminal) = &followup.terminal {
            out.push_str(&format!("{TERMINAL} now (it has changed since your reply):\n<terminal>\n{}\n</terminal>\n\n", terminal.trim_end()));
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
            let room = crate::settings().context_bytes / 2 / self.others.len().max(1);
            for other in &self.others {
                out.push_str(&format!("{}\n", contents(&other.path, &other.text, other.focus.clone(), room, false)));
            }
            out.push_str(FOLLOW_UP_GUIDE);
        }
        out
    }

    /// The file asked about, around the lines pointed at (marked, if `marked`).
    fn asked_contents(&self, marked: bool) -> String {
        let at = |byte: usize| self.text[..byte.min(self.text.len())].matches('\n').count();
        let (start, end) = (self.range.start, self.range.end.max(self.range.start + 1));
        contents(&self.file_name(), &self.text, at(start)..at(end - 1) + 1, crate::settings().context_bytes, marked)
    }

    /// The editor around the request: open files, diagnostics, uncommitted changes.
    fn surroundings_block(&self) -> String {
        let mut out = String::new();
        let s = &self.surroundings;
        if !s.open_files.is_empty() {
            out.push_str(&format!("Open in the editor: {}\n\n", s.open_files.join(", ")));
        }
        if !s.diagnostics.is_empty() {
            out.push_str(&format!("Diagnostics near the marked lines:\n{}\n\n", s.diagnostics.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n")));
        }
        if let Some(command) = &s.command {
            out.push_str(&format!("The user's last command in the editor (`:sh`), for context:\n<output>\n{}\n</output>\n\n", command.trim_end()));
        }
        if let Some(terminal) = &s.terminal {
            out.push_str(&format!("{TERMINAL}, for context:\n<terminal>\n{}\n</terminal>\n\n", terminal.trim_end()));
        }
        if let Some(diff) = self.root.as_deref().and_then(|root| files::uncommitted(root, crate::settings().max_diff)) {
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

/// Claude's notes, the recent interactions, and the file list.
fn project_context(root: &Path) -> String {
    let mut out = String::new();
    let notes = store::load_memory(root);
    if !notes.trim().is_empty() {
        out.push_str(&format!("Your notes from earlier sessions:\n<memory>\n{}\n</memory>\n\n", notes.trim()));
    }
    let recent: Vec<Interaction> = store::recent_interactions(root, RECENT).iter().filter_map(Interaction::from_json).collect();
    if !recent.is_empty() {
        let lines: Vec<String> = recent.iter().enumerate().map(|(i, it)| it.describe(i + RECAPPED >= recent.len())).collect();
        out.push_str(&format!("The user's recent requests in this project, oldest first:\n{}\n\n", lines.join("\n")));
    }
    out.push_str(&format!("Project files:\n{}\n\n", files::listing(root, MAX_LISTED)));
    out
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
        store::log_interaction(root, &self.to_json());
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
        }
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
            command: None,
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
        let fix = FollowUp {
            said: "its <at> lines aren't in lib.rs".into(),
            correction: true,
            command: Some("$ make (✗ exit 2)\nboom".into()),
            terminal: Some("$ ls\nsrc".into()),
            ..followup
        };
        let turns = Request { followups: vec![fix], ..request(None) }.messages();
        assert!(turns[2].1.contains("Ghostwrite couldn't use your reply: its <at> lines aren't in lib.rs"), "{}", turns[2].1);
        assert!(turns[2].1.contains("the user ran a command (`:sh`):\n<output>\n$ make (✗ exit 2)\nboom\n</output>"), "{}", turns[2].1);
        assert!(turns[2].1.contains("as it is now (it has changed since your reply):\n<terminal>\n$ ls\nsrc\n</terminal>"), "{}", turns[2].1);
    }

    #[test]
    fn the_newest_turn_shows_the_other_files_in_the_story() {
        let other = Shown { path: "util.rs".into(), text: "pub fn one() {}\npub fn two() {}\n".into(), focus: 1..2 };
        let followup = FollowUp { reply: "<file>…</new>".into(), decided: vec![], said: String::new(), correction: false, command: None, terminal: None };
        let req = Request { followups: vec![followup.clone(), followup], others: vec![other], ..request(None) };
        let turns = req.messages();
        assert!(turns[4].1.contains("<contents path=\"util.rs\">\n1│pub fn one() {}\n2│pub fn two() {}\n</contents>"), "{}", turns[4].1);
        assert!(!turns[2].1.contains("util.rs"), "only in the newest turn");
    }

    #[test]
    fn the_first_message_carries_notes_interactions_and_the_editor() {
        store::redirect_for_tests();
        let root = ghostwrite_project::scratch_dir("context");
        store::save_memory(&root, "The user likes early returns.");
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
            command: Some("$ cargo check (✗ exit 101)\nerror: oops".into()),
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
        assert!(prompt.contains("<output>\n$ cargo check (✗ exit 101)\nerror: oops\n</output>"), "{prompt}");
        assert!(prompt.contains("nothing you say is typed into it), as it is, for context:\n<terminal>\n$ git status\nclean\n</terminal>"), "{prompt}");
        assert!(prompt.contains("File: lib.rs (rust)"));
    }
}
