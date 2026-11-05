//! Helix's `:` commands: parsing, Tab completion, and history. The app runs them.

use std::path::Path;

use ghostwrite_project::{resolve, store};

/// What `:plugin` is asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum ManagePlugins {
    /// Fetch it and say what it can do; with `!` (`confirmed`), install it.
    Install {
        source: String,
        confirmed: bool,
    },
    /// A folder, used where it is, and read again when it changes.
    Link(String),
    /// One, or all that can be.
    Update(Option<String>),
    Remove(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// A command a plugin adds.
    FromPlugin {
        name: String,
        args: Vec<String>,
    },
    ManagePlugins(ManagePlugins),
    Open(Vec<String>),
    /// `:vsplit`: a second pane beside this one.
    Split,
    /// `:only`: only this pane.
    Only,
    /// `:break`: a breakpoint on the cursor's line, or none if there was one.
    Break,
    /// `:debug program args`.
    Debug(String),
    /// `:over`, `:step`, `:finish`, `:continue`, by what the debugger calls them.
    DebuggerGo(&'static str),
    /// `:stop`: stop debugging.
    StopDebugging,
    /// `:plugins`: what's been read from files, and what couldn't be.
    Plugins,
    /// `:tutor`: the keys, to try out in a tab.
    Tutor,
    /// `:update`: is there a newer version?
    Update,
    /// `:undo-story`: take back what the last story's accepted steps did.
    UndoStory,
    /// `:timings`: how long the model has kept you waiting, lately.
    Timings,
    /// `:clear`: end the story, and forget what the model kept of the ones before.
    Clear,
    /// `:replace old new`: go through the project's matches as steps; with `!`, apply them all.
    Replace {
        pattern: String,
        with: String,
        all: bool,
    },
    /// `:api-key ‹key›`: keep it where the system keeps secrets and ask the model with it; `clear` takes it out.
    ApiKey(Option<String>),
    /// `:model opus-5.5`: ask that from now on; alone, say what's asked and what could be.
    Model(Option<String>),
    /// `:w`, `:w!` (over changes on disk, or another file), `:wq`, `:x`.
    Write {
        path: Option<String>,
        force: bool,
        quit: bool,
        /// `:x`: only if there's something to write.
        changed_only: bool,
    },
    WriteAll {
        quit: bool,
    },
    /// Close the window (`:q`), asking about unsaved changes unless `force`d.
    Quit {
        force: bool,
    },
    /// Quit the app: every window (`:qa`).
    QuitAll {
        force: bool,
    },
    New,
    BufferNext,
    /// Move the tab along (`:tab-left`, `:tab-right`).
    TabLeft,
    TabRight,
    BufferPrev,
    BufferClose {
        force: bool,
    },
    /// Move the current file, or `path`, to the Trash.
    Remove {
        path: Option<String>,
        force: bool,
    },
    Reload {
        force: bool,
    },
    Goto(usize),
    /// Search the project, and go through the matches.
    Grep(String),
    /// Show or hide the file tree.
    Tree,
    /// Open the terminal, typing in it.
    Terminal,
    /// A file's pages in their tab, or what's beside it (`:preview`).
    Preview,
    /// The projects page, or straight into a folder.
    Project(Option<String>),
    /// Format the file as its language server does.
    Format,
}

/// Names Tab completes, as typed in helix.
const NAMES: &[&str] = &[
    "open",
    "edit",
    "write",
    "write!",
    "write-quit",
    "quit",
    "quit!",
    "write-all",
    "write-quit-all",
    "quit-all",
    "quit-all!",
    "new",
    "reload",
    "reload!",
    "rm",
    "rm!",
    "grep",
    "buffer-next",
    "tab-left",
    "tab-right",
    "buffer-previous",
    "buffer-close",
    "buffer-close!",
    "tree",
    "term",
    "preview",
    "project",
    "format",
    "model",
    "api-key",
    "replace",
    "undo-story",
    "timings",
    "clear",
    "tutor",
    "plugin",
    "plugins",
    "break",
    "debug",
    "over",
    "step",
    "finish",
    "continue",
    "stop",
    "vsplit",
    "only",
    "update",
];

pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim_start();
    let Some(name) = line.split_whitespace().next() else { return Err(String::new()) };
    let rest = line[name.len()..].trim();
    let args: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
    if let Ok(line) = name.parse::<usize>() {
        return Ok(Command::Goto(line));
    }
    let no_args = |cmd: Command| if args.is_empty() { Ok(cmd) } else { Err(format!(":{name} takes no arguments")) };
    let one_path = |what: &str| match args.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(format!(":{name} takes one {what}")),
    };
    match name {
        "grep" | "g" | "search" if rest.is_empty() => Err(format!(":{name} needs something to search for")),
        "grep" | "g" | "search" => Ok(Command::Grep(rest.to_string())),
        "o" | "open" | "e" | "edit" if args.is_empty() => Err(format!(":{name} needs a path")),
        "o" | "open" | "e" | "edit" => Ok(Command::Open(args)),
        "w" | "write" | "w!" | "write!" | "wq" | "wq!" | "x" | "x!" | "write-quit" | "write-quit!" => {
            let quit = matches!(name.trim_end_matches('!'), "wq" | "x" | "write-quit");
            let changed_only = name.starts_with('x');
            one_path("path").map(|path| Command::Write { path, force: name.ends_with('!'), quit, changed_only })
        }
        "q" | "quit" => no_args(Command::Quit { force: false }),
        "q!" | "quit!" => no_args(Command::Quit { force: true }),
        "qa" | "quit-all" => no_args(Command::QuitAll { force: false }),
        "qa!" | "quit-all!" => no_args(Command::QuitAll { force: true }),
        "wa" | "write-all" => no_args(Command::WriteAll { quit: false }),
        "wqa" | "xa" | "write-quit-all" => no_args(Command::WriteAll { quit: true }),
        "rm" | "rm!" => one_path("path").map(|path| Command::Remove { path, force: name == "rm!" }),
        "rl" | "reload" => no_args(Command::Reload { force: false }),
        "rl!" | "reload!" => no_args(Command::Reload { force: true }),
        "tree" | "explorer" => no_args(Command::Tree),
        "term" | "terminal" => no_args(Command::Terminal),
        "preview" => no_args(Command::Preview),
        "format" | "fmt" => no_args(Command::Format),
        "model" => Ok(Command::Model((!rest.is_empty()).then(|| rest.to_string()))),
        "vs" | "vsplit" => no_args(Command::Split),
        "only" => no_args(Command::Only),
        "break" => no_args(Command::Break),
        "debug" => Ok(Command::Debug(rest.to_string())),
        "over" => no_args(Command::DebuggerGo("next")),
        "step" => no_args(Command::DebuggerGo("stepIn")),
        "finish" => no_args(Command::DebuggerGo("stepOut")),
        "continue" => no_args(Command::DebuggerGo("continue")),
        "stop" => no_args(Command::StopDebugging),
        "plugins" => no_args(Command::Plugins),
        "plugin" => match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["install", source] => Ok(Command::ManagePlugins(ManagePlugins::Install { source: source.to_string(), confirmed: false })),
            ["install!", source] => Ok(Command::ManagePlugins(ManagePlugins::Install { source: source.to_string(), confirmed: true })),
            ["link", folder] => Ok(Command::ManagePlugins(ManagePlugins::Link(folder.to_string()))),
            ["update"] => Ok(Command::ManagePlugins(ManagePlugins::Update(None))),
            ["update", plugin] => Ok(Command::ManagePlugins(ManagePlugins::Update(Some(plugin.to_string())))),
            ["remove", plugin] => Ok(Command::ManagePlugins(ManagePlugins::Remove(plugin.to_string()))),
            ["list"] => Ok(Command::Plugins),
            _ => Err(":plugin install ‹folder, owner/repo[/name][@version] or git URL›, link ‹folder›, update [name], remove ‹name›, or list".into()),
        },
        "tutor" => no_args(Command::Tutor),
        "update" => no_args(Command::Update),
        "undo-story" => no_args(Command::UndoStory),
        "timings" => no_args(Command::Timings),
        "clear" => no_args(Command::Clear),
        "replace" | "replace!" => match rest.split_once(char::is_whitespace) {
            Some((pattern, with)) => Ok(Command::Replace { pattern: pattern.to_string(), with: with.trim().to_string(), all: name.ends_with('!') }),
            None => Err("replace what with what? :replace old new".into()),
        },
        "api-key" => Ok(Command::ApiKey((!rest.is_empty()).then(|| rest.to_string()))),
        "project" | "projects" => one_path("folder").map(Command::Project),
        "n" | "new" => no_args(Command::New),
        "tab-left" => no_args(Command::TabLeft),
        "tab-right" => no_args(Command::TabRight),
        "bn" | "bnext" | "buffer-next" => no_args(Command::BufferNext),
        "bp" | "bprev" | "buffer-previous" => no_args(Command::BufferPrev),
        "bc" | "bclose" | "buffer-close" => no_args(Command::BufferClose { force: false }),
        "bc!" | "bclose!" | "buffer-close!" => no_args(Command::BufferClose { force: true }),
        // The editor's own come first, whatever a plugin calls its commands.
        _ if ghostwrite_plugin::with_command(name).is_some() => Ok(Command::FromPlugin { name: name.to_string(), args }),
        _ => Err(format!("no such command: {name}")),
    }
}

