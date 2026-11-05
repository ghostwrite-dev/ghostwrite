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
//!   "pairs": ["||"],
//!   "unpaired": ["'"],
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
//!   "api": 2,
//!   "code": "words.wasm",
//!   "commands": [{ "name": "word-count", "what": "count the file's words" }],
//!   "events": ["saved"],
//!   "extensions": ["wd"],
//!   "grammar": { "name": "words", "wasm": "words-grammar.wasm", "query": "highlights.scm" }
//! }
//! ```
//!
//! `pairs` close themselves as they're typed, as brackets and quotes do, and
//! `unpaired` are quotes that don't. `files` are whole file names in the language
//! (`Makefile`), `id` what language servers call it if that isn't its name, and
//! `indent_after` the chars a line ends with when the next one goes in a level
//! (brackets, unless it says: `"{[(:"`).
//!
//! A plugin can have several languages, each said as the plugin says its own, with a
//! grammar each (one can serve several) and its queries: what's colored (`query`, a
//! file or several, read in order, the last winning), what other languages are inside
//! it (`injections`), and its text objects, which `maf` and `]f` select
//! (`textobjects`). Its languages are served by its servers, unless one names others
//! (`"servers": ["other-ls"]`):
//!
//! ```json
//! {
//!   "name": "ocaml",
//!   "api": 2,
//!   "languages": [
//!     { "name": "ocaml", "extensions": ["ml"],
//!       "grammar": { "name": "ocaml", "wasm": "tree-sitter-ocaml.wasm",
//!                    "query": "highlights.scm", "textobjects": "textobjects.scm" } },
//!     { "name": "ocaml-interface", "id": "ocaml.interface", "extensions": ["mli"],
//!       "grammar": { "name": "ocaml_interface", "wasm": "tree-sitter-ocaml_interface.wasm",
//!                    "query": ["highlights.scm", "interface.scm"] } }
//!   ],
//!   "servers": [{ "name": "ocamllsp", "command": ["ocamllsp"], "roots": ["dune-project"] }],
//!   "grammars": [{ "name": "regex", "wasm": "tree-sitter-regex.wasm", "query": "regex.scm" }]
//! }
//! ```
//!
//! `grammars` aren't a language's own: they're there to be injected, by name. A server
//! is started in the folder nearest the file with one of its `roots` in it (a build
//! tool's file, say), else the project's; with `"configured_by_code": true` the plugin's
//! code says what it starts with besides the login shell's environment, and its settings.
//!
//! A language can be typed with abbreviations (`"abbreviations": "symbols.tsv"`: lines of
//! an abbreviation, a tab and its symbol, so that `\alpha` becomes `α` as it's typed), and
//! the kinds of semantic token a plugin's servers name can show as scopes
//! (`"tokens": { "proofHole": "constant.builtin" }`).
//!
//! With code, a plugin can have a panel beside files of its languages, kept up to date
//! from their server. The manifest says what to ask the server about the cursor, again as
//! it moves or the text changes, and which of its notifications the panel hears of; the
//! code says what to show of the answers (`panel-open`, `panel-answered` and
//! `panel-notification`), and what it keeps about each file comes back to it each time:
//!
//! ```json
//! {
//!   "panel": { "title": "Proof", "requests": ["$/proof/goal"], "notifications": ["$/proof/progress"] }
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
//!     "env": ["PATH"],
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
//! and cache folders, mounted at `/data/…` and `/user-cache/…`. `env` are the
//! environment variables code is shown when it's asked about a server of its own, as
//! that server would have them; with `PATH` among them it can find a program on it.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ghostwrite_core::Language;
use ghostwrite_core::abbreviation::Abbreviations;
use ghostwrite_core::syntax::Grammar;
use ghostwrite_lsp::registry::{Env, Settings, Spec};
use serde_json::Value;

use crate::{Binding, Block, Code, Context, Event, Panel, Plugin, PluginCommand, Renderer, Reply, Shown};

