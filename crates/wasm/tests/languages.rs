//! The languages that come with the editor, each a plugin with no code of its own but
//! Python's: they load, their grammars and queries fit, and they color, select and
//! serve as the editor's own once did. Needs `scripts/build-plugins.sh` to have run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ghostwrite_core::syntax::{SCOPES, scope_named};
use ghostwrite_core::{Buffer, Language, Range, nodes};
use ghostwrite_plugin::Plugin as _;
use ghostwrite_plugin::manifest::Manifest;
use serde_json::Value;

fn bundled() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bundled-plugins")
}

/// They're all there, as the app has them; `false` if they haven't been built.
fn installed() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        ghostwrite_plugin::set_runtime(Box::new(ghostwrite_wasm::Runtime));
        if !bundled().join("rust/tree-sitter-rust.wasm").is_file() {
            return false;
        }
        let (found, wrong) = Manifest::in_folder(&bundled());
        assert!(wrong.is_empty(), "{wrong:?}");
        ghostwrite_plugin::install(|| found.into_iter().map(|plugin| Box::new(plugin) as Box<dyn ghostwrite_plugin::Plugin>).collect());
        true
    })
}

macro_rules! needs_the_plugins {
    () => {
        if !installed() {
            return eprintln!("scripts/build-plugins.sh hasn't run: skipped");
        }
    };
}

fn language(path: &str) -> &'static Language {
    Language::for_path(Path::new(path)).unwrap_or_else(|| panic!("no language for {path}"))
}

/// The scope at each of `at` in `text`, highlighted as `path`.
fn scopes_at(path: &str, text: &str, at: &[&str]) -> Vec<Option<&'static str>> {
    let spans = language(path).highlight(text);
    let at = at.iter().map(|what| text.find(what).unwrap_or_else(|| panic!("{what} in {text}")));
    at.map(|at| spans.iter().find(|s| s.range.contains(&at)).map(|s| SCOPES[s.scope])).collect()
}

#[test]
fn every_one_loads_and_its_grammars_and_queries_fit() {
    needs_the_plugins!();
    let (found, _) = Manifest::in_folder(&bundled());
    for plugin in &found {
        plugin.check().unwrap_or_else(|e| panic!("{}: {e}", plugin.name));
    }
    let names: Vec<&str> = found.iter().flat_map(|plugin| plugin.languages()).map(|language| language.name).collect();
    let languages = ["rust", "python", "javascript", "jsx", "typescript", "tsx", "bash", "toml", "markdown", "json", "yaml", "go", "c", "cpp", "css"];
    for name in languages {
        assert!(names.contains(&name), "{name} is in {names:?}");
        let language = Language::all().into_iter().find(|language| language.name == name).unwrap();
        // Markdown's have none in helix either.
        if name != "markdown" {
            assert!(language.grammar.and_then(|grammar| grammar.textobjects()).is_some(), "{name}'s text objects");
        }
    }
}

#[test]
fn finds_languages_by_name_and_extension() {
    needs_the_plugins!();
    let id = |path: &str| Language::for_path(Path::new(path)).map(Language::id);
    assert_eq!(id("src/main.rs"), Some("rust"));
    assert_eq!(id("App.tsx"), Some("typescriptreact"));
    assert_eq!(id("App.jsx"), Some("javascriptreact"));
    assert_eq!(id("Cargo.lock"), Some("toml"));
    assert_eq!(id(".bashrc"), Some("shellscript"));
    assert_eq!(id("a.h"), Some("c"));
    assert_eq!(id("a.hpp"), Some("cpp"));
    assert_eq!(id("notes.txt"), None);
}

#[test]
fn they_color_as_the_grammars_queries_say() {
    needs_the_plugins!();
    let rust = scopes_at("src/main.rs", "fn main() { let x = 1; }", &["fn", "main"]);
    assert_eq!(rust[0], Some("keyword"));
    assert_eq!(rust[1].map(|s| s.starts_with("function")), Some(true));
    assert_eq!(scopes_at("a.py", "def f():\n    return 1\n", &["return", "1"]), [Some("keyword"), Some("number")]);
    // Queries read one after another: TypeScript's own, then JavaScript's.
    assert_eq!(scopes_at("a.ts", "let n: number = 1;", &["let", "number"]), [Some("keyword"), Some("type.builtin")]);
    assert_eq!(scopes_at("a.cpp", "int main() { return 0; }", &["return", "int"]), [Some("keyword"), Some("type")]);
    assert_eq!(scopes_at("Cargo.lock", "[package]\nname = \"x\"\n", &["\"x\""]), [Some("string")]);
}