fn common_prefix<'a>(words: &[&'a str]) -> &'a str {
    let Some(first) = words.first() else { return "" };
    let mut len = first.len();
    for w in &words[1..] {
        len = first.char_indices().zip(w.chars()).take_while(|((_, a), b)| a == b).last().map_or(0, |((i, c), _)| i + c.len_utf8()).min(len);
    }
    &first[..len]
}

/// Tab: complete the command name, or the path being typed. Returns the new
/// line and, when the completion is ambiguous, the candidates.
pub fn complete(line: &str, root: &Path) -> (String, Vec<String>) {
    if let Some(typed) = line.strip_prefix("model ") {
        return complete_model(typed, &ghostwrite_ai::models::names());
    }
    if let Some(completed) = complete_plugin(line) {
        return completed;
    }
    let Some(split) = line.rfind(char::is_whitespace) else {
        let from_plugins = ghostwrite_plugin::command_names();
        let hits: Vec<&str> = NAMES.iter().copied().chain(from_plugins).filter(|n| n.starts_with(line)).collect();
        let done = if hits.len() == 1 { format!("{} ", hits[0]) } else { common_prefix(&hits).to_string() };
        let shown = if hits.len() > 1 { hits.iter().map(|h| h.to_string()).collect() } else { vec![] };
        return (if done.len() >= line.len() { done } else { line.to_string() }, shown);
    };
    let (head, token) = line.split_at(split + 1);
    let (dir, prefix) = match token.rfind('/') {
        Some(i) => token.split_at(i + 1),
        None => ("", token),
    };
    let Ok(entries) = std::fs::read_dir(resolve(if dir.is_empty() { "." } else { dir }, root)) else { return (line.to_string(), vec![]) };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden = name.starts_with('.') && !prefix.starts_with('.');
            (!hidden && name.starts_with(prefix)).then(|| if e.path().is_dir() { format!("{name}/") } else { name })
        })
        .collect();
    names.sort();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let completed = common_prefix(&refs);
    let line = format!("{head}{dir}{}", if completed.len() > prefix.len() { completed } else { prefix });
    (line, if names.len() > 1 { names } else { vec![] })
}

