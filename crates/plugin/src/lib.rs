//! What a plugin can add to the editor. The editor knows no file type in
//! particular: it knows what's asked of a file's language here, and which
//! plugins there are, as the app lists them when it starts.
//!
//! A plugin is a crate, built in, or comes from a folder ([`manifest`]): a file saying
//! what it adds, and maybe code in WebAssembly and grammars, run by a [`Runtime`]
//! and installed with [`install`]. It adds any of these, all optional:
//!
//! - languages ([`Language`]): files, grammars and highlighting, text objects,
//!   abbreviations, comments, indentation;
//! - grammars on their own ([`Grammar`]), for what's inside a language;
//! - language servers ([`Spec`]), what they start with and their settings, and
//!   the kinds of token they name;
//! - a [`Renderer`]: a file's pages, in a tab of their own (`:preview`);
//! - a [`Panel`]: something beside a file, kept up to date from its server;
//! - key bindings ([`Binding`]), for the files they're about;
//! - `:` commands ([`PluginCommand`]), and answers to what happens to files ([`Event`]);
//! - rules for the model's prompt.

pub mod install;
pub mod manifest;

use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, OnceLock, RwLock};

pub use ghostwrite_core::Language;
use ghostwrite_core::syntax::Grammar;
use ghostwrite_lsp::registry::Spec;
pub use ghostwrite_page::{Bitmap, Renderer};
use serde_json::Value;

/// A key sequence that runs a `:` command, in normal and select mode. Keys built in
/// are the editor's to give: a plugin's mustn't be one of them (a test keeps to that).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The keys, as the help writes them: `space v`, `g x`.
    pub keys: &'static str,
    /// The command line, without its `:`.
    pub command: &'static str,
    /// What it does, for the help.
    pub what: &'static str,
}

/// A `:` command a plugin adds. Those the editor has are the editor's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginCommand {
    pub name: &'static str,
    /// What it does, for the list of plugins.
    pub what: &'static str,
}

/// What happens to a file that a plugin can ask to hear of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Opened,
    Saved,
    Closed,
}

/// The file a plugin is asked about, as it is now (none open: no path, no text).
/// Offsets are bytes into `text`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub root: String,
    pub path: Option<String>,
    pub language: Option<String>,
    pub text: String,
    pub cursor: usize,
    pub selection: Range<usize>,
}

/// What a plugin answers with: what the editor is to do. The editor does it, or
/// says why not, so a plugin can't do anything it isn't shown doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A line for the status bar.
    Say(String),
    /// Replace part of the text it was shown. A reply's edits are all about that
    /// same text, and are one undo step.
    Edit { range: Range<usize>, text: String },
    /// Open a file of the project, relative to its root.
    Open(String),
}

/// What a plugin's commands and event answers come to.
pub type Reply = Result<Vec<Action>, String>;

pub trait Plugin: Send + Sync {
    /// What it's called, if it's one to list: nothing for what's part of the editor.
    fn name(&self) -> &str {
        ""
    }

    /// What it adds, in a line, for the list of plugins.
    fn describe(&self) -> String {
        String::new()
    }

    /// Its `:` commands.
    fn commands(&self) -> &'static [PluginCommand] {
        &[]
    }

    /// Run one of its commands. It's asked off the UI thread.
    fn run(&self, name: &str, _args: &[String], _context: &Context) -> Reply {
        Err(format!("there's no command {name} here"))
    }

    /// What it wants to hear of.
    fn events(&self) -> &'static [Event] {
        &[]
    }

    /// Something it wants to hear of happened. It's asked off the UI thread.
    fn on_event(&self, _event: Event, _context: &Context) -> Reply {
        Ok(vec![])
    }

    /// Its key bindings for a file of `language` (`None`: a file of none).
    fn keys(&self, _language: Option<&Language>) -> &'static [Binding] {
        &[]
    }

    fn languages(&self) -> Vec<&'static Language> {
        vec![]
    }

    /// Grammars that aren't a language's, for what's inside one (a markup language's inline text, say).
    fn grammars(&self) -> Vec<&'static Grammar> {
        vec![]
    }

    fn servers(&self) -> Vec<&'static Spec> {
        vec![]
    }

    /// Kinds of semantic token its servers name, and the scope each shows as.
    fn tokens(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Rules for the model's prompt, as lines starting `- `.
    fn prompt_rules(&self) -> &'static str {
        ""
    }

    /// Whether it makes pages of files of `language`.
    fn renders(&self, _language: &Language) -> bool {
        false
    }

    /// What makes pages of the file at `path`, in the project at `root`.
    fn renderer(&self, _root: &Path, _path: &Path) -> Result<Box<dyn Renderer>, String> {
        Err("this plugin doesn't make pages".into())
    }

    /// What's beside a file of `language`, if it's this plugin's.
    fn panel(&self, _language: &Language) -> Option<Box<dyn Panel>> {
        None
    }
}

