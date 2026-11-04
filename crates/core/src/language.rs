//! The languages Ghostwrite knows, one entry each: which files are in it,
//! how it's highlighted (see [`syntax`](crate::syntax)), which language
//! servers can serve it, and what's particular about editing it. Adding a
//! language means adding an entry here, or a plugin registering one; nothing
//! else asks a language's name.

use std::path::Path;
use std::sync::RwLock;

use crate::abbreviation::Abbreviations;
use crate::syntax::{Highlights, Span, highlights, with_injections};

/// Highlights a file by whatever means it has (a grammar a plugin brought, say).
pub type Highlighter = dyn Fn(&str) -> Vec<Span> + Send + Sync;

/// Spans of text and their scopes' names.
pub type Lex = fn(&str) -> Vec<(std::ops::Range<usize>, &'static str)>;

pub struct Language {
    /// What the status line and Claude call it.
    pub name: &'static str,
    /// Its LSP language id, when that isn't `name`.
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    pub file_names: &'static [&'static str],
    /// The language servers that can serve it, most preferred first, by
    /// their names in `ghostwrite_lsp`'s registry.
    pub servers: &'static [&'static str],
    /// The grammar and its highlight query, compiled.
    pub load: fn() -> Option<Highlights>,
    /// Highlights without a grammar.
    pub lex: Option<Lex>,
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
    /// What a line ends with when the next one goes in a level (Python's `:`).
    pub indent_after: &'static [char],
    /// Its grammar, for selecting by the syntax tree (`alt-o`, `maf`): see
    /// [`nodes`](crate::nodes). `None` where it's highlighted another way.
    pub grammar: Option<fn() -> tree_sitter::Language>,
    /// Its text objects (`maf`, `]f`): a query capturing `function.around`,
    /// `parameter.inside` and the like, as helix names them.
    pub textobjects: &'static str,
}

/// Languages plugins added.
static REGISTERED: RwLock<Vec<&'static Language>> = RwLock::new(Vec::new());

