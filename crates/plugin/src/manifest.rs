//! A plugin that isn't built in: JSON saying what it adds, read as the editor starts
//! (or as it's installed, [`crate::install`]), so nothing has to be rebuilt. On its
//! own, in `plugins/`, a file can add a language, its language server, keys and rules
//! for the model:
//!
//! ```json
//! {
//!   "name": "zig",
//!   "extensions": ["zig", "zon"],
//!   "comment": "//",
//!   "server": ["zls"],
//!   "keys": [{ "keys": "space z", "command": "term", "what": "the terminal" }],
//!   "rules": "- Keep functions small."
//! }
//! ```
//!
//! As `plugin.json` in a folder of its own, with the files it names beside it, it
//! can also have code and a grammar, which a [`Runtime`](crate::Runtime) runs, saying
//! which version of the plugin interface it was built for:
//!
//! ```json
//! {
//!   "name": "words",
//!   "version": "0.1.0",
//!   "api": 1,
//!   "code": "words.wasm",
//!   "commands": [{ "name": "word-count", "what": "count the file's words" }],
//!   "events": ["saved"],
//!   "extensions": ["wd"],
//!   "grammar": { "name": "words", "wasm": "words-grammar.wasm", "query": "highlights.scm" }
//! }
//! ```
//!
//! Code reaches nothing by itself. What it needs it asks for under `"access"`, and
//! it's shown when it's installed:
//!
//! ```json
//! {
//!   "pages": true,
//!   "only_pages": false,
//!   "highlights": true,
//!   "access": {
//!     "project": true,
//!     "fonts": true,
//!     "cache": true,
//!     "read": ["{data}/some-tool/things"],
//!     "write": ["{cache}/some-tool/things"],
//!     "network": ["example.org"],
//!     "memory_mb": 2048,
//!     "seconds": 60
//!   }
//! }
//! ```
//!
//! `only_pages` is for files that aren't text: they open as their pages, read-only, and
//! the plugin reads the file itself each time they're made. `pages_at_own_size` keeps
//! them from being shown bigger than they are.
//!
//! `project` mounts the project read-only at `/project`, for making pages. `fonts`
//! mounts the system's font folders read-only at `/fonts`. `cache` is a folder of the
//! plugin's own, at `/cache`. `read` and `write` are folders under the user's data
//! and cache folders, mounted at `/data/…` and `/user-cache/…`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use ghostwrite_core::Language;
use ghostwrite_lsp::registry::Spec;
use serde_json::Value;

use crate::{Binding, Code, Context, Event, Plugin, PluginCommand, Renderer, Reply};

/// The version of the plugin interface this editor has.
pub const API: u64 = 1;
/// What a plugin in a folder of its own says what it adds with.
pub const FILE: &str = "plugin.json";

/// What lives as long as the program, as what's built in does.
fn forever(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

fn strings(value: &Value) -> &'static [&'static str] {
    let list: Vec<&'static str> = value.as_array().map_or(vec![], |items| items.iter().filter_map(Value::as_str).map(forever).collect());
    Box::leak(list.into_boxed_slice())
}

/// Letters, digits, `-` and `_`: what a plugin and its commands can be called.
pub fn proper_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A file its manifest names, which has to be in its folder, however it's named.
fn inside(folder: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if !path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("`{relative}` isn't in its folder"));
    }
    let real = |path: &Path| path.canonicalize().map_err(|_| format!("it has no `{relative}`"));
    let found = real(&folder.join(path))?;
    if !found.starts_with(real(folder)?) {
        return Err(format!("`{relative}` isn't in its folder"));
    }
    Ok(found)
}

/// How long code is given, and how much memory, unless it asks for more.
pub const SECONDS: u64 = 5;
pub const MEMORY_MB: u64 = 256;
/// The most it can ask for.
const MOST_SECONDS: u64 = 300;
const MOST_MEMORY_MB: u64 = 4096;

