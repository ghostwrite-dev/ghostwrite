//! A small language-server client. [`Registry`] starts one server per
//! language per project on first use; a [`Document`] keeps one open file in
//! sync with its server and asks it things; [`convert`] turns the answers
//! into byte offsets and plain text. Diagnostics arrive as [`Event`]s.
//!
//! What's kept for later is as the server sent it (the `Raw` types): its
//! positions are placed in the text as it is when they're used, which
//! may have changed since.

pub mod client;
pub mod convert;
pub mod document;
mod python;
pub mod registry;
mod shell;

pub use client::{Client, Event};
pub use document::Document;
pub use registry::{Registry, warm};

/// A diagnostic as the server sends it, before [`convert::diagnostics`] places it.
pub use lsp_types::Diagnostic as RawDiagnostic;

/// A range as the server sends it, before [`convert::Lines`] places it.
pub use lsp_types::Range as RawRange;

/// A position as the server sends it (line, UTF-16 column).
pub use lsp_types::Position as RawPosition;

/// How bad a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        }
    }
}

/// A diagnostic, placed in the text it was published for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: std::ops::Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// A completion: what to show, and the edit accepting it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub detail: Option<String>,
    pub range: std::ops::Range<usize>,
    pub text: String,
    /// More edits that come with it (an import it needs), elsewhere in the file.
    pub more: Vec<(std::ops::Range<usize>, String)>,
}

/// An edit to a file a server asks for: where (as the server counts) and what goes there.
pub type TextEdit = (RawRange, String);

/// A symbol in a file (`space s`): its name, what it is, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    /// Its container's names, outermost first (`impl Editor`).
    pub within: Vec<String>,
    pub range: RawRange,
}

/// A change the server offers at the cursor (`space .`): a quick fix, a refactoring.
#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    /// The edit it makes, if the server said (else it's resolved first, or a command does it).
    pub edit: Option<serde_json::Value>,
    pub command: Option<serde_json::Value>,
    /// The action as the server sent it, to resolve.
    pub raw: serde_json::Value,
}