/// Something beside a file, kept up to date from its language server. It says
/// what to ask and what to show; the editor asks, draws and keeps it to the cursor.
pub trait Panel: Send {
    /// What it's called, at its top.
    fn title(&self) -> &'static str;
    /// Requests to its server about the cursor, sent again as it moves or the text changes.
    fn requests(&self) -> &'static [&'static str];
    /// The answers, in that order, each unless it failed or was empty.
    fn answered(&mut self, answers: Vec<Option<Value>>);
    /// A notification of the server's own about its file. `true`: ask again.
    fn notification(&mut self, _method: &str, _params: &Value) -> bool {
        false
    }
    /// The first line the server is still working on, if it's said.
    fn working_on(&self) -> Option<u32> {
        None
    }
    fn blocks(&self) -> Vec<Block>;
}

/// A part of what a [`Panel`] shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    /// In the code's font; lines starting `stands_out`, if anything, stand out.
    Code {
        text: String,
        stands_out: Option<char>,
    },
    /// Something good, in a word.
    Good(String),
    Muted(String),
}

static PLUGINS: RwLock<Vec<&'static dyn Plugin>> = RwLock::new(Vec::new());
static RUNTIME: OnceLock<Box<dyn Runtime>> = OnceLock::new();

/// What runs a plugin's code and grammars, if this build has something that does.
pub trait Runtime: Send + Sync {
    /// The code at `path`, ready to be asked (it's loaded when it's first needed),
    /// with what its manifest says it may have.
    fn code(&self, name: &str, path: &Path, access: &manifest::Access) -> Box<dyn Code>;
    /// What loads the grammar `name` at `path`, and parses with it.
    fn grammar(&self, name: &str, path: &Path) -> Box<dyn ghostwrite_core::syntax::Load>;
}

/// A plugin's code, as a [`Runtime`] runs it.
pub trait Code: Send + Sync {
    /// Whether it can be run, or what's wrong with it.
    fn check(&self) -> Result<(), String>;
    fn run(&self, name: &str, args: &[String], context: &Context) -> Reply;
    fn on_event(&self, event: Event, context: &Context) -> Reply;
    /// What makes pages of the file at `path`, in the project at `root`.
    fn renderer(&self, root: &Path, path: &Path) -> Result<Box<dyn Renderer>, String>;
    /// What it colors in `text`, in order, and each's scope; nothing if it can't.
    fn highlight(&self, text: &str) -> Vec<(Range<usize>, String)>;
    /// What its language server `server` starts with, in the project at `project`, started
    /// in `root`: variables over `env` (what it would have), of which it's shown what its
    /// access lists.
    fn server_env(&self, server: &str, project: &Path, root: &Path, env: &[(String, String)]) -> Result<Vec<(String, String)>, String>;
    /// Its answer to `server`'s `workspace/configuration` for `section`.
    fn server_settings(&self, server: &str, project: &Path, root: &Path, env: &[(String, String)], section: &str) -> Result<Value, String>;
    /// Its panel beside a file of `language`, before anything's been asked.
    fn panel_open(&self, language: &str) -> Result<Shown, String>;
    /// The panel kept as `state`, with the server's answers to its requests.
    fn panel_answered(&self, state: &str, answers: &[Option<Value>]) -> Result<Shown, String>;
    /// The panel kept as `state`, after a notification of its server's; and whether to ask again.
    fn panel_notification(&self, state: &str, method: &str, params: &Value) -> Result<(Shown, bool), String>;
}