/// The version of the plugin interface this editor has.
pub const API: u64 = 2;
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
    /// Environment variables it's shown when it's asked about a server of its own.
    pub env: Vec<String>,
    pub memory_mb: u64,
    pub seconds: u64,
}

impl Default for Access {
    fn default() -> Self {
        Self { project: false, fonts: false, cache: false, read: vec![], write: vec![], network: vec![], env: vec![], memory_mb: MEMORY_MB, seconds: SECONDS }
    }
}

impl Access {
    fn parse(json: &Value) -> Result<Self, String> {
        let on = |name: &str| json[name].as_bool().unwrap_or(false);
        let list =
            |name: &str| -> Vec<String> { json[name].as_array().map_or(vec![], |items| items.iter().filter_map(Value::as_str).map(str::to_string).collect()) };
        let (read, write, network, env) = (list("read"), list("write"), list("network"), list("env"));
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
        if let Some(name) = env.iter().find(|name| name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return Err(format!("`{name}` isn't an environment variable"));
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
            env,
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
        if !self.env.is_empty() {
            let names: Vec<String> = self.env.iter().map(|name| format!("${name}")).collect();
            parts.push(format!("reads {} as its servers have them", names.join(", ")));
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

/// A file its manifest names, in its folder, read.
fn read(folder: &Path, file: &str, what: &str) -> Result<String, String> {
    std::fs::read_to_string(inside(folder, file)?).map_err(|e| format!("its {what} `{file}` can't be read: {e}"))
}

/// The grammar `json` says, with its queries read from `folder`.
fn grammar(json: &Value, folder: Option<&Path>) -> Result<&'static Grammar, String> {
    let field = |field: &str| json[field].as_str().ok_or(format!("its grammar has no `{field}`"));
    let folder = folder.ok_or(format!("a plugin with a grammar needs a folder of its own, with {FILE} in it"))?;
    let name = field("name")?;
    // A query is a file, or several read one after another: where two say how to color
    // the same thing, the one read last wins.
    let files: Vec<&str> = match &json["query"] {
        Value::Array(files) => files.iter().filter_map(Value::as_str).collect(),
        file => file.as_str().into_iter().collect(),
    };
    let highlights = files.iter().map(|file| read(folder, file, "query")).collect::<Result<Vec<_>, _>>()?.join("\n");
    let optional = |key: &str, what: &str| json[key].as_str().map_or(Ok(String::new()), |file| read(folder, file, what));
    let (injections, textobjects) = (optional("injections", "injection query")?, optional("textobjects", "text-object query")?);
    let wasm = inside(folder, field("wasm")?)?;
    let runtime = crate::runtime().ok_or("this build of the editor can't run plugins' grammars")?;
    let load = runtime.grammar(name, &wasm);
    Ok(Box::leak(Box::new(Grammar::new(name, load, &highlights, &injections, &textobjects))))
}

/// The language `json` says, called `name`, if it says of any files that they're in
/// it. `servers` serve it, unless it's one of a plugin's `languages` and names others.
fn language(
    json: &Value,
    name: &'static str,
    folder: Option<&Path>,
    code: Option<&Arc<dyn Code>>,
    servers: &'static [&'static str],
    listed: bool,
) -> Result<Option<&'static Language>, String> {
    let grammar = match &json["grammar"] {
        Value::Null => None,
        said => Some(grammar(said, folder)?),
    };
    let colors_itself = json["highlights"].as_bool().unwrap_or(false);
    let highlighter = match (grammar, code) {
        (Some(_), _) if colors_itself => return Err("it has a grammar and also highlights by itself: one or the other".into()),
        (None, Some(code)) if colors_itself => Some(by_its_code(code.clone())),
        (None, None) if colors_itself => return Err("it says it highlights, with no `code` to do it".into()),
        _ => None,
    };
    let (extensions, file_names) = (strings(&json["extensions"]), strings(&json["files"]));
    if extensions.is_empty() && file_names.is_empty() {
        if grammar.is_some() || highlighter.is_some() {
            return Err("it has a grammar, but says of no files that they're in it".into());
        }
        return Ok(None);
    }
    // `"pairs": ["$$"]`: an opening and closing char each; `"unpaired": ["'"]`.
    let mut pairs = vec![];
    for pair in strings(&json["pairs"]) {
        let mut chars = pair.chars();
        let (Some(open), Some(close), None) = (chars.next(), chars.next(), chars.next()) else {
            return Err(format!("the pair `{pair}` isn't two chars, opening and closing"));
        };
        pairs.push((open, close));
    }
    let unpaired: Vec<char> = strings(&json["unpaired"]).iter().flat_map(|quote| quote.chars()).collect();
    let indent_after: &'static [char] = match json["indent_after"].as_str() {
        Some(chars) => Box::leak(chars.chars().collect::<Vec<_>>().into_boxed_slice()),
        None => Language::DEFAULT.indent_after,
    };
    let servers = if listed && json["servers"].is_array() { strings(&json["servers"]) } else { servers };
    // `abbreviation⇥symbol` lines.
    let abbreviations = match json["abbreviations"].as_str() {
        Some(file) => {
            let folder = folder.ok_or(format!("a plugin with abbreviations needs a folder of its own, with {FILE} in it"))?;
            let table = read(folder, file, "abbreviations")?;
            if !table.lines().any(|line| line.contains('\t')) {
                return Err(format!("its abbreviations `{file}` are none: they're lines of an abbreviation, a tab and its symbol"));
            }
            Some(&*Box::leak(Box::new(Abbreviations::new(forever(&table)))))
        }
        None => None,
    };
    Ok(Some(Box::leak(Box::new(Language {
        name,
        id: json["id"].as_str().map_or("", forever),
        extensions,
        file_names,
        servers,
        grammar,
        highlighter,
        only_pages: json["only_pages"].as_bool().unwrap_or(false),
        pages_at_own_size: json["pages_at_own_size"].as_bool().unwrap_or(false),
        comment: json["comment"].as_str().map(forever),
        pairs: Box::leak(pairs.into_boxed_slice()),
        unpaired: Box::leak(unpaired.into_boxed_slice()),
        indent_after,
        abbreviations,
    }))))
}

