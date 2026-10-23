//! What Claude is told with each request. Besides the file and the lines
//! pointed at, Claude gets the project's file list, its own notes from
//! earlier sessions (which it rewrites with a `<memory>` block), and the
//! user's recent interactions in this project, so it knows what they've been
//! working on. After each story, an [`Interaction`] is logged.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ghostwrite_project::{files, store};
use serde_json::{Value, json};

use crate::story::FollowUp;

/// The file around the lines is context; very large files get a window.
const CONTEXT_BYTES: usize = 200_000;
/// Files listed in the prompt.
const MAX_LISTED: usize = 400;
/// Earlier interactions shown in the prompt.
const RECENT: usize = 12;

pub struct Request {
    pub path: Option<PathBuf>,
    pub language: String,
    pub text: String,
    /// The lines pointed at, in bytes, whole lines.
    pub range: Range<usize>,
    pub instruction: String,
    /// What the user said during the review, and where, in order.
    pub followups: Vec<FollowUp>,
    /// Command output the request is about (an error from `:sh`), if any.
    pub output: Option<String>,
    /// The project, which paths are relative to.
    pub root: Option<PathBuf>,
}

impl Request {
    /// The file's name as Claude sees it: relative to the project.
    pub fn file_name(&self) -> String {
        self.path.as_ref().map_or("untitled".into(), |p| self.root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string())
    }

    /// The user message: context first, then the file with the lines
    /// marked, then the request and any follow-ups.
    pub fn prompt(&self) -> String {
        let mut out = String::new();
        if let Some(root) = &self.root {
            let notes = store::load_memory(root);
            if !notes.trim().is_empty() {
                out.push_str(&format!("Your notes from earlier sessions:\n<memory>\n{}\n</memory>\n\n", notes.trim()));
            }
            let recent: Vec<String> = store::recent_interactions(root, RECENT).iter().filter_map(Interaction::from_json).map(|i| i.describe()).collect();
            if !recent.is_empty() {
                out.push_str(&format!("The user's recent requests in this project, oldest first:\n{}\n\n", recent.join("\n")));
            }
            out.push_str(&format!("Project files:\n{}\n\n", files::listing(root, MAX_LISTED)));
        }
        let (text, range) = (&self.text, &self.range);
        let start = text.floor_char_boundary(range.start.saturating_sub(CONTEXT_BYTES / 2));
        let end = text.floor_char_boundary((range.end + CONTEXT_BYTES / 2).min(text.len()));
        out.push_str(&format!(
            "File: {} ({})\n\n<file>\n{}<cursor>{}</cursor>{}\n</file>\n\n",
            self.file_name(),
            self.language,
            &text[start..range.start],
            &text[range.clone()],
            &text[range.end..end],
        ));
        if let Some(output) = &self.output {
            out.push_str(&format!("Command output about the marked lines:\n<output>\n{}\n</output>\n\n", output.trim()));
        }
        out.push_str(&format!("Request: {}", self.instruction.trim()));
        for followup in &self.followups {
            out.push_str(&format!("\n\nYour earlier reply:\n<earlier>\n{}\n</earlier>\n", followup.reply.trim()));
            if let Some(at) = &followup.at {
                out.push_str(&format!("The user is at this part of it:\n<at>\n{}\n</at>\n", at.trim()));
            }
            out.push_str(&format!("The user says: {}\n", followup.said.trim()));
        }
        if !self.followups.is_empty() {
            out.push_str(
                "\nAny steps the user accepted are already in the file above and in the project's other files \
                 (read them again if you need to). It's up to you what the user's last message is (a question, \
                 a comment, a correction, a change of direction) and what it changes. Reply in two parts. First, \
                 what answers it, shown right where they are: notes in <answer>, or steps for something to do \
                 right now. Then, if what they said changes what should come next, write the new rest in the \
                 same format as a first reply, inside <rest></rest>: it replaces the part they're at (unless \
                 it's a note they've read) and everything after it, so include all of what's still to come, \
                 corrected steps too. Leave <rest> out if everything after stays as it was.",
            );
        }
        out
    }
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
}

impl Interaction {
    pub fn new(file: String, request: String, followups: Vec<String>, outcome: String) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self { at, file, request, followups, outcome }
    }

    /// Add it to `root`'s log.
    pub fn log(&self, root: &Path) {
        store::log_interaction(root, &self.to_json());
    }

    fn to_json(&self) -> Value {
        json!({ "at": self.at, "file": self.file, "request": self.request, "followups": self.followups, "outcome": self.outcome })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            at: value["at"].as_u64().unwrap_or(0),
            file: value["file"].as_str()?.to_string(),
            request: value["request"].as_str()?.to_string(),
            followups: value["followups"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            outcome: value["outcome"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// One line (plus follow-ups) for the prompt.
    fn describe(&self) -> String {
        let mut line = format!("- {} in {}: \"{}\" ({})", ago(self.at), self.file, self.request.trim(), self.outcome);
        for said in &self.followups {
            line.push_str(&format!("\n  then: \"{}\"", said.trim()));
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
            output: None,
            root,
        }
    }

    #[test]
    fn the_prompt_marks_the_lines() {
        let req = request(None);
        let prompt = req.prompt();
        assert!(prompt.contains("<file>\nfn a() {}\n<cursor>fn b() {}\n</cursor>\n</file>"), "{prompt}");
        assert!(prompt.ends_with("Request: rename to c"));
        let followup = FollowUp { reply: "<step>…</step>".into(), at: Some("a step".into()), said: "keep the old name".into() };
        let again = Request { followups: vec![followup], ..req }.prompt();
        assert!(again.contains("<earlier>\n<step>…</step>\n</earlier>\nThe user is at this part of it:\n<at>\na step\n</at>\nThe user says: keep the old name"), "{again}");
        assert!(again.contains("<rest>"));
    }

    #[test]
    fn the_prompt_carries_notes_and_recent_interactions() {
        store::redirect_for_tests();
        let root = ghostwrite_project::scratch_dir("context");
        store::save_memory(&root, "The user likes early returns.");
        Interaction::new("main.rs".into(), "add logging".into(), vec!["use tracing".into()], "applied 2 of 2 steps".into()).log(&root);
        let prompt = request(Some(root.clone())).prompt();
        assert!(prompt.contains("<memory>\nThe user likes early returns.\n</memory>"), "{prompt}");
        assert!(prompt.contains("just now in main.rs: \"add logging\" (applied 2 of 2 steps)\n  then: \"use tracing\""), "{prompt}");
        assert!(prompt.contains("File: lib.rs (rust)"));
    }
}