/// A panel as a plugin's code says it is: what it shows, and what it keeps about its
/// file (`state`, the code's own), which is held here and handed back with the next call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    pub state: String,
    pub blocks: Vec<Block>,
    pub working_on: Option<u32>,
}

/// What runs plugins' code and grammars from now on (only the first call counts).
pub fn set_runtime(runtime: Box<dyn Runtime>) {
    RUNTIME.get_or_init(|| runtime);
}

fn runtime() -> Option<&'static dyn Runtime> {
    RUNTIME.get().map(Box::as_ref)
}

/// Start with `plugins` (only the first call counts): what they add is added.
pub fn install(plugins: impl FnOnce() -> Vec<Box<dyn Plugin>>) {
    static INSTALLED: Mutex<bool> = Mutex::new(false);
    let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    if !std::mem::replace(&mut *installed, true) {
        plugins().into_iter().for_each(add);
    }
}

/// `plugin` is there from now on, with what it adds.
pub fn add(plugin: Box<dyn Plugin>) {
    let plugin: &'static dyn Plugin = Box::leak(plugin);
    plugin.languages().into_iter().for_each(Language::register);
    plugin.grammars().into_iter().for_each(Grammar::register);
    plugin.servers().into_iter().for_each(ghostwrite_lsp::registry::register);
    plugin.tokens().iter().for_each(|(token, scope)| ghostwrite_lsp::convert::register_token(token, scope));
    PLUGINS.write().unwrap_or_else(|e| e.into_inner()).push(plugin);
}

/// The plugin called `name` is gone, with what it adds (files open in its language
/// keep it until they're closed, and servers running go on until then).
pub fn remove(name: &str) {
    let mut plugins = PLUGINS.write().unwrap_or_else(|e| e.into_inner());
    plugins.retain(|plugin| {
        let keep = plugin.name() != name;
        if !keep {
            plugin.languages().into_iter().for_each(Language::unregister);
            plugin.grammars().into_iter().for_each(Grammar::unregister);
            plugin.servers().into_iter().for_each(ghostwrite_lsp::registry::unregister);
        }
        keep
    });
}

fn plugins() -> Vec<&'static dyn Plugin> {
    PLUGINS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The plugin with the command `name`, if any has it.
pub fn with_command(name: &str) -> Option<&'static dyn Plugin> {
    plugins().into_iter().find(|plugin| plugin.commands().iter().any(|command| command.name == name))
}

/// The commands plugins add.
pub fn command_names() -> Vec<&'static str> {
    plugins().into_iter().flat_map(|plugin| plugin.commands()).map(|command| command.name).collect()
}

/// The plugins that want to hear of `event`.
pub fn hearing(event: Event) -> Vec<&'static dyn Plugin> {
    plugins().into_iter().filter(|plugin| plugin.events().contains(&event)).collect()
}

/// The plugins to list: each's name, and what it adds.
pub fn listed() -> Vec<(String, String)> {
    plugins().into_iter().filter(|plugin| !plugin.name().is_empty()).map(|plugin| (plugin.name().to_string(), plugin.describe())).collect()
}

/// The bindings for a file of `language`.
pub fn keys(language: Option<&Language>) -> Vec<&'static Binding> {
    plugins().iter().flat_map(|p| p.keys(language)).collect()
}

/// Whether there are pages to make of a file of `language`.
pub fn renders(language: &Language) -> bool {
    plugins().iter().any(|p| p.renders(language))
}

/// What makes pages of a file of `language`, if anything does.
pub fn renderer(language: &Language, root: &Path, path: &Path) -> Option<Result<Box<dyn Renderer>, String>> {
    plugins().iter().find(|p| p.renders(language)).map(|p| p.renderer(root, path))
}

/// What's beside a file of `language`, if anything is.
pub fn panel(language: &Language) -> Option<Box<dyn Panel>> {
    plugins().iter().find_map(|p| p.panel(language))
}

/// What the plugins add to the model's prompt.
pub fn prompt_rules() -> String {
    plugins().iter().map(|p| p.prompt_rules()).filter(|r| !r.is_empty()).collect::<Vec<_>>().join("\n")
}