#[test]
fn markdown_colors_inline_text_and_code_blocks_by_other_grammars() {
    needs_the_plugins!();
    let text = "# Title\n\nsome **bold** and `code`\n\n```rust\nfn main() {}\n```\n";
    let [bold, code, keyword] = scopes_at("notes.md", text, &["bold", "code`", "fn"])[..] else { unreachable!() };
    assert_eq!(bold, Some("text.strong"), "inline text, by a grammar that isn't a language's");
    assert_eq!(code, Some("text.literal"));
    assert_eq!(keyword, Some("keyword"), "the code block is Rust, by another plugin's grammar");
}

#[test]
fn text_objects_and_the_tree_are_the_plugins_grammars() {
    needs_the_plugins!();
    let code = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn two() -> i32 {\n    add(1, 1)\n}\n";
    let at = |what: &str| Range::point(code.find(what).unwrap());
    let function = nodes::object(language("a.rs"), &Buffer::new(code), at("add(1, 1)"), "function", true).unwrap();
    assert_eq!(&code[function.bytes()], "fn two() -> i32 {\n    add(1, 1)\n}");
    let out = nodes::expand(language("a.rs"), &Buffer::new(code), at("b: i32")).unwrap();
    assert_eq!(&code[out.bytes()], "b: i32");

    let code = "class A:\n    def f(self, x):\n        return x\n";
    let argument = nodes::object(language("a.py"), &Buffer::new(code), Range::point(code.find("x)").unwrap()), "parameter", false).unwrap();
    assert_eq!(&code[argument.bytes()], "x");
    let code = "function f(a, b) { return a; }\nconst g = () => 1;\n";
    let next = nodes::goto_object(language("a.ts"), &Buffer::new(code), Range::point(0), "function", true, 1).unwrap();
    assert_eq!(&code[next.bytes()], "() => 1");
}

#[test]
fn what_else_is_particular_to_a_language_comes_with_it() {
    needs_the_plugins!();
    let (rust, python, json) = (language("a.rs"), language("a.py"), language("a.json"));
    assert_eq!((rust.comment, python.comment, json.comment), (Some("//"), Some("#"), None));
    assert_eq!(rust.unpaired, ['\''], "a lifetime's quote");
    assert!(python.indent_after.contains(&':') && !rust.indent_after.contains(&':'));
    assert_eq!(rust.servers, ["rust-analyzer"]);
    assert_eq!(python.servers, ["basedpyright", "pyright", "pylsp"]);
    assert_eq!((language("a.tsx").servers, language("a.cpp").servers), (&["typescript-language-server"][..], &["clangd"][..]));
}

