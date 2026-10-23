//! Helix's `:` commands: parsing and Tab completion. The editor runs them.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open(Vec<String>),
    Write(Option<String>),
    QuitAll { force: bool },
    New,
    BufferNext,
    BufferPrev,
    BufferClose { force: bool },
    WriteAll { quit: bool },
    /// Move the current file, or `path`, to the Trash.
    Remove { path: Option<String>, force: bool },
    Reload { force: bool },
    Goto(usize),
    /// Run a shell command (`None`: the last one again).
    Shell(Option<String>),
    /// Search the project; the matches are a tour.
    Grep(String),
    /// Ghostwrite's own: the file tree and projects.
    Tree,
    Project(Option<String>),
}

/// Names Tab completes, as typed in helix.
const NAMES: &[&str] = &[
    "open", "edit", "write", "write-all", "write-quit-all", "quit-all", "quit-all!",
    "new", "reload", "reload!", "rm", "rm!", "sh", "grep", "buffer-next", "buffer-previous", "buffer-close", "buffer-close!", "tree", "project",
];

pub fn parse(line: &str) -> Result<Command, String> {
    let mut words = line.split_whitespace();
    let Some(name) = words.next() else { return Err(String::new()) };
    let args: Vec<String> = words.map(str::to_string).collect();
    if let Ok(line) = name.parse::<usize>() {
        return Ok(Command::Goto(line));
    }
    if matches!(name, "sh" | "shell" | "run") {
        let rest = line.trim_start()[name.len()..].trim();
        return Ok(Command::Shell((!rest.is_empty()).then(|| rest.to_string())));
    }
    if matches!(name, "grep" | "g" | "search") {
        let rest = line.trim_start()[name.len()..].trim();
        return if rest.is_empty() { Err(format!(":{name} needs something to search for")) } else { Ok(Command::Grep(rest.to_string())) };
    }
    let no_args = |cmd: Command| if args.is_empty() { Ok(cmd) } else { Err(format!(":{name} takes no arguments")) };
    match name {
        "o" | "open" | "e" | "edit" => {
            if args.is_empty() {
                Err(format!(":{name} needs a path"))
            } else {
                Ok(Command::Open(args))
            }
        }
        "w" | "write" | "w!" | "write!" => match args.as_slice() {
            [] => Ok(Command::Write(None)),
            [path] => Ok(Command::Write(Some(path.clone()))),
            _ => Err(":write takes one path".into()),
        },
        "qa" | "quit-all" => no_args(Command::QuitAll { force: false }),
        "qa!" | "quit-all!" => no_args(Command::QuitAll { force: true }),
        "wa" | "write-all" => no_args(Command::WriteAll { quit: false }),
        "wqa" | "xa" | "write-quit-all" => no_args(Command::WriteAll { quit: true }),
        "rm" | "rm!" => match args.as_slice() {
            [] => Ok(Command::Remove { path: None, force: name == "rm!" }),
            [path] => Ok(Command::Remove { path: Some(path.clone()), force: name == "rm!" }),
            _ => Err(":rm takes one path".into()),
        },
        "rl" | "reload" => no_args(Command::Reload { force: false }),
        "rl!" | "reload!" => no_args(Command::Reload { force: true }),
        "tree" | "explorer" => no_args(Command::Tree),
        "project" | "projects" => match args.as_slice() {
            [] => Ok(Command::Project(None)),
            [path] => Ok(Command::Project(Some(path.clone()))),
            _ => Err(":project takes one folder".into()),
        },
        "n" | "new" => no_args(Command::New),
        "bn" | "bnext" | "buffer-next" => no_args(Command::BufferNext),
        "bp" | "bprev" | "buffer-previous" => no_args(Command::BufferPrev),
        "bc" | "bclose" | "buffer-close" => no_args(Command::BufferClose { force: false }),
        "bc!" | "bclose!" | "buffer-close!" => no_args(Command::BufferClose { force: true }),
        _ => Err(format!("no such command: {name}")),
    }
}

/// `~/` is home; anything relative is from the project root.
pub fn resolve(arg: &str, root: &Path) -> PathBuf {
    let path = match arg.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(arg),
    };
    if path.is_absolute() { path } else { root.join(path) }
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
    let base = resolve(if dir.is_empty() { "." } else { dir }, root);
    let Ok(entries) = std::fs::read_dir(&base) else { return (line.to_string(), vec![]) };
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
        let root = std::env::temp_dir().join(format!("ghostwrite-cmd-{}", std::process::id()));
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
        std::fs::remove_dir_all(root).ok();
    }
}
