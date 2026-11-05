//! What's particular about a language: which files are in it, how it's
//! highlighted (see [`syntax`](crate::syntax)), which language servers can
//! serve it, and what's particular about editing it. The editor has none of
//! its own: a file of none is plain text, and every language is a plugin's,
//! registered as it's added. Nothing else asks a language's name.

use std::path::Path;
use std::sync::RwLock;

use crate::abbreviation::Abbreviations;
use crate::syntax::{Grammar, Span};

/// Highlights a file by whatever means it has (a grammar a plugin brought, say).
pub type Highlighter = dyn Fn(&str) -> Vec<Span> + Send + Sync;

pub struct Language {
    /// What the status line and the model call it.
    pub name: &'static str,
    /// Its LSP language id, when that isn't `name`.
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    pub file_names: &'static [&'static str],
    /// The language servers that can serve it, most preferred first, by
    /// their names in `ghostwrite_lsp`'s registry.
    pub servers: &'static [&'static str],
    /// Its grammar, which highlights it and selects by its syntax tree (`alt-o`,
    /// `maf`: see [`nodes`](crate::nodes)).
    pub grammar: Option<&'static Grammar>,
    /// Highlights that aren't the editor's to work out.
    pub highlighter: Option<&'static Highlighter>,
    /// Its files aren't text: what shows is the pages a plugin makes of them.
    pub only_pages: bool,
    /// Those pages aren't shown bigger than they are.
    pub pages_at_own_size: bool,
    /// What a backslash starts typing (`\alpha` is `α`), if anything.
    pub abbreviations: Option<&'static Abbreviations>,
    /// Quotes that don't close themselves here (a name quoted with `` ` ``, say).
    pub unpaired: &'static [char],
    /// Pairs that close themselves here besides brackets and quotes (`$`…`$` for maths, say).
    pub pairs: &'static [(char, char)],
    /// What starts a line comment (`ctrl-c`), if there's such a thing.
    pub comment: Option<&'static str>,
    /// What a line ends with when the next one goes in a level (a `:`, in some).
    pub indent_after: &'static [char],
}

/// The languages there are: those plugins added.
static REGISTERED: RwLock<Vec<&'static Language>> = RwLock::new(Vec::new());

impl Language {
    pub const DEFAULT: Language = Language {
        name: "",
        id: "",
        extensions: &[],
        file_names: &[],
        servers: &[],
        grammar: None,
        highlighter: None,
        only_pages: false,
        pages_at_own_size: false,
        abbreviations: None,
        unpaired: &[],
        pairs: &[],
        comment: None,
        indent_after: &['{', '[', '('],
    };