#[test]
fn pythons_servers_see_the_projects_virtualenv_by_way_of_its_plugin() {
    needs_the_plugins!();
    let python = Manifest::in_own_folder(&bundled().join("python")).unwrap();
    let servers = python.servers();
    assert!(servers.iter().all(|server| server.env.is_some() && server.settings.is_some()), "its code configures them all");
    let (env, settings) = (servers[0].env.unwrap(), servers[0].settings.unwrap());

    let project = ghostwrite_project::scratch_dir("python-project");
    let app = project.join("app");
    std::fs::create_dir_all(app.join(".venv/bin")).unwrap();
    std::fs::write(app.join(".venv/bin/python"), "").unwrap();
    let venv = app.join(".venv");
    let shell: Vec<(String, String)> = vec![("PATH".into(), "/usr/bin".into()), ("HOME".into(), "/home/someone".into())];
    let started = env(&project, &app.join("pkg"), &shell);
    let expected = [("VIRTUAL_ENV", venv.display().to_string()), ("PATH", format!("{}:/usr/bin", venv.join("bin").display()))];
    assert_eq!(started, expected.map(|(k, v)| (k.to_string(), v)), "found from a folder below, and active");
    assert_eq!(settings(&project, &app, &shell, "python")["pythonPath"], venv.join("bin/python").display().to_string());
    assert_eq!(settings(&project, &app, &shell, "basedpyright.analysis")["typeCheckingMode"], "standard");
    assert_eq!(settings(&project, &app, &shell, "pyright")["analysis"]["typeCheckingMode"], "standard");
    assert_eq!(settings(&project, &app, &shell, "elsewhere"), Value::Null);
    std::fs::write(app.join("pyproject.toml"), "[tool.basedpyright]\ntypeCheckingMode = \"strict\"\n").unwrap();
    assert!(settings(&project, &app, &shell, "basedpyright.analysis")["typeCheckingMode"].is_null(), "the project's own choice wins");

    // One that's active is taken at its word.
    let active = [shell.clone(), vec![("VIRTUAL_ENV".into(), "/elsewhere/env".into())]].concat();
    let started: HashMap<String, String> = env(&project, &app, &active).into_iter().collect();
    assert_eq!(started["VIRTUAL_ENV"], "/elsewhere/env");

    // Without one, the server's `python3`.
    let bare = ghostwrite_project::scratch_dir("python-bare");
    let bin = ghostwrite_project::scratch_dir("python-bin");
    std::fs::write(bin.join("python3"), "").unwrap();
    let shell: Vec<(String, String)> = vec![("PATH".into(), bin.display().to_string())];
    assert!(env(&bare, &bare, &shell).is_empty());
    assert_eq!(settings(&bare, &bare, &shell, "python")["pythonPath"], bin.join("python3").display().to_string());
}

#[test]
fn a_language_plugin_needs_no_code() {
    needs_the_plugins!();
    let folder = ghostwrite_project::scratch_dir("no-code");
    for file in ["tree-sitter-json.wasm", "highlights.scm", "textobjects.scm"] {
        std::fs::copy(bundled().join("json").join(file), folder.join(file)).unwrap();
    }
    std::fs::write(folder.join("mine.scm"), "(number) @keyword\n").unwrap();
    let json = r#"{ "name": "data", "api": 2,
        "languages": [
          { "name": "data", "id": "data-files", "extensions": ["data"], "files": ["Datafile"], "comment": "//", "indent_after": "{[",
            "grammar": { "name": "json", "wasm": "tree-sitter-json.wasm", "query": "highlights.scm", "textobjects": "textobjects.scm" } },
          { "name": "data-lines", "extensions": ["datalines"], "servers": ["elsewhere-ls"],
            "grammar": { "name": "json", "wasm": "tree-sitter-json.wasm", "query": ["highlights.scm", "mine.scm"] } } ],
        "servers": [{ "name": "data-ls", "command": ["data-ls", "--stdio"], "roots": ["data.toml"] }] }"#;
    std::fs::write(folder.join("plugin.json"), json).unwrap();
    let plugin = Manifest::in_own_folder(&folder).unwrap();
    plugin.check().expect("it loads");
    let [data, lines] = plugin.languages()[..] else { panic!("two languages") };
    assert_eq!((data.name, data.id(), data.extensions, data.file_names), ("data", "data-files", &["data"][..], &["Datafile"][..]));
    assert_eq!((data.comment, data.indent_after), (Some("//"), &['{', '['][..]));
    assert_eq!((lines.id(), lines.comment, lines.indent_after), ("data-lines", None, Language::DEFAULT.indent_after));
    assert_eq!((data.servers, lines.servers), (&["data-ls"][..], &["elsewhere-ls"][..]), "its own servers, unless it names others");
    assert_eq!(plugin.servers()[0].roots, ["data.toml"]);

    let text = "{\"a\": [1, 2]}";
    let one = text.find('1').unwrap();
    let at = |language: &Language| language.highlight(text).into_iter().find(|s| s.range.contains(&one)).map(|s| s.scope);
    assert_eq!(at(data), scope_named("number"));
    assert_eq!(at(lines), scope_named("keyword"), "the last query read wins");
    let entry = nodes::object(data, &Buffer::new(text), Range::point(one), "entry", true).unwrap();
    assert_eq!(&text[entry.bytes()], "1", "its text objects");
    assert_eq!(nodes::object(lines, &Buffer::new(text), Range::point(one), "entry", true), None, "the other has none");
    let described = plugin.describe();
    assert!(described.contains(".data Datafile files, highlighted") && described.contains("starts `data-ls --stdio`"), "{described}");
}

