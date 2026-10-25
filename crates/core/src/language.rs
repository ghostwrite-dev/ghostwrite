//! The languages Ghostwrite knows, one entry each: which files are in it,
//! how it's highlighted (see [`syntax`](crate::syntax)), and which language
//! servers can serve it. Adding a language means adding an entry here.

use std::path::Path;

use crate::syntax::{Highlights, highlights};

pub struct Language {
    /// What the status line and Claude call it.
    pub name: &'static str,
    /// Its LSP language id, when that isn't `name`.
    id: &'static str,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    /// The language servers that can serve it, most preferred first, by
    /// their names in `ghostwrite_lsp`'s registry.
    pub servers: &'static [&'static str],
    /// The grammar and its highlight query, compiled.
    pub(crate) load: fn() -> Option<Highlights>,
}

impl Language {
    const DEFAULT: Language = Language { name: "", id: "", extensions: &[], file_names: &[], servers: &[], load: || None };

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

pub static LANGUAGES: [Language; 16] = [
    Language {
        name: "rust",
        extensions: &["rs"],
        servers: &["rust-analyzer"],
        load: || highlights("rust", tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "python",
        extensions: &["py", "pyi"],
        servers: PYTHON_SERVERS,
        load: || highlights("python", tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "javascript",
        extensions: &["js", "mjs", "cjs"],
        servers: TYPESCRIPT_SERVERS,
        load: || highlights("javascript", tree_sitter_javascript::LANGUAGE, tree_sitter_javascript::HIGHLIGHT_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "jsx",
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
        id: "shellscript",
        extensions: &["sh", "bash", "zsh"],
        file_names: &[".bashrc", ".zshrc", ".profile"],
        load: || highlights("bash", tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "toml",
        extensions: &["toml"],
        file_names: &["Cargo.lock"],
        load: || highlights("toml", tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "markdown",
        extensions: &["md", "markdown"],
        load: || highlights("markdown", tree_sitter_md::LANGUAGE, tree_sitter_md::HIGHLIGHT_QUERY_BLOCK),
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
        extensions: &["yaml", "yml"],
        load: || highlights("yaml", tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "go",
        extensions: &["go"],
        servers: &["gopls"],
        load: || highlights("go", tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "c",
        extensions: &["c", "h"],
        servers: &["clangd"],
        load: || highlights("c", tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
        ..Language::DEFAULT
    },
    Language {
        name: "cpp",
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
        extensions: &["typ"],
        servers: &["tinymist"],
        // The crate doesn't export its query; this is its queries/typst/highlights.scm.
        load: || highlights("typst", codebook_tree_sitter_typst::LANGUAGE, include_str!("../queries/typst/highlights.scm")),
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
        assert_eq!(id("Cargo.lock"), Some("toml"));
        assert_eq!(id("notes.txt"), None);
    }
}