/// What a plugin's code may have, beyond what it's handed. Nothing, to begin with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// The project, read-only, when it makes pages of a file.
    pub project: bool,
    pub fonts: bool,
    pub cache: bool,
    pub read: Vec<String>,
    pub write: Vec<String>,
    pub network: Vec<String>,
    pub memory_mb: u64,
    pub seconds: u64,
}

impl Default for Access {
    fn default() -> Self {
        Self { project: false, fonts: false, cache: false, read: vec![], write: vec![], network: vec![], memory_mb: MEMORY_MB, seconds: SECONDS }
    }
}

impl Access {
    fn parse(json: &Value) -> Result<Self, String> {
        let on = |name: &str| json[name].as_bool().unwrap_or(false);
        let list =
            |name: &str| -> Vec<String> { json[name].as_array().map_or(vec![], |items| items.iter().filter_map(Value::as_str).map(str::to_string).collect()) };
        let (read, write, network) = (list("read"), list("write"), list("network"));
        for folder in read.iter().chain(&write) {
            let rest = folder
                .strip_prefix("{data}/")
                .or_else(|| folder.strip_prefix("{cache}/"))
                .ok_or(format!("`{folder}`: it can only have folders under {{data}} or {{cache}}"))?;
            if rest.is_empty() || !Path::new(rest).components().all(|part| matches!(part, Component::Normal(_))) {
                return Err(format!("`{folder}` isn't a folder under those"));
            }
        }
        if let Some(host) = network.iter().find(|host| host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
            return Err(format!("`{host}` isn't a host"));
        }
        let limit = |name: &str, default: u64, most: u64| match json[name].as_u64() {
            Some(asked) if asked > most => Err(format!("it asks for {asked} {name}, and the most is {most}")),
            Some(asked) => Ok(asked.max(1)),
            None => Ok(default),
        };
        Ok(Self {
            project: on("project"),
            fonts: on("fonts"),
            cache: on("cache"),
            read,
            write,
            network,
            memory_mb: limit("memory_mb", MEMORY_MB, MOST_MEMORY_MB)?,
            seconds: limit("seconds", SECONDS, MOST_SECONDS)?,
        })
    }

    /// What it amounts to, for whoever's installing it; nothing if it's what all code gets.
    fn described(&self) -> Vec<String> {
        let mut parts = vec![];
        if self.project {
            parts.push("reads the project".to_string());
        }
        if self.fonts {
            parts.push("reads the system's fonts".to_string());
        }
        if !self.read.is_empty() {
            parts.push(format!("reads {}", self.read.join(", ")));
        }
        if !self.write.is_empty() {
            parts.push(format!("writes {}", self.write.join(", ")));
        }
        if !self.network.is_empty() {
            parts.push(format!("downloads from {}", self.network.join(", ")));
        }
        if self.memory_mb != MEMORY_MB || self.seconds != SECONDS {
            parts.push(format!("may use {} MB and {} s at a time", self.memory_mb, self.seconds));
        }
        parts
    }
}

/// What highlights by asking a plugin's code.
fn by_its_code(code: Arc<dyn Code>) -> &'static ghostwrite_core::language::Highlighter {
    Box::leak(Box::new(move |text: &str| -> Vec<ghostwrite_core::syntax::Span> {
        let spans = code.highlight(text).into_iter();
        spans.filter_map(|(range, scope)| Some(ghostwrite_core::syntax::Span { range, scope: ghostwrite_core::syntax::scope_named(&scope)? })).collect()
    }))
}

/// A grammar a manifest names.
struct Grammar {
    name: String,
    path: PathBuf,
    query: String,
}

pub struct Manifest {
    pub name: &'static str,
    pub version: Option<&'static str>,
    language: Option<&'static Language>,
    server: Option<&'static Spec>,
    keys: &'static [Binding],
    rules: &'static str,
    commands: &'static [PluginCommand],
    events: &'static [Event],
    code: Option<Arc<dyn Code>>,
    grammar: Option<Grammar>,
    pages: bool,
    access: Access,
}