/// A seeded source of numbers, so a failure can be had again.
struct Seeded(u64);

impl Seeded {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }
}

/// `pieces` typed and text taken out here and there, some undone and redone, a few at
/// a time; each time the colors moved on from the last are those of all of it again.
fn colored_bit_by_bit_as_all_of_it(language: &'static Language, text: &str, pieces: &[&str], seed: u64, rounds: usize) {
    use ghostwrite_core::Text as _;
    use ghostwrite_core::syntax::Colors;
    let mut rng = Seeded(seed);
    let mut buffer = Buffer::new(text);
    let mut colors = Colors::new(language, buffer.version(), buffer.rope().clone());
    assert!(!colors.spans.is_empty());
    for round in 0..rounds {
        for _ in 0..1 + rng.below(3) {
            let at = buffer.floor_boundary(rng.below(buffer.len() + 1));
            match rng.below(10) {
                0..=5 => buffer.edit(at..at, pieces[rng.below(pieces.len())]),
                6..=8 => buffer.edit(at..buffer.floor_boundary(at + 1 + rng.below(12)), ""),
                _ => {
                    buffer.commit(Range::point(0));
                    if rng.below(2) == 0 {
                        buffer.undo()
                    } else {
                        buffer.redo()
                    };
                }
            }
        }
        let edits = buffer.changes_since(colors.version).unwrap();
        colors = colors.update(buffer.version(), buffer.rope().clone(), &edits);
        // Of the same tree (one parsed from the last can recover from errors its own way).
        let whole = language.grammar.unwrap().spans(colors.tree().unwrap(), buffer.rope(), 0..buffer.len());
        if colors.spans != whole {
            let at = colors.spans.iter().zip(&whole).find(|(a, b)| a != b).map_or(0, |(a, b)| a.range.start.min(b.range.start));
            let near = |spans: &[ghostwrite_core::syntax::Span]| {
                spans.iter().filter(|s| s.range.end + 30 > at && s.range.start < at + 30).map(|s| (s.range.clone(), SCOPES[s.scope])).collect::<Vec<_>>()
            };
            panic!("seed {seed}, round {round}, at {at} of {:?}:\n{:?}\n{:?}", buffer.to_string(), near(&colors.spans), near(&whole));
        }
    }
}

#[test]
fn colors_worked_out_bit_by_bit_are_those_of_all_of_it() {
    needs_the_plugins!();
    let rust = include_str!("../src/lib.rs");
    let pieces = ["x", " ", "\n", "{", "}", "(", "\"", "'", "//", "/*", "*/", "fn f() {", "let a = 1;", "#[test]", "/// ", "é", "😀"];
    for seed in 1..=3 {
        colored_bit_by_bit_as_all_of_it(language("a.rs"), &rust[..rust.floor_char_boundary(6000)], &pieces, seed, 40);
    }
    // Inline text and code blocks, by other grammars.
    let markdown = "# Title\n\nsome **bold** and `code`, a [link](http://x.org) *too*\nover lines\n\n```rust\nfn main() { let a = \"s\"; }\n```\n\n- one\n- two **2**\n\n> quoted _text_\n\n```\nplain\n```\n".repeat(6);
    let pieces = ["*", "**", "_", "`", "```", "```rust\n", "\n", "\n\n", "# ", "- ", "> ", "[a](b)", "x", "fn ", "\"", "é"];
    let spans = language("notes.md").highlight(&markdown);
    let has = |scope: &str| spans.iter().any(|s| SCOPES[s.scope] == scope);
    assert!(has("keyword") && has("text.strong"), "the languages inside it are colored");
    for seed in 1..=4 {
        colored_bit_by_bit_as_all_of_it(language("notes.md"), &markdown, &pieces, seed, 40);
    }
}
