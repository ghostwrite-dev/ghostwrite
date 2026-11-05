//! The plugins there are. None is built in: those that come with the app (every
//! language, PDFs, images) are WebAssembly in a folder beside it, and the user's
//! are in a folder of their own.

/// Where the user's plugins are.
pub fn folder() -> Option<std::path::PathBuf> {
    Some(crate::config::path()?.parent()?.join("plugins"))
}

/// Where the plugins that come with the app are: as the installers put them, or
/// where `scripts/build-plugins.sh` does, when it's run from the source.
fn bundled() -> Option<std::path::PathBuf> {
    if let Some(folder) = std::env::var_os("GHOSTWRITE_BUNDLED_PLUGINS") {
        return Some(folder.into());
    }
    let exe = std::env::current_exe().ok()?;
    let beside = exe.parent()?;
    let installed = [beside.join("../Resources/plugins"), beside.join("../share/ghostwrite/plugins"), beside.join("plugins")];
    installed.into_iter().find(|folder| folder.is_dir()).or_else(|| {
        let built = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins");
        (cfg!(debug_assertions) && built.is_dir()).then_some(built)
    })
}

/// Start with them: what they add is there from now on.
pub fn install() {
    #[cfg(feature = "wasm-plugins")]
    ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
    ghostwrite_plugin::install(|| {
        let mut plugins: Vec<Box<dyn ghostwrite_plugin::Plugin>> = vec![];
        // Those from the plugins folder.
        if let Some(folder) = folder() {
            let (found, wrong) = ghostwrite_plugin::manifest::Manifest::in_folder(&folder);
            *NOT_READ.lock().unwrap_or_else(|e| e.into_inner()) = wrong;
            for manifest in found {
                plugins.push(Box::new(manifest));
            }
        }
        // And those that come with the app, unless they're there already.
        if let Some(folder) = bundled() {
            let (found, wrong) = ghostwrite_plugin::manifest::Manifest::in_folder(&folder);
            NOT_READ.lock().unwrap_or_else(|e| e.into_inner()).extend(wrong);
            for manifest in found {
                if plugins.iter().all(|plugin| plugin.name() != manifest.name) {
                    plugins.push(Box::new(manifest));
                }
            }
        }
        plugins
    });
}

/// What's wrong with the plugins that couldn't be read.
static NOT_READ: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(vec![]);