impl Manifest {
    /// The plugin that `text` says, on its own in a file, or what's wrong with it.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::parse_in(text, None)
    }

    /// The plugin that `text` says, with its files in `folder` if it has one.
    pub fn parse_in(text: &str, folder: Option<&Path>) -> Result<Self, String> {
        let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let name = json["name"].as_str().filter(|name| !name.is_empty()).ok_or("it has no name")?;
        if !proper_name(name) {
            return Err(format!("`{name}` isn't a name: letters, digits, - and _"));
        }
        let name = forever(name);
        let runs_code = !json["code"].is_null() || !json["grammar"].is_null();
        match json["api"].as_u64() {
            None if runs_code => return Err("it doesn't say which `api` it was built for".into()),
            Some(api) if api != API => return Err(format!("it was built for version {api} of the plugin interface; this editor has version {API}")),
            _ => {}
        }
        let needs_folder = |what: &str| folder.ok_or(format!("a plugin with {what} needs a folder of its own, with {FILE} in it"));
        let access = Access::parse(&json["access"])?;
        let code = match json["code"].as_str() {
            Some(file) => {
                let path = inside(needs_folder("code")?, file)?;
                let runtime = crate::runtime().ok_or("this build of the editor can't run plugins with code")?;
                Some(Arc::<dyn Code>::from(runtime.code(name, &path, &access)))
            }
            None => None,
        };
        let pages = json["pages"].as_bool().unwrap_or(false);
        if pages && code.is_none() {
            return Err("its pages have no `code` to make them".into());
        }
        let mut commands = vec![];
        for command in json["commands"].as_array().map_or(&[][..], Vec::as_slice) {
            let called = command["name"].as_str().ok_or("a command has no `name`")?;
            if !proper_name(called) {
                return Err(format!("`{called}` isn't a command's name: letters, digits, - and _"));
            }
            commands.push(PluginCommand { name: forever(called), what: forever(command["what"].as_str().unwrap_or("")) });
        }
        let mut events = vec![];
        for event in json["events"].as_array().map_or(&[][..], Vec::as_slice) {
            events.push(match event.as_str() {
                Some("opened") => Event::Opened,
                Some("saved") => Event::Saved,
                Some("closed") => Event::Closed,
                _ => return Err(format!("it asks to hear of {event}: there's opened, saved and closed")),
            });
        }
        if code.is_none() && !(commands.is_empty() && events.is_empty()) {
            return Err("its commands and events have no `code` to run".into());
        }
        let grammar = match &json["grammar"] {
            Value::Null => None,
            grammar => {
                let field = |field: &str| grammar[field].as_str().ok_or(format!("its grammar has no `{field}`"));
                let folder = needs_folder("a grammar")?;
                let query = std::fs::read_to_string(inside(folder, field("query")?)?).map_err(|e| format!("its query can't be read: {e}"))?;
                Some(Grammar { name: field("name")?.to_string(), path: inside(folder, field("wasm")?)?, query })
            }
        };
        let colors_itself = json["highlights"].as_bool().unwrap_or(false);
        let highlighter = match (&grammar, &code) {
            (Some(_), _) if colors_itself => return Err("it has a grammar and also highlights by itself: one or the other".into()),
            (Some(grammar), _) => {
                let runtime = crate::runtime().ok_or("this build of the editor can't run plugins' grammars")?;
                Some(runtime.grammar(&grammar.name, &grammar.path, &grammar.query))
            }
            (None, Some(code)) if colors_itself => Some(by_its_code(code.clone())),
            (None, None) if colors_itself => return Err("it says it highlights, with no `code` to do it".into()),
            (None, _) => None,
        };
        let command = strings(&json["server"]);
        let server: Option<&'static Spec> =
            (!command.is_empty()).then(|| &*Box::leak(Box::new(Spec { name: forever(&format!("{name}-server")), command, ..Spec::DEFAULT })));
        let (extensions, file_names) = (strings(&json["extensions"]), strings(&json["files"]));
        if highlighter.is_some() && extensions.is_empty() && file_names.is_empty() {
            return Err("it has a grammar, but says of no files that they're in it".into());
        }
        let only_pages = json["only_pages"].as_bool().unwrap_or(false);
        if only_pages && (!pages || (extensions.is_empty() && file_names.is_empty())) {
            return Err("only its pages show, but it doesn't say it makes them, or of which files".into());
        }
        let language: Option<&'static Language> = (!extensions.is_empty() || !file_names.is_empty()).then(|| {
            let servers: &'static [&'static str] = Box::leak(server.iter().map(|spec| spec.name).collect::<Vec<_>>().into_boxed_slice());
            &*Box::leak(Box::new(Language {
                name,
                id: name,
                extensions,
                file_names,
                servers,
                highlighter,
                only_pages,
                pages_at_own_size: json["pages_at_own_size"].as_bool().unwrap_or(false),
                comment: json["comment"].as_str().map(forever),
                ..Language::DEFAULT
            }))
        });
        let mut keys = vec![];
        for binding in json["keys"].as_array().map_or(&[][..], Vec::as_slice) {
            let field = |name: &str| binding[name].as_str().ok_or(format!("a key has no `{name}`"));
            keys.push(Binding {
                keys: forever(field("keys")?),
                command: forever(field("command")?.trim_start_matches(':')),
                what: forever(binding["what"].as_str().unwrap_or("")),
            });
        }
        let rules = forever(json["rules"].as_str().unwrap_or(""));
        Ok(Self {
            name,
            version: json["version"].as_str().map(forever),
            language,
            server,
            keys: Box::leak(keys.into_boxed_slice()),
            rules,
            commands: Box::leak(commands.into_boxed_slice()),
            events: Box::leak(events.into_boxed_slice()),
            code,
            grammar,
            pages,
            access,
        })
    }

    /// The plugin in `folder`.
    pub fn in_own_folder(folder: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(folder.join(FILE)).map_err(|_| format!("there's no {FILE} in it"))?;
        Self::parse_in(&text, Some(folder))
    }

    /// Whether its code and grammar can be run, or what's wrong with them: loading
    /// them, which is otherwise left until they're needed.
    pub fn check(&self) -> Result<(), String> {
        if let Some(code) = &self.code {
            code.check().map_err(|e| format!("its code can't be run: {e}"))?;
        }
        if let Some(grammar) = &self.grammar {
            let runtime = crate::runtime().ok_or("this build of the editor can't run plugins' grammars")?;
            runtime.check_grammar(&grammar.name, &grammar.path, &grammar.query).map_err(|e| format!("its grammar can't be used: {e}"))?;
        }
        Ok(())
    }

    /// The rules it adds for the model, if any.
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    /// The plugins in the folder, by file or folder of their own, and what's wrong
    /// with those that can't be read.
    pub fn in_folder(folder: &Path) -> (Vec<Self>, Vec<String>) {
        let (mut found, mut wrong) = (vec![], vec![]);
        let Ok(files) = std::fs::read_dir(folder) else { return (found, wrong) };
        let mut paths: Vec<_> = files.flatten().map(|file| file.path()).collect();
        paths.sort();
        for path in paths {
            let label = path.file_name().map_or_else(Default::default, |n| n.to_string_lossy().into_owned());
            let read = if label.starts_with('.') {
                continue;
            } else if path.is_dir() {
                Self::in_own_folder(&path)
            } else if path.extension().is_some_and(|e| e == "json") {
                std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| Self::parse(&text))
            } else {
                continue;
            };
            match read {
                Ok(manifest) => found.push(manifest),
                Err(why) => wrong.push(format!("{label}: {why}")),
            }
        }
        (found, wrong)
    }
}

