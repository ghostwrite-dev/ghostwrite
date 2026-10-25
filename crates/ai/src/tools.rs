//! Claude's read-only view of the project for the API backend: tools to
//! list, read and search files under the project folder. Changes never go
//! through here; they come back as steps the user accepts.

use std::path::Path;

use ghostwrite_project::files::{files, inside};
use regex::Regex;
use serde_json::{Value, json};

use crate::lines;

const MAX_READ: usize = 120_000;
const MAX_MATCHES: usize = 200;

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
            "description": "Read a file in the project, by path relative to the project root, its lines numbered as files are shown to you. Read-only.",
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

/// What a tool call is doing, for the waiting hint (Claude Code's tool names too).
pub fn describe(name: &str, input: &Value) -> String {
    match name {
        "read_file" | "Read" => format!("reading {}", input["path"].as_str().or_else(|| input["file_path"].as_str()).unwrap_or("a file")),
        "search" | "Grep" => format!("searching for {}", input["pattern"].as_str().unwrap_or("…")),
        "Bash" => format!("running {}", input["command"].as_str().unwrap_or("a command")),
        _ => "looking around the project".into(),
    }
}

/// Run a tool; `Err` goes back to Claude as an error result.
pub fn call(root: &Path, name: &str, input: &Value) -> Result<String, String> {
    let path_arg = input.get("path").and_then(Value::as_str);
    let folder = || match path_arg {
        Some(p) => inside(root, p).ok_or("that folder isn't in the project"),
        None => Ok(root.to_path_buf()),
    };
    match name {
        "list_files" => {
            let all = files(root, &folder()?);
            Ok(if all.is_empty() { "(no files)".into() } else { all.join("\n") })
        }
        "read_file" => {
            let path = inside(root, path_arg.ok_or("read_file needs a path")?).ok_or("that file isn't in the project")?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read it: {e}"))?;
            let cut = text.len() > MAX_READ;
            let text = if cut { &text[..text[..text.floor_char_boundary(MAX_READ)].rfind('\n').map_or(0, |i| i + 1)] } else { &text };
            Ok(format!("{}{}", lines::numbered(text, 0, None), if cut { "… (cut off)" } else { "" }))
        }
        "search" => {
            let pattern = Regex::new(input["pattern"].as_str().ok_or("search needs a pattern")?).map_err(|e| e.to_string())?;
            let mut hits = vec![];
            'files: for rel in files(root, &folder()?) {
                let Ok(text) = std::fs::read_to_string(root.join(&rel)) else { continue };
                for (i, line) in text.lines().enumerate().filter(|(_, l)| pattern.is_match(l)) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    if hits.len() >= MAX_MATCHES {
                        break 'files;
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
    use super::call;
    use serde_json::json;

    #[test]
    fn tools_read_inside_the_project_only() {
        let root = ghostwrite_project::scratch_dir("tools");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn one() {}\npub fn two() {}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/junk"), "x").unwrap();
        assert_eq!(call(&root, "list_files", &json!({})).unwrap(), ".gitignore\nsrc/lib.rs");
        assert_eq!(call(&root, "read_file", &json!({ "path": "src/lib.rs" })).unwrap(), "1│pub fn one() {}\n2│pub fn two() {}\n");
        assert_eq!(call(&root, "search", &json!({ "pattern": "fn two" })).unwrap(), "src/lib.rs:2: pub fn two() {}");
        assert!(call(&root, "read_file", &json!({ "path": "../../etc/passwd" })).is_err(), "nothing outside the project");
        assert!(call(&root, "read_file", &json!({ "path": "/etc/passwd" })).is_err());
    }
}