/// Complete a model's name among `names`. One it could be is written out; several
/// are as far as they agree, if what's typed is how they start.
/// `:plugin`'s words, and the plugins it's about. `None` for the rest (a path to install).
fn complete_plugin(line: &str) -> Option<(String, Vec<String>)> {
    let rest = line.strip_prefix("plugin ")?;
    let (head, typed, among): (&str, &str, Vec<String>) = match rest.split_once(' ') {
        None => ("", rest, ["install", "link", "update", "remove", "list"].map(String::from).to_vec()),
        Some((word @ ("update" | "remove"), typed)) => (word, typed, ghostwrite_plugin::listed().into_iter().map(|(name, _)| name).collect()),
        Some(_) => return None,
    };
    let hits: Vec<&str> = among.iter().map(String::as_str).filter(|name| name.starts_with(typed)).collect();
    let done = if hits.len() == 1 { hits[0] } else { common_prefix(&hits) };
    let done = if done.len() >= typed.len() { done } else { typed };
    let space = if head.is_empty() { "" } else { " " };
    let end = if hits.len() == 1 && head.is_empty() { " " } else { "" };
    let shown = if hits.len() > 1 { hits.iter().map(|hit| hit.to_string()).collect() } else { vec![] };
    Some((format!("plugin {head}{space}{done}{end}"), shown))
}

fn complete_model(typed: &str, names: &[String]) -> (String, Vec<String>) {
    let hits = ghostwrite_ai::models::matching(names, typed.trim());
    let line = |name: &str| format!("model {name}");
    match hits.as_slice() {
        [] => (line(typed), vec![]),
        [one] => (line(one), vec![]),
        several => {
            let lower: Vec<String> = several.iter().map(|h| h.to_lowercase()).collect();
            let refs: Vec<&str> = lower.iter().map(String::as_str).collect();
            let agreed = common_prefix(&refs).len();
            let done = if agreed > typed.len() && lower.iter().all(|h| h.starts_with(&typed.to_lowercase())) { &several[0][..agreed] } else { typed };
            (line(done), several.iter().map(|h| h.to_string()).collect())
        }
    }
}

/// The `:` commands run before, oldest first, kept between launches (and
/// shared by the windows), and where ↑/↓ is in them.
#[derive(Default)]
pub struct History {
    lines: Vec<String>,
    at: Option<usize>,
    /// Where it's kept, if anywhere: without one it's only this window's.
    kept_as: Option<&'static str>,
}

/// Where it's kept, and how many.
const HISTORY: &str = "command-history";
const MAX_HISTORY: usize = 200;

impl History {
    pub fn load() -> Self {
        Self { lines: store::lines(HISTORY), at: None, kept_as: Some(HISTORY) }
    }