/// The servers a manifest says: `"server"`, a command, which is called after the
/// plugin; and `"servers"`, each with a name of its own.
fn servers(json: &Value, name: &str, code: Option<&Arc<dyn Code>>) -> Result<Vec<&'static Spec>, String> {
    let mut found = vec![];
    let command = strings(&json["server"]);
    if !command.is_empty() {
        found.push(&*Box::leak(Box::new(Spec { name: forever(&format!("{name}-server")), command, ..Spec::DEFAULT })));
    }
    for server in json["servers"].as_array().map_or(&[][..], Vec::as_slice) {
        let called = server["name"].as_str().ok_or("a server has no `name`")?;
        if !proper_name(called) {
            return Err(format!("`{called}` isn't a server's name: letters, digits, - and _"));
        }
        let called = forever(called);
        let command = strings(&server["command"]);
        if command.is_empty() {
            return Err(format!("the server {called} has no `command`"));
        }
        let (mut env, mut settings): (Option<&'static Env>, Option<&'static Settings>) = (None, None);
        if server["configured_by_code"].as_bool().unwrap_or(false) {
            let code = code.ok_or(format!("the server {called} is configured by `code` the plugin doesn't have"))?;
            let (asked, answers) = (code.clone(), code.clone());
            let starts_with = move |project: &Path, root: &Path, has: &[(String, String)]| {
                asked.server_env(called, project, root, has).inspect_err(|e| eprintln!("what {called} starts with: {e}")).unwrap_or_default()
            };
            let answered = move |project: &Path, root: &Path, has: &[(String, String)], section: &str| {
                answers.server_settings(called, project, root, has, section).inspect_err(|e| eprintln!("{called}'s settings: {e}")).unwrap_or(Value::Null)
            };
            env = Some(Box::leak(Box::new(starts_with)));
            settings = Some(Box::leak(Box::new(answered)));
        }
        found.push(&*Box::leak(Box::new(Spec { name: called, command, roots: strings(&server["roots"]), env, settings })));
    }
    Ok(found)
}

