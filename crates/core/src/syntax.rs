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

/// A language's highlight query, compiled.
pub(crate) struct Highlights {
    config: HighlightConfiguration,
}

/// The capture names every query is set up with (ours, and what other
/// editors call them), and the scope each shows as. All share them, so an
/// injected language's highlights mean the same as its host's.
fn names() -> &'static (Vec<&'static str>, Vec<Option<usize>>) {
    static NAMES: OnceLock<(Vec<&'static str>, Vec<Option<usize>>)> = OnceLock::new();
    NAMES.get_or_init(|| {
        let names: Vec<&str> = SCOPES.iter().copied().chain(ALIASES.iter().map(|(theirs, _)| *theirs)).collect();
        let scopes = names.iter().map(|n| scope(n)).collect();
        (names, scopes)
    })
}

/// Compile `query` for `grammar`; `None` (and a complaint) if it doesn't load.
pub(crate) fn highlights(name: &str, grammar: impl Into<tree_sitter::Language>, query: &str) -> Option<Highlights> {
    with_injections(name, grammar, query, "")
}

/// [`highlights`], with other languages inside it (`injections`, a query:
/// Markdown's code blocks and inline text) highlighted as themselves.
pub(crate) fn with_injections(name: &str, grammar: impl Into<tree_sitter::Language>, query: &str, injections: &str) -> Option<Highlights> {
    let mut config = HighlightConfiguration::new(grammar.into(), name, query, injections, "")
        .inspect_err(|e| eprintln!("the {name} highlight query doesn't load: {e}"))
        .ok()?;
    // Report only the captures that show as something, each as itself.
    // tree-sitter matches a capture to the longest of these it starts with (`keyword.control` → `keyword`).
    config.configure(&names().0);
    Some(Highlights { config })
}

/// Markdown's inline grammar (emphasis, code, links), injected into its blocks.
fn markdown_inline() -> Option<&'static Highlights> {
    static INLINE: OnceLock<Option<Highlights>> = OnceLock::new();
    INLINE.get_or_init(|| highlights("markdown_inline", tree_sitter_md::INLINE_LANGUAGE, tree_sitter_md::HIGHLIGHT_QUERY_INLINE)).as_ref()
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

/// The scope called `name` (or its alias, or a shorter form), as [`Span::scope`] numbers it.
pub fn scope_named(name: &str) -> Option<usize> {
    scope(name)
}

/// `top` laid over `base` (both in order, not overlapping): where a span of
/// `top` is, it wins; `base` shows around it.
pub fn overlay(base: &[Span], top: &[Span]) -> Vec<Span> {
    let mut out = Vec::with_capacity(base.len() + top.len());
    let mut tops = top.iter().peekable();
    // Where the last `top` span ended: `base` shows only after it.
    let mut covered = 0;
    for span in base {
        let mut from = span.range.start.max(covered);
        // The spans of `top` before or inside this one.
        while let Some(t) = tops.next_if(|t| t.range.start < span.range.end) {
            if t.range.start > from {
                out.push(Span { range: from..t.range.start, scope: span.scope });
            }
            (from, covered) = (from.max(t.range.end), t.range.end);
            out.push(t.clone());
        }
        if from < span.range.end {
            out.push(Span { range: from..span.range.end, scope: span.scope });
        }
    }
    out.extend(tops.cloned());
    out
}

/// `spans` of `old` moved onto `new`, which is `old` edited in one place:
/// those before the edit stay, those after it move with the text, and
/// those it touched are gone.
pub fn follow_edit(spans: &[Span], old: &str, new: &str) -> Vec<Span> {
    let prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old.bytes().rev().zip(new.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
    let (edited_end, shift) = (old.len() - suffix, new.len() as isize - old.len() as isize);
    spans
        .iter()
        .filter_map(|s| {
            if s.range.end <= prefix {
                Some(s.clone())
            } else if s.range.start >= edited_end {
                let moved = |b: usize| (b as isize + shift) as usize;
                Some(Span { range: moved(s.range.start)..moved(s.range.end), scope: s.scope })
            } else {
                None
            }
        })
        .collect()
}

/// Language `ix`'s highlights, compiled the first time they're needed.
fn loaded(ix: usize) -> Option<&'static Highlights> {
    static LOADED: [OnceLock<Option<Highlights>>; LANGUAGES.len()] = [const { OnceLock::new() }; LANGUAGES.len()];
    LOADED[ix].get_or_init(LANGUAGES[ix].load).as_ref()
}

/// What's injected into a language: Markdown's inline text, or a code
/// block's language by its name (`rust`, `py`, `typescript`…).
fn injected<'a>(name: &str) -> Option<&'a HighlightConfiguration> {
    if name == "markdown_inline" {
        return markdown_inline().map(|h| &h.config);
    }
    let ix = LANGUAGES.iter().position(|l| l.name == name || l.extensions.contains(&name) || l.id() == name)?;
    loaded(ix).map(|h| &h.config)
}

impl Language {
    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        if text.len() > MAX_HIGHLIGHT {
            return vec![];
        }
        if let Some(lex) = self.lex {
            return lex(text).into_iter().filter_map(|(range, name)| Some(Span { range, scope: scope(name)? })).collect();
        }
        let Some(ix) = LANGUAGES.iter().position(|l| std::ptr::eq(l, self)) else { return vec![] };
        let Some(Highlights { config }) = loaded(ix) else { return vec![] };
        let scopes = &names().1;
        let mut highlighter = Highlighter::new();
        let Ok(events) = highlighter.highlight(config, text.as_bytes(), None, None, injected) else { return vec![] };
        let mut spans = vec![];
        let mut stack: Vec<Option<usize>> = vec![];
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(scopes[h.0]),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Some(&Some(scope)) = stack.last() {
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
        for language in LANGUAGES.iter().filter(|l| l.lex.is_none()) {
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

    fn spans(list: &[(Range<usize>, usize)]) -> Vec<Span> {
        list.iter().map(|(range, scope)| Span { range: range.clone(), scope: *scope }).collect()
    }

    #[test]
    fn the_servers_spans_win_over_the_grammars() {
        let base = spans(&[(0..10, 1), (12..20, 2)]);
        let top = spans(&[(2..4, 7), (8..14, 8), (25..30, 9)]);
        assert_eq!(overlay(&base, &top), spans(&[(0..2, 1), (2..4, 7), (4..8, 1), (8..14, 8), (14..20, 2), (25..30, 9)]));
        assert_eq!(overlay(&base, &[]), base);
    }

    #[test]
    fn spans_follow_an_edit() {
        let old = "let a = b;";
        let new = "let a = xyz + b;";
        let moved = follow_edit(&spans(&[(0..3, 1), (4..5, 2), (8..9, 3)]), old, new);
        assert_eq!(moved, spans(&[(0..3, 1), (4..5, 2), (14..15, 3)]));
        assert_eq!(follow_edit(&spans(&[(4..5, 2)]), old, "let xb = b;"), spans(&[]), "the edit touched it");
        assert_eq!(follow_edit(&spans(&[(4..5, 2)]), old, "let ab = b;"), spans(&[(4..5, 2)]), "typing on after it leaves it be");
    }

    #[test]
    fn markdown_highlights_inline_text_and_code_blocks() {
        let text = "# Title\n\nsome **bold** and `code`\n\n```rust\nfn main() {}\n```\n";
        let at = [text.find("bold").unwrap(), text.find("code").unwrap(), text.find("fn").unwrap()];
        let [bold, code, keyword] = scopes_at("notes.md", text, &at)[..] else { unreachable!() };
        assert_eq!(bold, Some("text.strong"));
        assert_eq!(code, Some("text.literal"));
        assert_eq!(keyword, Some("keyword"), "the code block is Rust");
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
