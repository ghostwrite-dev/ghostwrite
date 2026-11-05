//! Syntax with tree-sitter: a language's [`Grammar`], however it's run (a plugin's,
//! compiled to WebAssembly), its syntax tree, and the highlighted spans of its
//! text, in scopes the themes know.

use std::ops::Range;
use std::sync::{Mutex, OnceLock, RwLock};

use tree_sitter::{Parser, Query, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::language::Language;

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

/// What loads a grammar, and parses with it. A plugin's runtime has one for the
/// grammars it brings, compiled to WebAssembly.
pub trait Load: Send + Sync {
    /// The grammar itself.
    fn language(&self) -> Result<tree_sitter::Language, String>;
    /// A parser that can parse with it.
    fn parser(&self) -> Result<Parser, String>;
}

/// A grammar, with its queries: what colors a language (`highlights`), the other
/// languages inside it (`injections`: a code block's, say), and its text objects
/// (`textobjects`: what `maf` and `]f` select, as helix names them). It's loaded,
/// and its queries compiled, when it's first needed.
pub struct Grammar {
    pub name: String,
    load: Box<dyn Load>,
    highlights: String,
    injections: String,
    textobjects: String,
    loaded: OnceLock<Result<Loaded, String>>,
}

struct Loaded {
    language: tree_sitter::Language,
    highlights: HighlightConfiguration,
    textobjects: Option<Query>,
    /// Highlighters and parsers not in use, to use again: making one can take a while.
    highlighters: Mutex<Vec<Highlighter>>,
    parsers: Mutex<Vec<Parser>>,
}

/// Grammars there to be injected only (a markup language's inline text, say), not a language's own.
static GRAMMARS: RwLock<Vec<&'static Grammar>> = RwLock::new(Vec::new());

impl Grammar {
    pub fn new(name: &str, load: Box<dyn Load>, highlights: &str, injections: &str, textobjects: &str) -> Self {
        Self {
            name: name.to_string(),
            load,
            highlights: highlights.to_string(),
            injections: injections.to_string(),
            textobjects: textobjects.to_string(),
            loaded: OnceLock::new(),
        }
    }

    /// Add a grammar that's injected by its name, and isn't a language's.
    pub fn register(grammar: &'static Grammar) {
        GRAMMARS.write().unwrap_or_else(|e| e.into_inner()).push(grammar);
    }

    pub fn unregister(grammar: &'static Grammar) {
        GRAMMARS.write().unwrap_or_else(|e| e.into_inner()).retain(|known| !std::ptr::eq(*known, grammar));
    }

    fn loaded(&self) -> Result<&Loaded, &str> {
        let loaded = self.loaded.get_or_init(|| {
            let language = self.load.language()?;
            let mut highlights = HighlightConfiguration::new(language.clone(), &self.name, &self.highlights, &self.injections, "")
                .map_err(|e| format!("its highlight query doesn't fit it: {e}"))?;
            // Report only the captures that show as something, each as itself.
            // tree-sitter matches a capture to the longest of these it starts with (`keyword.control` → `keyword`).
            highlights.configure(&names().0);
            let textobjects = (!self.textobjects.is_empty())
                .then(|| Query::new(&language, &self.textobjects).map_err(|e| format!("its text-object query doesn't fit it: {e}")))
                .transpose()?;
            Ok(Loaded { language, highlights, textobjects, highlighters: Mutex::default(), parsers: Mutex::default() })
        });
        loaded.as_ref().map_err(|e| {
            static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
            let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
            if !said.contains(&self.name) {
                eprintln!("the {} grammar can't be used: {e}", self.name);
                said.push(self.name.clone());
            }
            e.as_str()
        })
    }

    /// Whether it loads, its queries fit it and it parses, or what's wrong.
    pub fn check(&self) -> Result<(), String> {
        self.loaded()?;
        self.parse("").map(drop).ok_or_else(|| "it doesn't parse".to_string())
    }

    /// `text`'s syntax tree.
    pub fn parse(&self, text: &str) -> Option<Tree> {
        let loaded = self.loaded().ok()?;
        let parser = loaded.parsers.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut parser = match parser {
            Some(parser) => parser,
            None => {
                let mut parser = self.load.parser().inspect_err(|e| eprintln!("the {} grammar can't parse: {e}", self.name)).ok()?;
                parser.set_language(&loaded.language).ok()?;
                parser
            }
        };
        let tree = parser.parse(text, None);
        loaded.parsers.lock().unwrap_or_else(|e| e.into_inner()).push(parser);
        tree
    }

    /// Its text-object query, if it has one.
    pub fn textobjects(&self) -> Option<&Query> {
        self.loaded().ok()?.textobjects.as_ref()
    }

    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        let Ok(loaded) = self.loaded() else { return vec![] };
        let highlighter = loaded.highlighters.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut highlighter = match highlighter {
            Some(highlighter) => highlighter,
            None => {
                let Ok(parser) = self.load.parser() else { return vec![] };
                let mut highlighter = Highlighter::new();
                highlighter.parser = parser;
                highlighter
            }
        };
        let spans = spans(&loaded.highlights, &mut highlighter, text);
        loaded.highlighters.lock().unwrap_or_else(|e| e.into_inner()).push(highlighter);
        spans
    }
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

/// What's injected into a language, by its name: a code block's language
/// (by its name, extension or id), or a grammar there only to be injected.
fn injected<'a>(name: &str) -> Option<&'a HighlightConfiguration> {
    let language = Language::all().into_iter().find(|l| l.grammar.is_some() && (l.name == name || l.extensions.contains(&name) || l.id() == name));
    let grammar = match language {
        Some(language) => language.grammar?,
        None => *GRAMMARS.read().unwrap_or_else(|e| e.into_inner()).iter().find(|grammar| grammar.name == name)?,
    };
    grammar.loaded().ok().map(|loaded| &loaded.highlights)
}

impl Language {
    /// The spans of `text`, innermost scope wins, in order.
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        if text.len() > MAX_HIGHLIGHT {
            return vec![];
        }
        if let Some(highlighter) = self.highlighter {
            return highlighter(text);
        }
        self.grammar.map_or_else(Vec::new, |grammar| grammar.highlight(text))
    }
}

/// The spans of `text`, innermost scope wins, in order, parsed by `highlighter`
/// (whose parser can parse with `config`'s grammar).
fn spans(config: &HighlightConfiguration, highlighter: &mut Highlighter, text: &str) -> Vec<Span> {
    let scopes = &names().1;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The scope at each of `at` in `text`, highlighted as `path`.
    fn scopes_at(path: &str, text: &str, at: &[usize]) -> Vec<Option<&'static str>> {
        crate::language::testing::install();
        let spans = Language::for_path(Path::new(path)).unwrap().highlight(text);
        at.iter().map(|&at| spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope])).collect()
    }

    #[test]
    fn highlights_rust() {
        let [keyword, name] = scopes_at("src/main.rs", "fn main() { let x = 1; }", &[0, 3])[..] else { unreachable!() };
        assert_eq!(keyword, Some("keyword"));
        assert_eq!(name.map(|s| s.starts_with("function")), Some(true));
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
    fn aliases_and_shorter_forms() {
        let name = |capture| scope(capture).map(|s| SCOPES[s]);
        assert_eq!(name("keyword.control.import"), Some("keyword"));
        assert_eq!(name("markup.heading.2"), Some("text.title"));
        assert_eq!(name("constant.numeric.float"), Some("number"));
        assert_eq!(name("function.method.call"), Some("function.method"));
        assert_eq!(name("spell"), None);
    }
}
