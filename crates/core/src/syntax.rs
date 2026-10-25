//! Syntax highlighting with tree-sitter: the highlighted spans of a
//! [`Language`]'s text, in scopes the themes know.

use std::ops::Range;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::language::{LANGUAGES, Language};

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

/// What other editors' queries call scopes (Helix, nvim-treesitter), as
/// ours, so a query from anywhere colors without being rewritten. A name
/// not here falls back to its shorter forms (`keyword.control.import` →
/// `keyword`).
const ALIASES: &[(&str, &str)] = &[
    ("character", "string.special"),
    ("constant.builtin.boolean", "boolean"),
    ("constant.character", "string.special"),
    ("constant.character.escape", "escape"),
    ("constant.numeric", "number"),
    ("markup.bold", "text.strong"),
    ("markup.heading", "text.title"),
    ("markup.heading.marker", "punctuation.special"),
    ("markup.italic", "text.emphasis"),
    ("markup.link", "text.reference"),
    ("markup.link.url", "text.uri"),
    ("markup.list", "punctuation.special"),
    ("markup.quote", "text.literal"),
    ("markup.raw", "text.literal"),
    ("markup.strong", "text.strong"),
    ("namespace", "module"),
    ("special", "punctuation.special"),
    ("string.special.url", "text.uri"),
    ("type.enum.variant", "constructor"),
    ("variable.member", "property"),
    ("variable.other.member", "property"),
];

/// Files bigger than this aren't highlighted (it's redone on every change).
const MAX_HIGHLIGHT: usize = 1 << 20;

/// A highlighted stretch of text and its scope (an index into [`SCOPES`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub scope: usize,
}

/// A language's highlight query, compiled, and the scope each capture it
/// reports shows as.
pub(crate) struct Highlights {
    config: HighlightConfiguration,
    scopes: Vec<usize>,
}

/// Compile `query` for `grammar`; `None` (and a complaint) if it doesn't load.
pub(crate) fn highlights(name: &str, grammar: impl Into<tree_sitter::Language>, query: &str) -> Option<Highlights> {
    let mut config =
        HighlightConfiguration::new(grammar.into(), name, query, "", "").inspect_err(|e| eprintln!("the {name} highlight query doesn't load: {e}")).ok()?;
    // Report only the captures that show as something, each as itself.
    let (captures, scopes): (Vec<String>, Vec<usize>) = config.names().iter().filter_map(|c| Some((c.to_string(), scope(c)?))).unzip();
    config.configure(&captures);
    Some(Highlights { config, scopes })
}

/// The scope a capture shows as: its name, or what it's an alias of, or
/// the same for a shorter form of it.
fn scope(capture: &str) -> Option<usize> {
    let mut name = capture;
    loop {
        let ours = ALIASES.iter().find(|(theirs, _)| *theirs == name).map_or(name, |(_, ours)| ours);
        if let Some(scope) = SCOPES.iter().position(|s| *s == ours) {
            return Some(scope);
        }
        name = &name[..name.rfind('.')?];
    }
}

impl Language {
    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        static LOADED: [OnceLock<Option<Highlights>>; LANGUAGES.len()] = [const { OnceLock::new() }; LANGUAGES.len()];
        if text.len() > MAX_HIGHLIGHT {
            return vec![];
        }
        let Some(ix) = LANGUAGES.iter().position(|l| std::ptr::eq(l, self)) else { return vec![] };
        let Some(Highlights { config, scopes }) = LOADED[ix].get_or_init(self.load).as_ref() else { return vec![] };
        let mut highlighter = Highlighter::new();
        let Ok(events) = highlighter.highlight(config, text.as_bytes(), None, None, |_| None) else { return vec![] };
        let mut spans = vec![];
        let mut stack: Vec<usize> = vec![];
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(scopes[h.0]),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn every_query_loads() {
        for language in &LANGUAGES {
            assert!((language.load)().is_some(), "{}", language.name);
        }
    }

    /// The scope at each of `at` in `text`, highlighted as `path`.
    fn scopes_at(path: &str, text: &str, at: &[usize]) -> Vec<Option<&'static str>> {
        let spans = Language::for_path(Path::new(path)).unwrap().highlight(text);
        at.iter().map(|&at| spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope])).collect()
    }

    #[test]
    fn highlights_rust() {
        let [keyword, name] = scopes_at("src/main.rs", "fn main() { let x = 1; }", &[0, 3])[..] else { unreachable!() };
        assert_eq!(keyword, Some("keyword"));
        assert_eq!(name.map(|s| s.starts_with("function")), Some(true));
    }

    #[test]
    fn highlights_typst_through_aliases() {
        let text = "= Title\n#let x = 1\n*bold*";
        let at = [2, text.find("let").unwrap(), text.find("1").unwrap(), text.find("bold").unwrap()];
        assert_eq!(scopes_at("paper.typ", text, &at), [Some("text.title"), Some("keyword"), Some("number"), Some("text.strong")]);
    }

    #[test]
    fn aliases_and_shorter_forms() {
        let name = |capture| scope(capture).map(|s| SCOPES[s]);
        assert_eq!(name("keyword.control.import"), Some("keyword"));
        assert_eq!(name("markup.heading.2"), Some("text.title"));
        assert_eq!(name("constant.numeric.float"), Some("number"));
        assert_eq!(name("function.method.call"), Some("function.method"));
        assert_eq!(name("spell"), None);
    }
}