    /// A command was run: it's the latest.
    pub fn remember(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        // Another window may have added to it since.
        if let Some(name) = self.kept_as {
            self.lines = store::lines(name);
        }
        self.lines.retain(|l| l != line);
        self.lines.push(line.to_string());
        if let Some(name) = self.kept_as {
            store::write_lines(name, &self.lines, MAX_HISTORY);
        }
    }

    /// Start from the newest again (a new `:` line).
    pub fn rewind(&mut self) {
        self.at = None;
    }

    /// ↑: the previous command; ↓ forward again, then back to an empty line.
    pub fn step(&mut self, back: bool) -> Option<String> {
        let len = self.lines.len();
        self.at = match (self.at, back) {
            (None, true) if len > 0 => Some(len - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            (Some(_), false) => None,
            (None, _) => return None,
        };
        Some(self.at.map(|i| self.lines[i].clone()).unwrap_or_default())
    }
}

/// What's typed that must not be kept: `:api-key` has a secret in it.
pub fn is_secret(line: &str) -> bool {
    line.split_whitespace().next() == Some("api-key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_helix_commands() {
        assert_eq!(parse("e src/main.rs"), Ok(Command::Open(vec!["src/main.rs".into()])));
        assert_eq!(parse("w"), Ok(Command::Write { path: None, force: false, quit: false, changed_only: false }));
        assert_eq!(parse("w! a.txt"), Ok(Command::Write { path: Some("a.txt".into()), force: true, quit: false, changed_only: false }));
        assert_eq!(parse("wq"), Ok(Command::Write { path: None, force: false, quit: true, changed_only: false }));
        assert_eq!(parse("x!"), Ok(Command::Write { path: None, force: true, quit: true, changed_only: true }));
        assert_eq!(parse("q"), Ok(Command::Quit { force: false }));
        assert_eq!(parse("model"), Ok(Command::Model(None)));
        assert_eq!(parse("model opus-5.5"), Ok(Command::Model(Some("opus-5.5".into()))));
        assert_eq!(parse("q!"), Ok(Command::Quit { force: true }));
        assert_eq!(parse("bc!"), Ok(Command::BufferClose { force: true }));
        assert_eq!(parse("42"), Ok(Command::Goto(42)));
        assert_eq!(parse("wa"), Ok(Command::WriteAll { quit: false }));
        assert_eq!(parse("grep fn  main("), Ok(Command::Grep("fn  main(".into())));
        assert!(parse("grep").is_err());
        assert_eq!(parse("rm"), Ok(Command::Remove { path: None, force: false }));
        assert_eq!(parse("rm! a.txt"), Ok(Command::Remove { path: Some("a.txt".into()), force: true }));
        assert_eq!(parse("project ~/code"), Ok(Command::Project(Some("~/code".into()))));
        assert_eq!(parse("term"), Ok(Command::Terminal));
        assert!(parse("e").is_err());
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn completes_models_by_any_part_of_their_name() {
        let names: Vec<String> = ["opus-5.5", "sonnet-5.5", "qwen3:4b", "qwen3:latest", "Qwen2.5-Coder-14B"].map(str::to_string).into();
        assert_eq!(complete_model("op", &names), ("model opus-5.5".to_string(), vec![]));
        assert_eq!(complete_model("14b", &names), ("model Qwen2.5-Coder-14B".to_string(), vec![]));
        let (line, shown) = complete_model("qwen", &names);
        assert_eq!(line, "model qwen", "they agree only on what's typed");
        assert_eq!(shown.len(), 3);
        let names: Vec<String> = ["qwen3:4b", "qwen3:latest"].map(str::to_string).into();
        assert_eq!(complete_model("qw", &names).0, "model qwen3:", "as far as they agree");
        assert_eq!(complete_model("", &names).1.len(), 2, "nothing typed suggests them all");
        assert_eq!(complete_model("nope", &names), ("model nope".to_string(), vec![]));
    }

    #[test]
    fn completes_names_and_paths() {
        let root = ghostwrite_project::scratch_dir("cmd");
        std::fs::create_dir_all(root.join("src")).unwrap();
        for f in ["src/main.rs", "src/modal.rs", "README.md"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        assert_eq!(complete("ne", &root).0, "new ");
        assert_eq!(complete("write-", &root), ("write-".into(), vec!["write-quit".into(), "write-all".into(), "write-quit-all".into()]));
        assert_eq!(complete("e R", &root), ("e README.md".into(), vec![]));
        assert_eq!(complete("e s", &root), ("e src/".into(), vec![]));
        let (line, hits) = complete("e src/m", &root);
        assert_eq!(line, "e src/m");
        assert_eq!(hits, ["main.rs", "modal.rs"]);
        assert_eq!(complete("e src/ma", &root).0, "e src/main.rs");
    }
}