pub struct Manifest {
    pub name: &'static str,
    pub version: Option<&'static str>,
    languages: Vec<&'static Language>,
    /// Grammars that aren't a language's own, there to be injected.
    grammars: Vec<&'static Grammar>,
    servers: Vec<&'static Spec>,
    keys: &'static [Binding],
    rules: &'static str,
    commands: &'static [PluginCommand],
    events: &'static [Event],
    code: Option<Arc<dyn Code>>,
    pages: bool,
    panel: Option<PanelSaid>,
    /// Kinds of semantic token its servers name, and the scope each shows as.
    tokens: &'static [(&'static str, &'static str)],
    access: Access,
}

/// What a manifest says of its panel: its code says what's in it.
#[derive(Clone, Copy)]
struct PanelSaid {
    title: &'static str,
    requests: &'static [&'static str],
    notifications: &'static [&'static str],
}

/// A panel beside a file, its code asked what's in it. What it keeps is kept here.
struct CodePanel {
    said: PanelSaid,
    code: Arc<dyn Code>,
    language: &'static str,
    /// As its code last said, once it's been asked.
    shown: OnceLock<Shown>,
}

impl CodePanel {
    fn shown(&self) -> &Shown {
        self.shown.get_or_init(|| self.code.panel_open(self.language).unwrap_or_else(|why| self.failed(&Shown::default(), &why)))
    }

    /// What was there, saying why it isn't up to date.
    fn failed(&self, was: &Shown, why: &str) -> Shown {
        Shown { blocks: vec![Block::Muted(format!("{}: {why}", self.said.title))], ..was.clone() }
    }
}

