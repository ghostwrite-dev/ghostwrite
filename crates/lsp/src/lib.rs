//! A small language-server client. [`Registry`] starts one server per
//! language per project on first use; a [`Document`] keeps one open file in
//! sync with its server and asks it things; [`convert`] turns the answers
//! into byte offsets and plain text. Diagnostics arrive as [`Event`]s.

pub mod client;
pub mod convert;
pub mod document;
mod python;
pub mod registry;

pub use client::{Client, Event};
pub use document::Document;
pub use registry::Registry;

/// A diagnostic as the server sends it, before [`convert::diagnostics`] places it.
pub use lsp_types::Diagnostic as RawDiagnostic;

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
}
