//! Helix's `:` commands: parsing, Tab completion, history, and running them.

use std::path::Path;

use ghostwrite_project::{fs, resolve, store};
use gpui::{Context, Window};

use super::{Editor, EditorEvent};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open(Vec<String>),
    Write(Option<String>),
    WriteAll { quit: bool },
    QuitAll { force: bool },
    New,
    BufferNext,
    BufferPrev,
    BufferClose { force: bool },
    /// Move the current file, or `path`, to the Trash.
    Remove { path: Option<String>, force: bool },
    Reload { force: bool },
    Goto(usize),
    /// Run a shell command in the background (`None`: the last one again).
    Shell(Option<String>),
    /// Stop the running command.
    Stop,
    /// Tour the last command's errors, or its output.
    Output,
    /// Search the project; the matches are a tour.
    Grep(String),
    /// Show or hide the file tree.
    Tree,
    /// The projects page, or straight into a folder.
    Project(Option<String>),
}

/// Names Tab completes, as typed in helix.
const NAMES: &[&str] = &[
    "open", "edit", "write", "write-all", "write-quit-all", "quit-all", "quit-all!", "new", "reload", "reload!", "rm", "rm!", "sh", "stop", "output", "grep",
    "buffer-next", "buffer-previous", "buffer-close", "buffer-close!", "tree", "project",
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
        "sh" | "shell" | "run" => Ok(Command::Shell((!rest.is_empty()).then(|| rest.to_string()))),
        "stop" => no_args(Command::Stop),
        "output" | "out" => no_args(Command::Output),
        "grep" | "g" | "search" if rest.is_empty() => Err(format!(":{name} needs something to search for")),
        "grep" | "g" | "search" => Ok(Command::Grep(rest.to_string())),
        "o" | "open" | "e" | "edit" if args.is_empty() => Err(format!(":{name} needs a path")),
        "o" | "open" | "e" | "edit" => Ok(Command::Open(args)),
        "w" | "write" | "w!" | "write!" => one_path("path").map(Command::Write),
        "qa" | "quit-all" => no_args(Command::QuitAll { force: false }),
        "qa!" | "quit-all!" => no_args(Command::QuitAll { force: true }),
        "wa" | "write-all" => no_args(Command::WriteAll { quit: false }),
        "wqa" | "xa" | "write-quit-all" => no_args(Command::WriteAll { quit: true }),
        "rm" | "rm!" => one_path("path").map(|path| Command::Remove { path, force: name == "rm!" }),
        "rl" | "reload" => no_args(Command::Reload { force: false }),
        "rl!" | "reload!" => no_args(Command::Reload { force: true }),
        "tree" | "explorer" => no_args(Command::Tree),
        "project" | "projects" => one_path("folder").map(Command::Project),
        "n" | "new" => no_args(Command::New),
        "bn" | "bnext" | "buffer-next" => no_args(Command::BufferNext),
        "bp" | "bprev" | "buffer-previous" => no_args(Command::BufferPrev),
        "bc" | "bclose" | "buffer-close" => no_args(Command::BufferClose { force: false }),
        "bc!" | "bclose!" | "buffer-close!" => no_args(Command::BufferClose { force: true }),
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
    let Some(split) = line.rfind(char::is_whitespace) else {
        let hits: Vec<&str> = NAMES.iter().copied().filter(|n| n.starts_with(line)).collect();
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

impl Editor {
    pub(super) fn remember_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.history.retain(|l| l != line);
        self.history.push(line.to_string());
        store::save_command_history(&self.history);
    }

    /// ↑ in the `:` line: the previous command; ↓ goes forward again.
    pub(super) fn history_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let len = self.history.len();
        let at = match (self.history_at, back) {
            (None, true) if len > 0 => Some(len - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            (Some(_), false) => None,
            (None, _) => return,
        };
        self.history_at = at;
        let line = at.map(|i| self.history[i].clone()).unwrap_or_default();
        if let Some(prompt) = &self.prompt {
            prompt.field.update(cx, |f, cx| f.set_text(line, cx));
        }
    }

    pub(super) fn complete_command(&mut self, line: &str, cx: &mut Context<Self>) {
        let (completed, candidates) = complete(line, &self.project.root);
        if completed != line
            && let Some(prompt) = &self.prompt
        {
            prompt.field.update(cx, |f, cx| f.set_text(completed, cx));
        }
        self.project.status = (!candidates.is_empty()).then(|| candidates.join("  ").into());
    }

    /// Run a `:` command. `Err` carries the message for the status line.
    pub(super) fn run_command(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let unsaved = |name: &str| format!("{name} has unsaved changes (add ! to discard them)");
        match parse(line)? {
            Command::Open(args) => {
                let paths: Vec<_> = args.iter().map(|a| resolve(a, &self.project.root)).collect();
                if let Some(dir) = paths.iter().find(|p| p.is_dir()) {
                    return Err(format!("{} is a folder", dir.display()));
                }
                self.open_paths(paths, cx);
            }
            Command::Write(path) => {
                let path = match path {
                    Some(p) => resolve(&p, &self.project.root),
                    None => self.current()?.doc.path.clone().ok_or("no file name yet: use :write <path>")?,
                };
                self.project.write(self.project.tabs.active_index(), path);
            }
            Command::WriteAll { quit } => {
                self.project.write_all();
                if let Some(tab) = self.project.tabs.iter().find(|t| t.doc.dirty) {
                    return Err(format!("couldn't write {}", tab.doc.name()));
                }
                self.project.status = Some("wrote all files".into());
                if quit {
                    cx.quit();
                }
            }
            Command::QuitAll { force } => {
                if let Some(tab) = self.project.tabs.iter().find(|t| t.doc.dirty).filter(|_| !force) {
                    return Err(unsaved(&tab.doc.name()));
                }
                cx.quit();
            }
            Command::Remove { path, force } => {
                let target = match path {
                    Some(p) => resolve(&p, &self.project.root),
                    None => self.current()?.doc.path.clone().ok_or("this buffer has no file to remove")?,
                };
                if !target.exists() {
                    return Err(format!("{} doesn't exist", target.display()));
                }
                let affected = |t: &super::Tab| t.doc.path.as_deref().is_some_and(|p| p.starts_with(&target));
                if let Some(tab) = self.project.tabs.iter().find(|t| affected(t) && t.doc.dirty).filter(|_| !force) {
                    return Err(format!("{} has unsaved changes (use :rm! to remove it anyway)", tab.doc.name()));
                }
                fs::trash(&target)?;
                for tab in self.project.tabs.iter_mut().filter(|t| affected(t)) {
                    tab.doc.dirty = false;
                }
                self.project.path_deleted(&target);
                self.tab_changed(cx);
                self.project.status = Some(format!("moved {} to the Trash", ghostwrite_project::display(&target, &self.project.root)).into());
            }
            Command::Reload { force } => {
                let doc = &self.current()?.doc;
                if doc.dirty && !force {
                    return Err(unsaved(&doc.name()));
                }
                let path = doc.path.clone().ok_or("nothing to reload: this buffer has no file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't reload: {e}"))?;
                self.leave_insert();
                if let Some(tab) = self.tab_mut() {
                    tab.reload(text);
                }
            }
            Command::Goto(line) => {
                let tab = self.tab_mut().ok_or("no file open")?;
                let line = line.saturating_sub(1).min(tab.doc.buffer.last_line());
                let at = tab.doc.buffer.line_start(line);
                tab.select(ghostwrite_core::Range::point(at), super::Autoscroll::Center(line..line + 1));
            }
            Command::Shell(command) => {
                let command = command.or_else(|| self.last_shell.clone()).ok_or("usage: :sh <command>")?;
                self.start_shell(command, cx)?;
            }
            Command::Stop => self.stop_shell()?,
            Command::Output => self.output_tour(cx)?,
            Command::Grep(pattern) => self.start_search(&pattern, window, cx)?,
            Command::Tree => self.toggle_tree(cx),
            Command::Project(path) => {
                let path = path.map(|p| resolve(&p, &self.project.root));
                if let Some(p) = path.as_ref().filter(|p| !p.is_dir()) {
                    return Err(format!("{} isn't a folder", p.display()));
                }
                cx.emit(EditorEvent::OpenProject(path));
            }
            Command::New => self.new_buffer(cx),
            Command::BufferNext => self.cycle_tab(true, cx),
            Command::BufferPrev => self.cycle_tab(false, cx),
            Command::BufferClose { force } => {
                let doc = &self.current()?.doc;
                if doc.dirty && !force {
                    return Err(unsaved(&doc.name()));
                }
                self.close_active(cx);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_helix_commands() {
        assert_eq!(parse("e src/main.rs"), Ok(Command::Open(vec!["src/main.rs".into()])));
        assert_eq!(parse("w"), Ok(Command::Write(None)));
        assert_eq!(parse("bc!"), Ok(Command::BufferClose { force: true }));
        assert!(parse("q").is_err() && parse("wq").is_err());
        assert_eq!(parse("42"), Ok(Command::Goto(42)));
        assert_eq!(parse("wa"), Ok(Command::WriteAll { quit: false }));
        assert_eq!(parse("sh cargo test -- --nocapture"), Ok(Command::Shell(Some("cargo test -- --nocapture".into()))));
        assert_eq!(parse("sh"), Ok(Command::Shell(None)));
        assert_eq!(parse("grep fn  main("), Ok(Command::Grep("fn  main(".into())));
        assert!(parse("grep").is_err());
        assert_eq!(parse("rm"), Ok(Command::Remove { path: None, force: false }));
        assert_eq!(parse("rm! a.txt"), Ok(Command::Remove { path: Some("a.txt".into()), force: true }));
        assert_eq!(parse("project ~/code"), Ok(Command::Project(Some("~/code".into()))));
        assert!(parse("e").is_err());
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn completes_names_and_paths() {
        let root = ghostwrite_project::scratch_dir("cmd");
        std::fs::create_dir_all(root.join("src")).unwrap();
        for f in ["src/main.rs", "src/modal.rs", "README.md"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        assert_eq!(complete("ne", &root).0, "new ");
        assert_eq!(complete("write-", &root), ("write-".into(), vec!["write-all".into(), "write-quit-all".into()]));
        assert_eq!(complete("e R", &root), ("e README.md".into(), vec![]));
        assert_eq!(complete("e s", &root), ("e src/".into(), vec![]));
        let (line, hits) = complete("e src/m", &root);
        assert_eq!(line, "e src/m");
        assert_eq!(hits, ["main.rs", "modal.rs"]);
        assert_eq!(complete("e src/ma", &root).0, "e src/main.rs");
    }
}