impl Language {
    pub const DEFAULT: Language = Language {
        name: "",
        id: "",
        extensions: &[],
        file_names: &[],
        servers: &[],
        load: || None,
        lex: None,
        highlighter: None,
        only_pages: false,
        pages_at_own_size: false,
        abbreviations: None,
        unpaired: &[],
        pairs: &[],
        comment: None,
        indent_after: &['{', '[', '('],
        grammar: None,
        textobjects: "",
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
        LANGUAGES.iter().chain(REGISTERED.read().unwrap_or_else(|e| e.into_inner()).iter().copied()).collect()
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

const TYPESCRIPT_SERVERS: &[&str] = &["typescript-language-server"];

pub static LANGUAGES: [Language; 15] = [
    Language {
        name: "rust",
        comment: Some("//"),
        extensions: &["rs"],
        servers: &["rust-analyzer"],
        // `'a`: a lifetime, or a char that's closed by hand.
        unpaired: &['\''],
        load: || highlights("rust", tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_rust::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/rust.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "python",
        comment: Some("#"),
        extensions: &["py", "pyi"],
        servers: &["basedpyright", "pyright", "pylsp"],
        indent_after: &['{', '[', '(', ':'],
        load: || highlights("python", tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_python::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/python.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "javascript",
        comment: Some("//"),
        extensions: &["js", "mjs", "cjs"],
        servers: TYPESCRIPT_SERVERS,
        load: || highlights("javascript", tree_sitter_javascript::LANGUAGE, tree_sitter_javascript::HIGHLIGHT_QUERY),
        grammar: Some(|| tree_sitter_javascript::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/javascript.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "jsx",
        comment: Some("//"),
        id: "javascriptreact",
        extensions: &["jsx"],
        servers: TYPESCRIPT_SERVERS,
        load: || {
            let query = [tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n");
            highlights("jsx", tree_sitter_javascript::LANGUAGE, &query)
        },
        grammar: Some(|| tree_sitter_javascript::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/jsx.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "typescript",
        comment: Some("//"),
        extensions: &["ts", "mts", "cts"],
        servers: TYPESCRIPT_SERVERS,
        load: || {
            let query = [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n");
            highlights("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT, &query)
        },
        grammar: Some(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        textobjects: include_str!("../queries/textobjects/typescript.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "tsx",
        comment: Some("//"),
        id: "typescriptreact",
        extensions: &["tsx"],
        servers: TYPESCRIPT_SERVERS,
        load: || {
            let query =
                [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n");
            highlights("tsx", tree_sitter_typescript::LANGUAGE_TSX, &query)
        },
        grammar: Some(|| tree_sitter_typescript::LANGUAGE_TSX.into()),
        textobjects: include_str!("../queries/textobjects/tsx.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "bash",
        comment: Some("#"),
        id: "shellscript",
        extensions: &["sh", "bash", "zsh"],
        file_names: &[".bashrc", ".zshrc", ".profile"],
        load: || highlights("bash", tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY),
        grammar: Some(|| tree_sitter_bash::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/bash.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "toml",
        comment: Some("#"),
        extensions: &["toml"],
        file_names: &["Cargo.lock"],
        load: || highlights("toml", tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_toml_ng::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/toml.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "markdown",
        extensions: &["md", "markdown"],
        load: || {
            // Inline text is a node with children (its words): the injection takes them all in.
            let injections = tree_sitter_md::INJECTION_QUERY_BLOCK.replace(
                r#"(#set! injection.language "markdown_inline")"#,
                r#"(#set! injection.language "markdown_inline") (#set! injection.include-children)"#,
            );
            with_injections("markdown", tree_sitter_md::LANGUAGE, tree_sitter_md::HIGHLIGHT_QUERY_BLOCK, &injections)
        },
        grammar: Some(|| tree_sitter_md::LANGUAGE.into()),
        ..Language::DEFAULT
    },
    Language {
        name: "json",
        extensions: &["json"],
        load: || highlights("json", tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_json::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/json.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "yaml",
        comment: Some("#"),
        extensions: &["yaml", "yml"],
        load: || highlights("yaml", tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_yaml::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/yaml.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "go",
        comment: Some("//"),
        extensions: &["go"],
        servers: &["gopls"],
        load: || highlights("go", tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_go::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/go.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "c",
        comment: Some("//"),
        extensions: &["c", "h"],
        servers: &["clangd"],
        load: || highlights("c", tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
        grammar: Some(|| tree_sitter_c::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/c.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "cpp",
        comment: Some("//"),
        extensions: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        servers: &["clangd"],
        load: || {
            let query = [tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY].join("\n");
            highlights("cpp", tree_sitter_cpp::LANGUAGE, &query)
        },
        grammar: Some(|| tree_sitter_cpp::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/cpp.scm"),
        ..Language::DEFAULT
    },
    Language {
        name: "css",
        extensions: &["css"],
        load: || highlights("css", tree_sitter_css::LANGUAGE, tree_sitter_css::HIGHLIGHTS_QUERY),
        grammar: Some(|| tree_sitter_css::LANGUAGE.into()),
        textobjects: include_str!("../queries/textobjects/css.scm"),
        ..Language::DEFAULT
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_languages_by_name_and_extension() {
        let id = |path: &str| Language::for_path(Path::new(path)).map(Language::id);
        assert_eq!(id("src/main.rs"), Some("rust"));
        assert_eq!(id("App.tsx"), Some("typescriptreact"));
        assert_eq!(id("Cargo.lock"), Some("toml"));
        assert_eq!(id("notes.txt"), None);
    }

    #[test]
    fn comment_leaders_are_as_written() {
        let rust = Language::for_path(Path::new("a.rs")).unwrap();
        assert_eq!(rust.comment_leader("    // x"), Some("//"));
        assert_eq!(rust.comment_leader("/// docs"), Some("///"));
        assert_eq!(rust.comment_leader("//! crate"), Some("//!"));
        assert_eq!(rust.comment_leader("let x; // y"), None);
        let python = Language::for_path(Path::new("a.py")).unwrap();
        assert_eq!(python.comment_leader("## x"), Some("#"));
    }
}