impl Plugin for Manifest {
    fn name(&self) -> &str {
        self.name
    }

    /// Everything it can do, as it's told to you when it's installed: what it can
    /// reach beyond the editor is its language server, which runs as you do.
    fn describe(&self) -> String {
        let mut parts: Vec<String> = vec![];
        parts.extend(self.version.map(|version| format!("version {version}")));
        if !self.commands.is_empty() {
            parts.push(self.commands.iter().map(|command| format!(":{}", command.name)).collect::<Vec<_>>().join(" "));
        }
        if !self.events.is_empty() {
            let names = self.events.iter().map(|event| format!("{event:?}").to_lowercase()).collect::<Vec<_>>().join(", ");
            parts.push(format!("runs when a file is {names}"));
        }
        if let Some(language) = self.language {
            let colors = if language.highlighter.is_some() { ", highlighted" } else { "" };
            parts.push(format!(
                "{} files{colors}",
                language.extensions.iter().map(|e| format!(".{e}")).chain(language.file_names.iter().map(|f| f.to_string())).collect::<Vec<_>>().join(" ")
            ));
        }
        if let Some(server) = self.server {
            parts.push(format!("starts `{}` with your permissions", server.command.join(" ")));
        }
        parts.extend(self.access.described());
        if !self.keys.is_empty() {
            parts.push(self.keys.iter().map(|key| key.keys).collect::<Vec<_>>().join(", "));
        }
        if self.has_rules() {
            parts.push("adds rules to the model's prompt".to_string());
        }
        parts.join("; ")
    }

