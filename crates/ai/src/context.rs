//! What Claude is told. A story is a conversation: the first message carries
//! the context (Claude's own notes from earlier sessions, the user's recent
//! interactions and what Claude said in them, the project's files, the
//! editor around the request, the last `:sh` command and its output,
//! uncommitted changes), then the file with the lines marked and the
//! request. Each follow-up turn says what the user accepted and skipped,
//! where they are, and what they said (or, when Ghostwrite couldn't use
//! Claude's reply, why).
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
    /// The project, which paths are relative to.
    pub root: Option<PathBuf>,
    pub surroundings: Surroundings,
    /// A Claude Code session that already holds the story's earlier turns.
    pub session: Option<String>,
}

/// Who says a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// A reminder with each follow-up (the system prompt has the rules).
const FOLLOW_UP_GUIDE: &str = "Steps the user accepted are already in the files. Reply as for any follow-up: what \
answers them now, then <rest> with the whole new plan from here if what they said changes what comes next \
(corrections included).";

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
        out.push_str(&format!("File: {} ({})\n\n<file>\n{}\n</file>\n\n", self.file_name(), self.language, self.file_window(true)));
        out.push_str(&format!("Request: {}", self.instruction.trim()));
        out
    }

    /// A follow-up turn: what the user decided, where they are, what they
    /// said. The newest also has the marked file as it is now, and the guide.
    fn follow_up_message(&self, followup: &FollowUp, latest: bool) -> String {
        let mut out = String::new();
        if !followup.decided.is_empty() {
            let decided: Vec<String> = followup.decided.iter().map(|d| format!("- {}", d.trim())).collect();
            out.push_str(&format!("Since your reply, the user has decided:\n{}\n\n", decided.join("\n")));
        }
        if let Some(at) = &followup.at {
            out.push_str(&format!("They're at this part of your reply:\n<at>\n{}\n</at>\n\n", at.trim()));
        }
        if let Some(command) = &followup.command {
            out.push_str(&format!("Since your reply, the user ran a command (`:sh`):\n<output>\n{}\n</output>\n\n", command.trim_end()));
        }
        if followup.correction {
            out.push_str(&format!("Ghostwrite couldn't use your reply: {}", followup.said.trim()));
        } else {
            out.push_str(&format!("The user says: {}", followup.said.trim()));
        }
        if latest {
            out.push_str(&format!("\n\n{} as it is now:\n<file>\n{}\n</file>\n\n{FOLLOW_UP_GUIDE}", self.file_name(), self.file_window(false)));
        }
        out
    }

    /// The file around the marked lines (marked with `<cursor>` if `marked`).
    fn file_window(&self, marked: bool) -> String {
        let (text, range) = (&self.text, self.range.start.min(self.text.len())..self.range.end.min(self.text.len()));
        // The file around the lines is context; very large files get a window.
        let half = crate::settings().context_bytes / 2;
        let start = text.floor_char_boundary(range.start.saturating_sub(half));
        let end = text.floor_char_boundary((range.end + half).min(text.len()));
        if !marked {
            return text[start..end].to_string();
        }
        format!("{}<cursor>{}</cursor>{}", &text[start..range.start], &text[range.clone()], &text[range.end..end])
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
        if let Some(diff) = self.root.as_deref().and_then(|root| files::uncommitted(root, crate::settings().max_diff)) {
            out.push_str(&format!("Uncommitted changes in the project:\n<diff>\n{}\n</diff>\n\n", diff.trim_end()));
        }
        out
    }
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
            surroundings: Surroundings::default(),
            session: None,
        }
    }

    #[test]
    fn the_first_message_marks_the_lines() {
        let prompt = request(None).prompt();
        assert!(prompt.contains("<file>\nfn a() {}\n<cursor>fn b() {}\n</cursor>\n</file>"), "{prompt}");
        assert!(prompt.ends_with("Request: rename to c"));
    }

    #[test]
    fn a_story_is_a_conversation() {
        let followup = FollowUp {
            reply: "<step>…</step>".into(),
            decided: vec!["accepted: a step".into()],
            at: Some("a step".into()),
            said: "keep the old name".into(),
            correction: false,
            command: None,
        };
        let req = Request {
            followups: vec![followup.clone(), FollowUp { decided: vec![], at: None, said: "and add docs".into(), ..followup.clone() }],
            ..request(None)
        };
        let turns = req.messages();
        let roles: Vec<Role> = turns.iter().map(|(r, _)| *r).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant, Role::User]);
        assert_eq!(turns[1].1, "<step>…</step>");
        assert!(turns[2].1.starts_with("Since your reply, the user has decided:\n- accepted: a step\n\nThey're at this part of your reply:\n<at>\na step\n</at>\n\nThe user says: keep the old name"), "{}", turns[2].1);
        assert!(!turns[2].1.contains("<rest>"), "the guide is in the newest turn only");
        assert!(turns[4].1.contains("The user says: and add docs") && turns[4].1.contains("untitled as it is now") && turns[4].1.contains("<rest>"));
        assert_eq!(req.latest(), turns[4].1, "resuming a session sends only that");
        let flat = req.prompt();
        assert!(flat.contains("Your reply:\n<earlier>\n<step>…</step>\n</earlier>") && flat.ends_with(&turns[4].1));
        let fix = FollowUp { said: "its <old> lines aren't in lib.rs".into(), correction: true, command: Some("$ make (✗ exit 2)\nboom".into()), ..followup };
        let turns = Request { followups: vec![fix], ..request(None) }.messages();
        assert!(turns[2].1.contains("Ghostwrite couldn't use your reply: its <old> lines aren't in lib.rs"), "{}", turns[2].1);
        assert!(turns[2].1.contains("the user ran a command (`:sh`):\n<output>\n$ make (✗ exit 2)\nboom\n</output>"), "{}", turns[2].1);
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
        assert!(prompt.contains("File: lib.rs (rust)"));
    }
}
