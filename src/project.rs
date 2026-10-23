//! Claude's read-only view of the project: a file list for the prompt, and
//! tools to list, read and search files under the project folder. Changes
//! never go through here; they come back as steps the user accepts.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

const MAX_FILES: usize = 400;
const MAX_READ: usize = 120_000;
const MAX_MATCHES: usize = 200;
/// Matches a project search stops at.
pub const MAX_FOUND: usize = 2000;

/// Files under `root` (honouring `.gitignore`), relative, sorted.
pub fn files(root: &Path, under: &Path) -> Vec<String> {
    let mut out: Vec<String> = ignore::WalkBuilder::new(under)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(|p| p.display().to_string()))
        .collect();
    out.sort();
    out
}

/// The file list for the prompt, cut off when it gets long.
pub fn listing(root: &Path) -> String {
    let all = files(root, root);
    let shown = all.iter().take(MAX_FILES).cloned().collect::<Vec<_>>().join("\n");
    if all.len() > MAX_FILES { format!("{shown}\n… and {} more", all.len() - MAX_FILES) } else { shown }
}

/// What `space /` searches for: a regex, smart-case like helix (ignoring
/// case unless there's a capital), or the text as typed if it isn't one.
pub fn search_pattern(typed: &str) -> Result<Regex, String> {
    let flags = if typed.chars().any(char::is_uppercase) { "" } else { "(?i)" };
    Regex::new(&format!("{flags}{typed}"))
        .or_else(|_| Regex::new(&format!("{flags}{}", regex::escape(typed))))
        .map_err(|e| e.to_string())
}

/// A match: the file, its line (zero-based), the match's bytes in the line, and the line.
pub struct Found {
    pub path: PathBuf,
    pub line: usize,
    pub bytes: Range<usize>,
    pub text: String,
}

/// Every match of `pattern` in the project, file by file, line by line.
/// Files open in the editor are searched as they are there (maybe unsaved).
pub fn grep(root: &Path, pattern: &Regex, open: &HashMap<PathBuf, String>) -> Vec<Found> {
    let mut found = vec![];
    for rel in files(root, root) {
        let path = root.join(&rel);
        let Some(text) = open.get(&path).cloned().or_else(|| std::fs::read_to_string(&path).ok()) else { continue };
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

/// `path` inside the project, or `None` if it would leave it.
fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let joined = root.join(path.trim_start_matches('/'));
    let real = joined.canonicalize().ok()?;
    real.starts_with(&root).then_some(real)
}

/// The tools, as the Messages API takes them.
pub fn definitions() -> Value {
    json!([
        {
            "name": "list_files",
            "description": "List files in the project (or a folder of it), relative to the project root. Read-only.",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string", "description": "A folder relative to the project root; omit for all." } }, "additionalProperties": false },
            "eager_input_streaming": true,
        },
        {
            "name": "read_file",
            "description": "Read a file in the project, by path relative to the project root. Read-only.",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"], "additionalProperties": false },
            "eager_input_streaming": true,
        },
        {
            "name": "search",
            "description": "Search project files for a regular expression; returns path:line: text matches. Read-only.",
            "input_schema": { "type": "object", "properties": { "pattern": { "type": "string" }, "path": { "type": "string", "description": "Limit to this folder." } }, "required": ["pattern"], "additionalProperties": false },
            "eager_input_streaming": true,
        },
    ])
}

/// What a tool call is doing, for the waiting hint.
pub fn describe(name: &str, input: &Value) -> String {
    match name {
        "read_file" | "Read" => format!("reading {}", input["path"].as_str().or_else(|| input["file_path"].as_str()).unwrap_or("a file")),
        "search" | "Grep" => format!("searching for {}", input["pattern"].as_str().unwrap_or("…")),
        _ => "looking around the project".into(),
    }
}

/// Run a tool; `Err` is reported back to Claude as an error result.
pub fn call(root: &Path, name: &str, input: &Value) -> Result<String, String> {
    let path_arg = input.get("path").and_then(Value::as_str);
    match name {
        "list_files" => {
            let under = match path_arg {
                Some(p) => inside(root, p).ok_or("that folder isn't in the project")?,
                None => root.to_path_buf(),
            };
            let all = files(root, &under);
            Ok(if all.is_empty() { "(no files)".into() } else { all.join("\n") })
        }
        "read_file" => {
            let path = inside(root, path_arg.ok_or("read_file needs a path")?).ok_or("that file isn't in the project")?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read it: {e}"))?;
            Ok(if text.len() > MAX_READ { format!("{}\n… (cut off)", &text[..text.floor_char_boundary(MAX_READ)]) } else { text })
        }
        "search" => {
            let pattern = Regex::new(input["pattern"].as_str().ok_or("search needs a pattern")?).map_err(|e| e.to_string())?;
            let under = match path_arg {
                Some(p) => inside(root, p).ok_or("that folder isn't in the project")?,
                None => root.to_path_buf(),
            };
            let mut hits = vec![];
            'files: for rel in files(root, &under) {
                let Ok(text) = std::fs::read_to_string(root.join(&rel)) else { continue };
                for (i, line) in text.lines().enumerate() {
                    if pattern.is_match(line) {
                        hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                        if hits.len() >= MAX_MATCHES {
                            break 'files;
                        }
                    }
                }
            }
            Ok(if hits.is_empty() { "(no matches)".into() } else { hits.join("\n") })
        }
        other => Err(format!("no tool named {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{call, grep, listing, search_pattern};
    use serde_json::json;

    #[test]
    fn search_is_smart_case_and_prefers_open_text() {
        let root = std::env::temp_dir().join(format!("ghostwrite-grep-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn Parse {}\nlet x = parse(y);\n").unwrap();
        std::fs::write(root.join("b.rs"), "nothing\n").unwrap();
        let hits = |p: &str, open: &std::collections::HashMap<_, _>| {
            grep(&root, &search_pattern(p).unwrap(), open).iter().map(|f| (f.path.file_name().unwrap().to_string_lossy().to_string(), f.line, f.bytes.clone())).collect::<Vec<_>>()
        };
        let none = Default::default();
        assert_eq!(hits("parse", &none), [("a.rs".into(), 0, 3..8), ("a.rs".into(), 1, 8..13)], "lowercase ignores case");
        assert_eq!(hits("Parse", &none), [("a.rs".into(), 0, 3..8)], "a capital makes it exact");
        assert_eq!(hits("parse(", &none), [("a.rs".into(), 1, 8..14)], "not a regex: taken as typed");
        let open = [(root.join("b.rs"), "parse here\n".to_string())].into_iter().collect();
        assert_eq!(hits("parse", &open).last().cloned(), Some(("b.rs".into(), 0, 0..5)), "unsaved text counts");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tools_read_inside_the_project_only() {
        let root = std::env::temp_dir().join(format!("ghostwrite-proj-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn one() {}\npub fn two() {}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/junk"), "x").unwrap();
        assert_eq!(listing(&root), ".gitignore\nsrc/lib.rs");
        assert_eq!(call(&root, "read_file", &json!({ "path": "src/lib.rs" })).unwrap(), "pub fn one() {}\npub fn two() {}\n");
        assert_eq!(call(&root, "search", &json!({ "pattern": "fn two" })).unwrap(), "src/lib.rs:2: pub fn two() {}");
        assert!(call(&root, "read_file", &json!({ "path": "../../etc/passwd" })).is_err(), "nothing outside the project");
        assert!(call(&root, "read_file", &json!({ "path": "/etc/passwd" })).is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
