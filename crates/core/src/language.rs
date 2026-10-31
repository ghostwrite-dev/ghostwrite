//! The languages Ghostwrite knows, one entry each: which files are in it,
//! how it's highlighted (see [`syntax`](crate::syntax)), and which language
//! servers can serve it. Adding a language means adding an entry here.

use std::path::Path;

use crate::syntax::{Highlights, highlights, with_injections};

/// Spans of text and their scopes' names.
pub(crate) type Lex = fn(&str) -> Vec<(std::ops::Range<usize>, &'static str)>;

pub struct Language {
    /// What the status line and Claude call it.
    pub name: &'static str,
    /// Its LSP language id, when that isn't `name`.
    id: &'static str,
    pub(crate) extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    /// The language servers that can serve it, most preferred first, by
    /// their names in `ghostwrite_lsp`'s registry.
    pub servers: &'static [&'static str],
    /// The grammar and its highlight query, compiled.
    pub(crate) load: fn() -> Option<Highlights>,
    /// Highlights without a grammar.
    pub(crate) lex: Option<Lex>,
    /// Whether `\alpha` types `α` (see [`crate::abbreviation`]).
    pub abbreviations: bool,
    /// Quotes that don't close themselves here (Lean's `` `name ``).
    pub unpaired: &'static [char],
    /// What starts a line comment (`ctrl-c`), if there's such a thing.
    pub comment: Option<&'static str>,
}

impl Language {
    const DEFAULT: Language = Language {
        name: "",
        id: "",
        extensions: &[],
        file_names: &[],
        servers: &[],
        load: || None,
        lex: None,
        abbreviations: false,
        unpaired: &[],
        comment: None,
    };

    /// The language of `path`, by its name or extension.
    pub fn for_path(path: &Path) -> Option<&'static Language> {
        let name = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        LANGUAGES.iter().find(|l| l.file_names.contains(&name) || l.extensions.contains(&ext))
    }

    /// Its LSP language id.
    pub fn id(&self) -> &'static str {
        if self.id.is_empty() { self.name } else { self.id }
    }
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

const PYTHON_SERVERS: &[&str] = &["basedpyright", "pyright", "pylsp"];
const TYPESCRIPT_SERVERS: &[&str] = &["typescript-language-server"];

pub static LANGUAGES: [Language; 17] = [
    Language {
        name: "rust",
        comment: Some("//"),
        extensions: &["rs"],
        servers: &["rust-analyzer"],
        // `'a`: a lifetime, or a char that's closed by hand.
        unpaired: &['\''],
        load: || highlights("rust", tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "python",
        comment: Some("#"),
        extensions: &["py", "pyi"],
        servers: PYTHON_SERVERS,
        load: || highlights("python", tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "javascript",
        comment: Some("//"),
        extensions: &["js", "mjs", "cjs"],
        servers: TYPESCRIPT_SERVERS,
        load: || highlights("javascript", tree_sitter_javascript::LANGUAGE, tree_sitter_javascript::HIGHLIGHT_QUERY),
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
        ..Language::DEFAULT
    },
    Language {
        name: "bash",
        comment: Some("#"),
        id: "shellscript",
        extensions: &["sh", "bash", "zsh"],
        file_names: &[".bashrc", ".zshrc", ".profile"],
        load: || highlights("bash", tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "toml",
        comment: Some("#"),
        extensions: &["toml"],
        file_names: &["Cargo.lock"],
        load: || highlights("toml", tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
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
        ..Language::DEFAULT
    },
    Language {
        name: "json",
        extensions: &["json"],
        load: || highlights("json", tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "yaml",
        comment: Some("#"),
        extensions: &["yaml", "yml"],
        load: || highlights("yaml", tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "go",
        comment: Some("//"),
        extensions: &["go"],
        servers: &["gopls"],
        load: || highlights("go", tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "c",
        comment: Some("//"),
        extensions: &["c", "h"],
        servers: &["clangd"],
        load: || highlights("c", tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
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
        ..Language::DEFAULT
    },
    Language {
        name: "css",
        extensions: &["css"],
        load: || highlights("css", tree_sitter_css::LANGUAGE, tree_sitter_css::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "typst",
        comment: Some("//"),
        extensions: &["typ"],
        servers: &["tinymist"],
        // The crate doesn't export its query; this is its queries/typst/highlights.scm.
        load: || highlights("typst", codebook_tree_sitter_typst::LANGUAGE, include_str!("../queries/typst/highlights.scm")),
        ..Language::DEFAULT
    },
    Language {
        name: "lean",
        comment: Some("--"),
        extensions: &["lean"],
        servers: &["lean"],
        // No tree-sitter grammar for Lean fits; it's lexed, as its VS Code extension does.
        lex: Some(crate::lean::highlight),
        abbreviations: true,
        unpaired: &['`'],
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
        assert_eq!(id("paper.typ"), Some("typst"));
        assert_eq!(id("Basic.lean"), Some("lean"));
        assert_eq!(id("Cargo.lock"), Some("toml"));
        assert_eq!(id("notes.txt"), None);
    }
}
