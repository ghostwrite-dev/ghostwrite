//! Listing and searching the project's files: all of them, but for the
//! folders tools make (`.gitignore` says what git keeps, not what's worth
//! reading).

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Matches a search stops at.
pub const MAX_FOUND: usize = 2000;

/// Folders tools make and fill: dependencies, caches, build output.
const MADE: [&str; 9] = [".git", "node_modules", ".venv", "venv", "__pycache__", "target", ".mypy_cache", ".pytest_cache", ".ruff_cache"];

/// Files bigger than this aren't searched.
const MAX_SEARCHED: u64 = 8 << 20;

/// Files under `under` (but in [`MADE`] folders), relative to `root`, sorted.
pub fn files(root: &Path, under: &Path) -> Vec<String> {
    let mut out: Vec<String> = ignore::WalkBuilder::new(under)
        .standard_filters(false)
        .filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir()) && MADE.iter().any(|m| e.file_name() == *m)) && e.file_name() != ".DS_Store")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(|p| p.display().to_string()))
        .collect();
    out.sort();
    out
}

/// The file list, cut off after `max` entries.
pub fn listing(root: &Path, max: usize) -> String {
    let all = files(root, root);
    let shown = all.iter().take(max).cloned().collect::<Vec<_>>().join("\n");
    if all.len() > max { format!("{shown}\n… and {} more", all.len() - max) } else { shown }
}

/// One folder's entries (`.git` and `.DS_Store` left out): folders first,
/// then files, each by case-insensitive name, and whether `.gitignore`
/// leaves each out.
pub fn list_dir(dir: &Path) -> Vec<(PathBuf, bool, bool)> {
    let kept: std::collections::HashSet<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| e.into_path())
        .collect();
    let mut entries: Vec<(PathBuf, bool, bool)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != ".git" && e.file_name() != ".DS_Store")
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            let ignored = !kept.contains(&path);
            (path, is_dir, ignored)
        })
        .collect();
    entries.sort_by_cached_key(|(p, is_dir, _)| (!is_dir, p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
    entries
}

/// A file's text to search, unless it's too big or isn't text.
pub fn searchable(path: &Path) -> Option<String> {
    (std::fs::metadata(path).ok()?.len() <= MAX_SEARCHED).then(|| std::fs::read_to_string(path).ok()).flatten()
}

/// What a project search looks for: a regex, smart-case like helix (ignoring
/// case unless there's a capital), or the text as typed if it isn't one.
pub fn search_pattern(typed: &str) -> Result<Regex, String> {
    let flags = if typed.chars().any(char::is_uppercase) { "" } else { "(?i)" };
    Regex::new(&format!("{flags}{typed}")).or_else(|_| Regex::new(&format!("{flags}{}", regex::escape(typed)))).map_err(|e| e.to_string())
}

/// A match: the file, its line (zero-based), the match's bytes in the line, and the line.
#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    pub line: usize,
    pub bytes: Range<usize>,
    pub text: String,
}

/// Every match of `pattern` in the project, file by file, line by line.
/// Files in `open` are searched as they are there (maybe unsaved).
pub fn grep(root: &Path, pattern: &Regex, open: &HashMap<PathBuf, String>) -> Vec<Found> {
    let mut found = vec![];
    for rel in files(root, root) {
        let path = root.join(&rel);
        let Some(text) = open.get(&path).cloned().or_else(|| searchable(&path)) else { continue };
        for (line, text) in text.lines().enumerate() {
            for m in pattern.find_iter(text).filter(|m| !m.is_empty()) {
                found.push(Found { path: path.clone(), line, bytes: m.range(), text: text.to_string() });
                if found.len() >= MAX_FOUND {
                    return found;
                }
            }
        }
    }
    found
}