impl Panel for CodePanel {
    fn title(&self) -> &'static str {
        self.said.title
    }

    fn requests(&self) -> &'static [&'static str] {
        self.said.requests
    }

    fn answered(&mut self, answers: Vec<Option<Value>>) {
        let was = self.shown().clone();
        let now = self.code.panel_answered(&was.state, &answers).unwrap_or_else(|why| self.failed(&was, &why));
        self.shown = OnceLock::from(now);
    }

    fn notification(&mut self, method: &str, params: &Value) -> bool {
        if !self.said.notifications.contains(&method) {
            return false;
        }
        let was = self.shown().clone();
        let (now, again) = self.code.panel_notification(&was.state, method, params).unwrap_or_else(|why| (self.failed(&was, &why), false));
        self.shown = OnceLock::from(now);
        again
    }

    fn working_on(&self) -> Option<u32> {
        self.shown().working_on
    }

    fn blocks(&self) -> Vec<Block> {
        self.shown().blocks.clone()
    }
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
        let has_grammar = |json: &Value| !json["grammar"].is_null();
        let languages_said = json["languages"].as_array().map_or(&[][..], Vec::as_slice);
        let runs_code = !json["code"].is_null()
            || has_grammar(&json)
            || languages_said.iter().any(has_grammar)
            || json["grammars"].as_array().is_some_and(|grammars| !grammars.is_empty());
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
        let servers = servers(&json, name, code.as_ref())?;
        let serving: &'static [&'static str] = Box::leak(servers.iter().map(|spec| spec.name).collect::<Vec<_>>().into_boxed_slice());
        // Its own language, said as the plugin's, and the others it lists.
        let mut languages: Vec<&'static Language> = language(&json, name, folder, code.as_ref(), serving, false)?.into_iter().collect();
        for said in languages_said {
            let called = said["name"].as_str().ok_or("a language has no `name`")?;
            if !proper_name(called) {
                return Err(format!("`{called}` isn't a language's name: letters, digits, - and _"));
            }
            if said["highlights"].as_bool().unwrap_or(false) || said["only_pages"].as_bool().unwrap_or(false) {
                return Err(format!("the language {called}: only the plugin's own language can be highlighted by its code, or be pages"));
            }
            let found = language(said, forever(called), folder, None, serving, true).map_err(|e| format!("the language {called}: {e}"))?;
            languages.push(found.ok_or(format!("the language {called} says of no files that they're in it"))?);
        }
        let only_pages = languages.first().is_some_and(|own| own.only_pages && own.name == name);
        if json["only_pages"].as_bool().unwrap_or(false) && !(pages && only_pages) {
            return Err("only its pages show, but it doesn't say it makes them, or of which files".into());
        }
        let grammars = json["grammars"].as_array().map_or(&[][..], Vec::as_slice).iter().map(|said| grammar(said, folder)).collect::<Result<_, _>>()?;
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
        let panel = match &json["panel"] {
            Value::Null => None,
            said => {
                if code.is_none() {
                    return Err("its panel has no `code` to say what's in it".into());
                }
                if languages.is_empty() {
                    return Err("its panel is beside files of its languages, and it has none".into());
                }
                let title = said["title"].as_str().map_or(name, forever);
                Some(PanelSaid { title, requests: strings(&said["requests"]), notifications: strings(&said["notifications"]) })
            }
        };
        let mut tokens = vec![];
        for (token, scope) in json["tokens"].as_object().into_iter().flatten() {
            let scope = scope.as_str().ok_or(format!("the token {token} shows as no scope"))?;
            if ghostwrite_core::syntax::scope_named(scope).is_none() {
                return Err(format!("the token {token} shows as `{scope}`, which isn't a scope"));
            }
            tokens.push((forever(token), forever(scope)));
        }
        Ok(Self {
            name,
            version: json["version"].as_str().map(forever),
            languages,
            grammars,
            servers,
            keys: Box::leak(keys.into_boxed_slice()),
            rules,
            commands: Box::leak(commands.into_boxed_slice()),
            events: Box::leak(events.into_boxed_slice()),
            code,
            pages,
            panel,
            tokens: Box::leak(tokens.into_boxed_slice()),
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
        let own = self.languages.iter().filter_map(|language| language.grammar);
        for grammar in own.chain(self.grammars.iter().copied()) {
            grammar.check().map_err(|e| format!("its grammar {} can't be used: {e}", grammar.name))?;
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
        for language in &self.languages {
            let colors = if language.highlighter.is_some() || language.grammar.is_some() { ", highlighted" } else { "" };
            let typed = if language.abbreviations.is_some() { ", typed with abbreviations" } else { "" };
            parts.push(format!(
                "{} files{colors}{typed}",
                language.extensions.iter().map(|e| format!(".{e}")).chain(language.file_names.iter().map(|f| f.to_string())).collect::<Vec<_>>().join(" ")
            ));
        }
        for server in &self.servers {
            parts.push(format!("starts `{}` with your permissions", server.command.join(" ")));
        }
        if let Some(panel) = &self.panel {
            parts.push(format!("shows {} beside its files", panel.title));
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
        self.pages && self.languages.iter().any(|own| std::ptr::eq(*own, language) || own.name == language.name)
    }

    fn renderer(&self, root: &Path, path: &Path) -> Result<Box<dyn Renderer>, String> {
        match &self.code {
            Some(code) if self.pages => code.renderer(root, path),
            _ => Err("this plugin doesn't make pages".into()),
        }
    }

    /// With languages, for their files; without, for every file.
    fn keys(&self, language: Option<&Language>) -> &'static [Binding] {
        let ours = self.languages.is_empty() || language.is_some_and(|language| self.languages.iter().any(|own| own.name == language.name));
        if ours { self.keys } else { &[] }
    }

    fn languages(&self) -> Vec<&'static Language> {
        self.languages.clone()
    }

    fn grammars(&self) -> Vec<&'static Grammar> {
        self.grammars.clone()
    }

    fn servers(&self) -> Vec<&'static Spec> {
        self.servers.clone()
    }

    fn tokens(&self) -> &'static [(&'static str, &'static str)] {
        self.tokens
    }

    fn prompt_rules(&self) -> &'static str {
        self.rules
    }

    /// For files of its languages, if it says it has one.
    fn panel(&self, language: &Language) -> Option<Box<dyn Panel>> {
        let (said, code) = (self.panel?, self.code.clone()?);
        let own = self.languages.iter().find(|own| own.name == language.name)?;
        Some(Box::new(CodePanel { said, code, language: own.name, shown: OnceLock::new() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZIG: &str = r#"{ "name": "zig-test", "extensions": ["zigtest"], "comment": "//", "pairs": ["||"], "unpaired": ["'"], "server": ["zls", "--stdio"],
        "keys": [{ "keys": "space z", "command": ":term", "what": "the terminal" }], "rules": "- Keep it small." }"#;

    #[test]
    fn a_file_says_what_a_plugin_adds() {
        let zig = Manifest::parse(ZIG).unwrap();
        let language = zig.languages()[0];
        assert_eq!((language.name, language.extensions, language.comment), ("zig-test", &["zigtest"][..], Some("//")));
        assert_eq!((language.pairs, language.unpaired), (&[('|', '|')][..], &['\''][..]));
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
    fn a_plugin_can_have_several_languages_and_servers() {
        let json = r#"{ "name": "family", "id": "family-own", "extensions": ["fam"], "indent_after": "{:",
            "server": ["old-style"],
            "servers": [{ "name": "family-ls", "command": ["family-ls", "--stdio"], "roots": ["family.toml"] }],
            "languages": [
              { "name": "kin", "id": "kinfolk", "extensions": ["kin"], "files": ["Kinfile"], "comment": "--", "unpaired": ["`"] },
              { "name": "cousin", "extensions": ["cou"], "servers": ["elsewhere-ls"] } ],
            "keys": [{ "keys": "space z", "command": "term" }] }"#;
        let plugin = Manifest::parse(json).unwrap();
        let [own, kin, cousin] = plugin.languages()[..] else { panic!("three languages") };
        assert_eq!((own.name, own.id(), own.indent_after), ("family", "family-own", &['{', ':'][..]));
        assert_eq!((kin.id(), kin.file_names, kin.comment, kin.unpaired), ("kinfolk", &["Kinfile"][..], Some("--"), &['`'][..]));
        assert_eq!((cousin.id(), cousin.indent_after), ("cousin", Language::DEFAULT.indent_after), "its name, and brackets");
        let servers = plugin.servers();
        assert_eq!(servers.iter().map(|server| server.name).collect::<Vec<_>>(), ["family-server", "family-ls"]);
        assert_eq!((servers[1].command, servers[1].roots), (&["family-ls", "--stdio"][..], &["family.toml"][..]));
        assert!(servers[1].env.is_none() && servers[1].settings.is_none());
        assert_eq!((own.servers, kin.servers), (&["family-server", "family-ls"][..], &["family-server", "family-ls"][..]), "the plugin's servers");
        assert_eq!(cousin.servers, ["elsewhere-ls"], "unless it names others");
        assert!(!plugin.keys(Some(cousin)).is_empty() && plugin.keys(Some(&Language::DEFAULT)).is_empty(), "its keys are for all its languages");
        let described = plugin.describe();
        assert!(described.contains(".kin Kinfile files") && described.contains("starts `family-ls --stdio`"), "{described}");
    }

    #[test]
    fn whats_wrong_with_a_language_or_server_is_said() {
        let wrong = |json: &str| Manifest::parse(json).err().unwrap_or_default();
        assert!(wrong(r#"{ "name": "a", "languages": [{ "name": "b" }] }"#).contains("says of no files"));
        assert!(wrong(r#"{ "name": "a", "languages": [{ "extensions": ["b"] }] }"#).contains("a language has no `name`"));
        assert!(wrong(r#"{ "name": "a", "languages": [{ "name": "b", "extensions": ["b"], "highlights": true }] }"#).contains("only the plugin's own"));
        assert!(wrong(r#"{ "name": "a", "servers": [{ "name": "b" }] }"#).contains("has no `command`"));
        assert!(wrong(r#"{ "name": "a", "servers": [{ "command": ["b"] }] }"#).contains("a server has no `name`"));
        assert!(wrong(r#"{ "name": "a", "servers": [{ "name": "b", "command": ["b"], "configured_by_code": true }] }"#).contains("doesn't have"));
        assert!(wrong(r#"{ "name": "a", "access": { "env": ["NOT A NAME"] } }"#).contains("isn't an environment variable"));
        assert!(wrong(r#"{ "name": "a", "languages": [{ "name": "b", "extensions": ["b"], "grammar": {} }] }"#).contains("doesn't say which `api`"));
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
    fn a_language_can_be_typed_with_abbreviations_and_its_servers_tokens_colored() {
        let folder = ghostwrite_project::scratch_dir("abbreviations");
        std::fs::write(folder.join("symbols.tsv"), "alpha\tα\nN\tℕ\n").unwrap();
        let json = r#"{ "name": "symbolic", "extensions": ["sym"], "abbreviations": "symbols.tsv",
            "tokens": { "proofHole": "constant.builtin" } }"#;
        let plugin = Manifest::parse_in(json, Some(&folder)).unwrap();
        let table = plugin.languages()[0].abbreviations.expect("its abbreviations");
        assert_eq!((table.eager("alpha"), table.finished("N")), (Some("α"), Some("ℕ")));
        assert_eq!(plugin.tokens(), [("proofHole", "constant.builtin")]);
        assert!(plugin.describe().contains(".sym files, typed with abbreviations"), "{}", plugin.describe());

        let wrong = |json: &str| Manifest::parse_in(json, Some(&folder)).err().unwrap_or_default();
        assert!(wrong(r#"{ "name": "a", "extensions": ["a"], "abbreviations": "missing.tsv" }"#).contains("no `missing.tsv`"));
        std::fs::write(folder.join("empty.tsv"), "no tabs here\n").unwrap();
        assert!(wrong(r#"{ "name": "a", "extensions": ["a"], "abbreviations": "empty.tsv" }"#).contains("are none"));
        assert!(wrong(r#"{ "name": "a", "tokens": { "x": "not.a.scope.at.all" } }"#).contains("isn't a scope"));
        let alone = Manifest::parse(r#"{ "name": "a", "extensions": ["a"], "abbreviations": "symbols.tsv" }"#).err().unwrap_or_default();
        assert!(alone.contains("a folder of its own"), "{alone}");
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
        assert!(parse(r#"{ "name": "a", "api": 2, "code": "../secret" }"#).contains("isn't in its folder"));
        assert!(parse(r#"{ "name": "a", "api": 2, "code": "/etc/hosts" }"#).contains("isn't in its folder"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(parent.join("secret"), folder.join("link")).unwrap();
            assert!(parse(r#"{ "name": "a", "api": 2, "code": "link" }"#).contains("isn't in its folder"), "nor by way of a link");
        }
        assert!(parse(r#"{ "name": "a", "code": "x.wasm" }"#).contains("doesn't say which `api`"));
        assert!(parse(r#"{ "name": "a", "api": 1, "code": "x.wasm" }"#).contains("built for version 1"));
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
        let no_folder = Manifest::parse(r#"{ "name": "a", "api": 2, "code": "x.wasm" }"#).err().unwrap_or_default();
        assert!(no_folder.contains("a folder of its own"), "{no_folder}");
    }
}