/// `:plugins`: what's there, what each adds, and what's wrong with the rest.
pub fn list() -> String {
    let there: Vec<String> =
        ghostwrite_plugin::listed().into_iter().map(|(name, adds)| if adds.is_empty() { name } else { format!("{name} ({adds})") }).collect();
    let read = if there.is_empty() { "no plugins: :plugin install ‹folder or repository›".to_string() } else { format!("plugins: {}", there.join(", ")) };
    let wrong = NOT_READ.lock().unwrap_or_else(|e| e.into_inner());
    if wrong.is_empty() { read } else { format!("{read}. Not read: {}", wrong.join("; ")) }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ghostwrite_core::Language;

    /// What each adds is there, by way of the plugin interface.
    #[test]
    fn what_they_add_is_there() {
        super::install();
        let language = |path: &str| Language::for_path(Path::new(path)).expect(path);
        assert!(ghostwrite_plugin::renders(language("paper.typ")), "pages");
        assert!(ghostwrite_plugin::panel(language("paper.typ")).is_none());
        assert!(ghostwrite_plugin::panel(language("Basic.lean")).is_some(), "goals");
        assert!(!ghostwrite_plugin::renders(language("Basic.lean")));
        assert!(ghostwrite_plugin::panel(language("main.rs")).is_none() && !ghostwrite_plugin::renders(language("main.rs")), "and nothing for the rest");
        assert!(language("Basic.lean").abbreviations.is_some(), "symbols");
    }

    /// Nothing built in knows them, nor any language: what they need goes through the
    /// plugin interface. Built in, a file is plain text.
    #[test]
    fn only_the_plugins_name_themselves() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = vec![];
        for krate in ["core", "lsp", "dap", "ai", "project", "terminal", "page", "model", "plugin", "plugin-api", "wasm", "view", "app"] {
            walk(&root.join(krate).join("src"), &mut found);
        }
        assert!(found.is_empty(), "these name a plugin or a language:\n{}", found.join("\n"));
    }

    /// Nor is any built in, or built with it: what builds the editor names none that
    /// were (`lean`, and its server, `lake`), in its crates, features or scripts.
    #[test]
    fn none_is_built_in() {
        let top = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = vec![top.join("Cargo.toml"), top.join("check.sh"), top.join("install.sh")];
        for folder in [top.join("crates"), top.join("scripts")] {
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = entry.path();
                files.push(if path.is_dir() { path.join("Cargo.toml") } else { path });
            }
        }
        files.retain(|file| file.is_file());
        assert!(files.len() > 10, "it looked: {files:?}");
        let mut found = vec![];
        for file in &files {
            for (ix, line) in std::fs::read_to_string(file).unwrap_or_default().lines().enumerate() {
                if names(&line.to_lowercase(), &["lean", "lake"]) {
                    found.push(format!("{}:{}: {}", file.display(), ix + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "these build one in:\n{}", found.join("\n"));
    }

    /// Whether `text` has one of `words` in it, as a word, or as one of the editor's crates (`ghostwrite_lean`).
    fn names(text: &str, words: &[&str]) -> bool {
        let text = &text.replace("ghostwrite_", "ghostwrite ");
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        words.iter().any(|name| text.match_indices(name).any(|(at, _)| !word(text[..at].chars().last()) && !word(text[at + name.len()..].chars().next())))
    }

    /// The plugins, the languages that come as plugins, and their servers, as words. Some
    /// names are words too often to look for (`go`, `c`, a `bash` shell, the config's
    /// `json`): they're looked for as extensions, quoted, with the rest.
    const NAMES: &[&str] = &[
        "lean",
        "lake",
        "typst",
        "pdf",
        "rust",
        "python",
        "javascript",
        "typescript",
        "jsx",
        "tsx",
        "toml",
        "markdown",
        "yaml",
        "cpp",
        "css",
        "golang",
        "pyright",
        "basedpyright",
        "pylsp",
        "gopls",
        "clangd",
    ];
    const EXTENSIONS: &[&str] = &["rs", "py", "pyi", "js", "mjs", "ts", "go", "hpp", "md", "yml"];
    /// What says those words about something else: a plugin is written in Rust, its
    /// releases, llama.cpp, LSP's name for a kind of text, and what a model's reply has none of.
    const NOT_LANGUAGES: &[&str] = &["written in rust", "rust releases", "llama.cpp", r#"["plaintext", "markdown"]"#, "no markdown, no code fences"];

    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, found);
                }
            // What models take is about models: `attach.rs` and `web.rs` say which formats they read.
            } else if name.ends_with(".rs") && !["tests.rs", "attach.rs", "web.rs"].contains(&name.as_str()) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                // A file's own tests may use them: those are about the whole app.
                let tests = ["#[cfg(test)]", "#[cfg(all(test"].iter().filter_map(|mark| text.find(mark)).min();
                let text = &text[..tests.unwrap_or(text.len())];
                for (ix, line) in text.lines().enumerate() {
                    let mut lower = line.to_lowercase();
                    for phrase in NOT_LANGUAGES {
                        lower = lower.replace(phrase, "");
                    }
                    let named = names(&lower, NAMES);
                    let quoted = EXTENSIONS.iter().any(|extension| lower.contains(&format!("\"{extension}\"")));
                    if named || quoted {
                        found.push(format!("{}:{}: {}", path.display(), ix + 1, line.trim()));
                    }
                }
            }
        }
    }
}