/// The project's uncommitted changes, as `git diff` shows them (plus new,
/// untracked files by name), cut off after `max` bytes. `None` outside a
/// repository or with nothing changed.
pub fn uncommitted(root: &Path, max: usize) -> Option<String> {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(root).args(args).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let mut diff = git(&["diff", "HEAD", "--no-color", "--no-ext-diff"]).or_else(|| git(&["diff", "--no-color", "--no-ext-diff"]))?;
    let untracked = git(&["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    if !untracked.trim().is_empty() {
        diff.push_str(&format!("\nNew files, not yet in git:\n{}", untracked.trim_end()));
    }
    if diff.trim().is_empty() {
        return None;
    }
    if diff.len() > max {
        diff.truncate(diff.floor_char_boundary(max));
        diff.push_str("\n… (cut off)");
    }
    Some(diff)
}

/// `path` inside the project (resolved), or `None` if it would leave it.
pub fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let real = root.join(path.trim_start_matches('/')).canonicalize().ok()?;
    real.starts_with(&root).then_some(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_is_smart_case_and_prefers_open_text() {
        let root = crate::scratch_dir("grep");
        std::fs::write(root.join("a.rs"), "fn Parse {}\nlet x = parse(y);\n").unwrap();
        std::fs::write(root.join("b.rs"), "nothing\n").unwrap();
        let hits = |p: &str, open: &HashMap<_, _>| {
            grep(&root, &search_pattern(p).unwrap(), open)
                .iter()
                .map(|f| (f.path.file_name().unwrap().to_string_lossy().to_string(), f.line, f.bytes.clone()))
                .collect::<Vec<_>>()
        };
        let none = HashMap::new();
        assert_eq!(hits("parse", &none), [("a.rs".into(), 0, 3..8), ("a.rs".into(), 1, 8..13)], "lowercase ignores case");
        assert_eq!(hits("Parse", &none), [("a.rs".into(), 0, 3..8)], "a capital makes it exact");
        assert_eq!(hits("parse(", &none), [("a.rs".into(), 1, 8..14)], "not a regex: taken as typed");
        let open = [(root.join("b.rs"), "parse here\n".to_string())].into_iter().collect();
        assert_eq!(hits("parse", &open).last().cloned(), Some(("b.rs".into(), 0, 0..5)), "unsaved text counts");
    }

    #[test]
    fn uncommitted_changes_come_from_git() {
        let root = crate::scratch_dir("diff");
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
        assert_eq!(uncommitted(&root, 1000), None, "not a repository");
        git(&["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "a"]);
        assert_eq!(uncommitted(&root, 1000), None, "nothing changed");
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("b.txt"), "new\n").unwrap();
        let diff = uncommitted(&root, 1000).unwrap();
        assert!(diff.contains("-one\n+two") && diff.contains("New files, not yet in git:\nb.txt"), "{diff}");
        assert!(uncommitted(&root, 10).unwrap().ends_with("(cut off)"));
    }

    #[test]
    fn lists_folders_first_and_marks_ignored() {
        let dir = crate::scratch_dir("tree");
        for d in ["src", "target", ".git"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["b.rs", "A.md", ".gitignore"] {
            std::fs::write(dir.join(f), "target/\n").unwrap();
        }
        let names: Vec<(String, bool)> =
            list_dir(&dir).into_iter().map(|(p, _, ignored)| (p.file_name().unwrap().to_string_lossy().into_owned(), ignored)).collect();
        let names: Vec<(&str, bool)> = names.iter().map(|(n, i)| (n.as_str(), *i)).collect();
        assert_eq!(names, [("src", false), ("target", true), (".gitignore", false), ("A.md", false), ("b.rs", false)]);
        assert_eq!(listing(&dir, 2), ".gitignore\nA.md\n… and 1 more");
        std::fs::write(dir.join("target/junk"), "").unwrap();
        std::fs::write(dir.join(".gitignore"), "A.md\n").unwrap();
        assert_eq!(files(&dir, &dir), [".gitignore", "A.md", "b.rs"], "what git ignores is still listed; target isn't");
    }
}