    /// The language of `path`, by its name or extension.
    pub fn for_path(path: &Path) -> Option<&'static Language> {
        let name = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        Self::all().into_iter().find(|l| l.file_names.contains(&name) || l.extensions.contains(&ext))
    }

    /// Add a language a plugin knows.
    pub fn register(language: &'static Language) {
        REGISTERED.write().unwrap_or_else(|e| e.into_inner()).push(language);
    }

    /// A language a plugin knew is gone: new files aren't in it (those open keep it).
    pub fn unregister(language: &'static Language) {
        REGISTERED.write().unwrap_or_else(|e| e.into_inner()).retain(|known| !std::ptr::eq(*known, language));
    }

    /// Every language.
    pub fn all() -> Vec<&'static Language> {
        REGISTERED.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Its LSP language id.
    pub fn id(&self) -> &'static str {
        if self.id.is_empty() { self.name } else { self.id }
    }

    /// The line comment `line` starts with (after its indent), as written
    /// there: the token, or a doc comment's longer one (`///`, `//!`).
    pub fn comment_leader<'a>(&self, line: &'a str) -> Option<&'a str> {
        let token = self.comment?;
        let rest = line.trim_start_matches([' ', '\t']);
        let after = rest.strip_prefix(token)?;
        let last = token.chars().next_back()?;
        let longer = token.len() > 1 && after.starts_with([last, '!']);
        Some(&rest[..token.len() + longer as usize])
    }
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Languages for core's own tests, as a plugin would add them: one with a
    //! grammar compiled in, one with only its comments and indentation.

    use super::*;
    use crate::syntax::Load;

    struct CompiledIn(tree_sitter::Language);

    impl Load for CompiledIn {
        fn language(&self) -> Result<tree_sitter::Language, String> {
            Ok(self.0.clone())
        }

        fn parser(&self) -> Result<tree_sitter::Parser, String> {
            Ok(tree_sitter::Parser::new())
        }
    }

    /// A grammar compiled in, with its highlights and these text objects and injections.
    pub fn grammar(name: &str, language: tree_sitter::Language, highlights: &str, injections: &str, textobjects: &str) -> &'static Grammar {
        Box::leak(Box::new(Grammar::new(name, Box::new(CompiledIn(language)), highlights, injections, textobjects)))
    }

    /// `.rs` files, as the bundled plugin has them; `.py` files, without a grammar.
    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let textobjects = include_str!("../../../plugins/rust/textobjects.scm");
            let rust = grammar("rust", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, "", textobjects);
            Language::register(Box::leak(Box::new(Language {
                name: "rust",
                comment: Some("//"),
                extensions: &["rs"],
                unpaired: &['\''],
                grammar: Some(rust),
                ..Language::DEFAULT
            })));
            Language::register(Box::leak(Box::new(Language {
                name: "python",
                comment: Some("#"),
                extensions: &["py"],
                file_names: &["SConstruct"],
                indent_after: &['{', '[', '(', ':'],
                ..Language::DEFAULT
            })));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_languages_by_name_and_extension() {
        testing::install();
        let id = |path: &str| Language::for_path(Path::new(path)).map(Language::id);
        assert_eq!(id("src/main.rs"), Some("rust"));
        assert_eq!(id("SConstruct"), Some("python"));
        assert_eq!(id("notes.txt"), None);
    }

    #[test]
    fn comment_leaders_are_as_written() {
        testing::install();
        let rust = Language::for_path(Path::new("a.rs")).unwrap();
        assert_eq!(rust.comment_leader("    // x"), Some("//"));
        assert_eq!(rust.comment_leader("/// docs"), Some("///"));
        assert_eq!(rust.comment_leader("//! crate"), Some("//!"));
        assert_eq!(rust.comment_leader("let x; // y"), None);
        let python = Language::for_path(Path::new("a.py")).unwrap();
        assert_eq!(python.comment_leader("## x"), Some("#"));
    }

    #[test]
    fn a_grammar_there_only_to_be_injected_is_found_by_its_name() {
        // Comments that are code, colored as such by a grammar registered on its own.
        let inner = testing::grammar("in-comments", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, "", "");
        Grammar::register(inner);
        let injections = r#"((line_comment) @injection.content (#set! injection.language "in-comments"))"#;
        let outer = testing::grammar("outer", tree_sitter_rust::LANGUAGE.into(), "", injections, "");
        static OUTER: std::sync::OnceLock<Language> = std::sync::OnceLock::new();
        let language = OUTER.get_or_init(|| Language { name: "outer", grammar: Some(outer), ..Language::DEFAULT });
        let text = "let a = 1; // fn b() {}\n";
        let keyword = crate::syntax::scope_named("keyword");
        let spans = language.highlight(text);
        assert!(spans.iter().any(|s| &text[s.range.clone()] == "fn" && Some(s.scope) == keyword), "{spans:?}");
        assert!(!spans.iter().any(|s| &text[s.range.clone()] == "let"), "only what's injected is colored: {spans:?}");
        Grammar::unregister(inner);
        assert!(language.highlight(text).is_empty(), "and once it's gone, nothing is");
    }
}