    fn commands(&self) -> &'static [PluginCommand] {
        self.commands
    }

    fn run(&self, name: &str, args: &[String], context: &Context) -> Reply {
        match &self.code {
            Some(code) => code.run(name, args, context),
            None => Err(format!("there's no command {name} here")),
        }
    }

    fn events(&self) -> &'static [Event] {
        self.events
    }

    fn on_event(&self, event: Event, context: &Context) -> Reply {
        match &self.code {
            Some(code) => code.on_event(event, context),
            None => Ok(vec![]),
        }
    }

    fn renders(&self, language: &Language) -> bool {
        self.pages && self.language.is_some_and(|own| std::ptr::eq(own, language) || own.name == language.name)
    }

    fn renderer(&self, root: &Path, path: &Path) -> Result<Box<dyn Renderer>, String> {
        match &self.code {
            Some(code) if self.pages => code.renderer(root, path),
            _ => Err("this plugin doesn't make pages".into()),
        }
    }

    /// With a language, for its files; without one, for every file.
    fn keys(&self, language: Option<&Language>) -> &'static [Binding] {
        let ours = self.language.is_none_or(|own| language.is_some_and(|language| language.name == own.name));
        if ours { self.keys } else { &[] }
    }

    fn languages(&self) -> Vec<&'static Language> {
        self.language.into_iter().collect()
    }

    fn servers(&self) -> Vec<&'static Spec> {
        self.server.into_iter().collect()
    }

    fn prompt_rules(&self) -> &'static str {
        self.rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZIG: &str = r#"{ "name": "zig-test", "extensions": ["zigtest"], "comment": "//", "server": ["zls", "--stdio"],
        "keys": [{ "keys": "space z", "command": ":term", "what": "the terminal" }], "rules": "- Keep it small." }"#;

    #[test]
    fn a_file_says_what_a_plugin_adds() {
        let zig = Manifest::parse(ZIG).unwrap();
        let language = zig.languages()[0];
        assert_eq!((language.name, language.extensions, language.comment), ("zig-test", &["zigtest"][..], Some("//")));
        let server = zig.servers()[0];
        assert_eq!(server.command, ["zls", "--stdio"]);
        assert_eq!(language.servers, [server.name], "and the language knows its server");
        assert_eq!(zig.prompt_rules(), "- Keep it small.");
        assert_eq!(zig.keys(Some(language))[0].command, "term", "a command, without its colon");
        assert!(zig.keys(None).is_empty() && zig.keys(Some(&Language::DEFAULT)).is_empty(), "its keys are for its files");
        Language::register(language);
        assert_eq!(Language::for_path(Path::new("a.zigtest")).map(|l| l.name), Some("zig-test"), "and its files are found");
        assert!(zig.describe().contains("starts `zls --stdio` with your permissions"), "{}", zig.describe());
    }

    #[test]
    fn what_code_may_have_is_nothing_unless_it_asks_and_then_its_said() {
        assert_eq!(Access::parse(&Value::Null), Ok(Access::default()));
        assert!(Access::default().described().is_empty());
        let access = Access::parse(
            &serde_json::json!({ "project": true, "fonts": true, "read": ["{data}/t"], "write": ["{cache}/t"], "network": ["x.org"], "memory_mb": 512 }),
        )
        .unwrap();
        let said = access.described().join("; ");
        for part in ["reads the project", "reads the system's fonts", "reads {data}/t", "writes {cache}/t", "downloads from x.org", "512 MB"] {
            assert!(said.contains(part), "{said}");
        }
    }

    #[test]
    fn keys_without_a_language_are_for_every_file() {
        let plugin = Manifest::parse(r#"{ "name": "mine", "keys": [{ "keys": "space z", "command": "term" }] }"#).unwrap();
        assert_eq!(plugin.keys(None).len(), 1);
        assert_eq!(plugin.keys(Some(&Language::DEFAULT)).len(), 1);
    }

    #[test]
    fn whats_wrong_with_a_file_is_said_and_the_others_still_load() {
        assert!(Manifest::parse("{").is_err());
        assert_eq!(Manifest::parse(r#"{ "extensions": ["x"] }"#).err().as_deref(), Some("it has no name"));
        assert_eq!(Manifest::parse(r#"{ "name": "x", "keys": [{ "keys": "z" }] }"#).err().as_deref(), Some("a key has no `command`"));
        let folder = ghostwrite_project::scratch_dir("manifests");
        std::fs::write(folder.join("good.json"), ZIG).unwrap();
        std::fs::write(folder.join("bad.json"), "{").unwrap();
        std::fs::write(folder.join("notes.txt"), "not a plugin").unwrap();
        std::fs::create_dir(folder.join(".incoming")).unwrap();
        let (found, wrong) = Manifest::in_folder(&folder);
        assert_eq!(found.iter().map(|m| m.name).collect::<Vec<_>>(), ["zig-test"]);
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with("bad.json: "), "{wrong:?}");
    }

    #[test]
    fn a_plugin_is_held_to_its_folder_and_its_version() {
        let parent = ghostwrite_project::scratch_dir("manifest-paths");
        let folder = parent.join("mine");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(parent.join("secret"), "out of reach").unwrap();
        let parse = |json: &str| Manifest::parse_in(json, Some(&folder)).err().unwrap_or_default();
        assert!(parse(r#"{ "name": "a", "api": 1, "code": "../secret" }"#).contains("isn't in its folder"));
        assert!(parse(r#"{ "name": "a", "api": 1, "code": "/etc/hosts" }"#).contains("isn't in its folder"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(parent.join("secret"), folder.join("link")).unwrap();
            assert!(parse(r#"{ "name": "a", "api": 1, "code": "link" }"#).contains("isn't in its folder"), "nor by way of a link");
        }
        assert!(parse(r#"{ "name": "a", "code": "x.wasm" }"#).contains("doesn't say which `api`"));
        assert!(parse(r#"{ "name": "a", "api": 2, "code": "x.wasm" }"#).contains("built for version 2"));
        assert!(parse(r#"{ "name": "../a" }"#).contains("isn't a name"));
        for bad in [
            r#""read": ["/etc"]"#,
            r#""write": ["{data}/../x"]"#,
            r#""read": ["{home}/x"]"#,
            r#""network": ["a b"]"#,
            r#""memory_mb": 99999"#,
            r#""seconds": 99999"#,
        ] {
            assert!(!parse(&format!(r#"{{ "name": "a", "access": {{ {bad} }} }}"#)).is_empty(), "{bad}");
        }
        assert!(parse(r#"{ "name": "a", "commands": [{ "name": "go" }] }"#).contains("no `code`"));
        let no_folder = Manifest::parse(r#"{ "name": "a", "api": 1, "code": "x.wasm" }"#).err().unwrap_or_default();
        assert!(no_folder.contains("a folder of its own"), "{no_folder}");
    }
}
