//! Syntax highlighting with tree-sitter: which language a file is, and the
//! highlighted spans of its text.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// The scopes spans are tagged with; themes color them by name, falling
/// back to the part before the last dot (`function.method` → `function`).
pub const SCOPES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "variable.special",
];

/// Files bigger than this aren't highlighted (it's redone on every change).
const MAX_HIGHLIGHT: usize = 1 << 20;

/// A highlighted stretch of text and its scope (an index into [`SCOPES`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub scope: usize,
}

pub struct Language {
    /// What the status line and Claude call it.
    pub name: &'static str,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    config: fn() -> Option<HighlightConfiguration>,
    loaded: OnceLock<Option<HighlightConfiguration>>,
}

macro_rules! language {
    ($name:literal, [$($ext:literal),*], [$($file:literal),*], $lang:expr, $query:expr) => {
        Language {
            name: $name,
            extensions: &[$($ext),*],
            file_names: &[$($file),*],
            config: || configure($name, $lang.into(), &$query),
            loaded: OnceLock::new(),
        }
    };
}

static LANGUAGES: [Language; 15] = [
    language!("rust", ["rs"], [], tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY),
    language!("python", ["py", "pyi"], [], tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY),
    language!("javascript", ["js", "mjs", "cjs"], [], tree_sitter_javascript::LANGUAGE, tree_sitter_javascript::HIGHLIGHT_QUERY),
    language!(
        "jsx",
        ["jsx"],
        [],
        tree_sitter_javascript::LANGUAGE,
        [tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n")
    ),
    language!(
        "typescript",
        ["ts", "mts", "cts"],
        [],
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n")
    ),
    language!(
        "tsx",
        ["tsx"],
        [],
        tree_sitter_typescript::LANGUAGE_TSX,
        [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n")
    ),
    language!("bash", ["sh", "bash", "zsh"], [".bashrc", ".zshrc", ".profile"], tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY),
    language!("toml", ["toml"], ["Cargo.lock"], tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
    language!("markdown", ["md", "markdown"], [], tree_sitter_md::LANGUAGE, tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
    language!("json", ["json"], [], tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY),
    language!("yaml", ["yaml", "yml"], [], tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY),
    language!("go", ["go"], [], tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY),
    language!("c", ["c", "h"], [], tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
    language!(
        "cpp",
        ["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        [],
        tree_sitter_cpp::LANGUAGE,
        [tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY].join("\n")
    ),
    language!("css", ["css"], [], tree_sitter_css::LANGUAGE, tree_sitter_css::HIGHLIGHTS_QUERY),
];

fn configure(name: &str, language: tree_sitter::Language, query: &str) -> Option<HighlightConfiguration> {
    let mut config =
        HighlightConfiguration::new(language, name, query, "", "").inspect_err(|e| eprintln!("the {name} highlight query doesn't load: {e}")).ok()?;
    config.configure(SCOPES);
    Some(config)
}

impl Language {
    /// The language of `path`, by its name or extension.
    pub fn for_path(path: &Path) -> Option<&'static Language> {
        let name = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        LANGUAGES.iter().find(|l| l.file_names.contains(&name) || l.extensions.contains(&ext))
    }

    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        if text.len() > MAX_HIGHLIGHT {
            return vec![];
        }
        let Some(config) = self.loaded.get_or_init(self.config).as_ref() else { return vec![] };
        let mut highlighter = Highlighter::new();
        let Ok(events) = highlighter.highlight(config, text.as_bytes(), None, None, |_| None) else { return vec![] };
        let mut spans = vec![];
        let mut stack: Vec<usize> = vec![];
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(h.0),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Some(&scope) = stack.last() {
                        spans.push(Span { range: start..end, scope });
                    }
                }
                Err(_) => return vec![],
            }
        }
        spans
    }
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_loads() {
        for language in &LANGUAGES {
            assert!((language.config)().is_some(), "{}", language.name);
        }
    }

    #[test]
    fn highlights_rust() {
        let rust = Language::for_path(Path::new("src/main.rs")).unwrap();
        let spans = rust.highlight("fn main() { let x = 1; }");
        let scope_at = |at: usize| spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope]);
        assert_eq!(scope_at(0), Some("keyword"));
        assert_eq!(scope_at(3).map(|s| s.starts_with("function")), Some(true));
        assert!(Language::for_path(Path::new("notes.txt")).is_none());
    }
}
