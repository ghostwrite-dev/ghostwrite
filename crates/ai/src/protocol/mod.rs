//! The reply format: what the model is asked to write, and parsing it as it
//! streams. A story goes one item at a time: every reply is exactly one item
//! (a note, an edit, a new file, a file removed or renamed, a command to run
//! in the user's terminal) or `<done>`. Its parts come in a fixed
//! order, each closed before the next opens, so an item can be checked part by
//! part as it arrives. Quoted lines may carry the numbers they were shown with
//! ([`crate::lines`]); those come off here, as where to look.

mod command;
mod parse;
mod prompt;
mod wrap;

use crate::lines;

pub(crate) use parse::STARTS;
pub use parse::{MAX_CREATED, check, parse};
pub use prompt::{NOTHING_TO_REVIEW, SAID_ALREADY, SYSTEM, about_only_pages, grammar, is_question, misplaced, missing_file, system};
pub use wrap::{wrap, wrap_code};

/// What a local model says when what's sent doesn't fit its context; said by the worker.
pub const NO_ROOM: &str = "ran out of room";

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
