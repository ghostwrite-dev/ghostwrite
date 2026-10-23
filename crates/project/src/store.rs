//! What Ghostwrite remembers between launches, in its Application Support
//! folder: recent projects, each project's open files, the `:` history, the
//! file tree's width, and the last exchange with Claude.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

const MAX_RECENT: usize = 20;
const MAX_HISTORY: usize = 200;

static REDIRECT: OnceLock<PathBuf> = OnceLock::new();

/// Keep state in a scratch folder for the rest of the process (tests).
pub fn redirect_for_tests() {
    REDIRECT.get_or_init(|| std::env::temp_dir().join(format!("ghostwrite-support-{}", std::process::id())));
}

/// Where state is kept: Application Support, or `GHOSTWRITE_SUPPORT_DIR`.
pub fn support_dir() -> Option<PathBuf> {
    if cfg!(test) {
        redirect_for_tests();
    }
    if let Some(dir) = REDIRECT.get() {
        return Some(dir.clone());
    }
    if let Some(dir) = std::env::var_os("GHOSTWRITE_SUPPORT_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(std::env::home_dir()?.join("Library/Application Support/Ghostwrite"))
}

fn path_of(name: &str) -> Option<PathBuf> {
    Some(support_dir()?.join(name))
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(path_of(name)?).ok()
}

fn write(name: &str, text: &str) {
    let Some(path) = path_of(name) else { return };
    let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text));
    if let Err(e) = saved {
        eprintln!("couldn't save {}: {e}", path.display());
    }
}

fn lines(name: &str) -> Vec<String> {
    read(name).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn write_lines(name: &str, lines: &[String]) {
    write(name, &lines.iter().map(|l| format!("{l}\n")).collect::<String>());
}

// Recent projects, most recently opened first.

pub fn recent_projects() -> Vec<PathBuf> {
    lines("projects").into_iter().map(PathBuf::from).collect()
}

/// Move `project` to the top of the recent list.
pub fn touch_project(project: &Path) {
    let mut projects = recent_projects();
    projects.retain(|p| p != project);
    projects.insert(0, project.to_path_buf());
    projects.truncate(MAX_RECENT);
    write_lines("projects", &projects.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
}

pub fn forget_project(project: &Path) {
    let projects: Vec<String> = recent_projects().into_iter().filter(|p| p != project).map(|p| p.display().to_string()).collect();
    write_lines("projects", &projects);
}

// Sessions: a project's open files.

/// A project's open files, in tab order, with their cursors (line, column),
/// and which tab was active.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Session {
    pub files: Vec<(PathBuf, usize, usize)>,
    pub active: usize,
}

pub fn load_session(root: &Path) -> Option<Session> {
    let all: Value = serde_json::from_str(&read("sessions.json")?).ok()?;
    let entry = all.get(root.to_string_lossy().as_ref())?;
    let files = entry["files"]
        .as_array()?
        .iter()
        .filter_map(|f| Some((PathBuf::from(f["path"].as_str()?), f["line"].as_u64()? as usize, f["column"].as_u64()? as usize)))
        .collect();
    Some(Session { files, active: entry["active"].as_u64().unwrap_or(0) as usize })
}

pub fn save_session(root: &Path, session: &Session) {
    let mut all: Value = read("sessions.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    let files: Vec<Value> = session.files.iter().map(|(p, line, column)| json!({ "path": p, "line": line, "column": column })).collect();
    all[root.to_string_lossy().as_ref()] = json!({ "files": files, "active": session.active });
    write("sessions.json", &all.to_string());
}

// The `:` command history, oldest first.

pub fn command_history() -> Vec<String> {
    lines("command-history")
}

pub fn save_command_history(history: &[String]) {
    write_lines("command-history", &history[history.len().saturating_sub(MAX_HISTORY)..]);
}

// The file tree's width.

pub fn tree_width() -> Option<f32> {
    read("tree-width")?.trim().parse().ok()
}

pub fn save_tree_width(width: f32) {
    write("tree-width", &width.to_string());
}

/// The last request to Claude and its replies, kept for when a step won't place.
pub fn save_transcript(text: &str) {
    write("last-reply.txt", text);
}

// Claude's context for a project: its own condensed notes, and a log of
// recent interactions. Kept here rather than in the project, so nothing
// lands in the user's repository.

/// `project-state/<folder name>-<hash of its path>`, stable across launches.
fn project_key(root: &Path) -> String {
    // FNV-1a: std's hasher isn't guaranteed stable between Rust releases.
    let hash = root.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("project-state/{name}-{hash:016x}")
}

/// Claude's notes about the project and the user, as it last condensed them.
pub fn load_memory(root: &Path) -> String {
    read(&format!("{}/memory.md", project_key(root))).unwrap_or_default()
}

pub fn save_memory(root: &Path, notes: &str) {
    write(&format!("{}/memory.md", project_key(root)), notes);
}

/// Add an interaction to the project's log.
pub fn log_interaction(root: &Path, entry: &Value) {
    let name = format!("{}/interactions.jsonl", project_key(root));
    let mut entries: Vec<String> = lines(&name);
    entries.push(entry.to_string());
    // The log is for recent context; older knowledge lives in the notes.
    let keep = entries.len().saturating_sub(MAX_HISTORY);
    write_lines(&name, &entries[keep..]);
}

/// The last `n` interactions, oldest first.
pub fn recent_interactions(root: &Path, n: usize) -> Vec<Value> {
    let entries = lines(&format!("{}/interactions.jsonl", project_key(root)));
    entries[entries.len().saturating_sub(n)..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_projects_and_sessions() {
        let a = PathBuf::from("/tmp/ghostwrite-a");
        let b = PathBuf::from("/tmp/ghostwrite-b");
        touch_project(&a);
        touch_project(&b);
        touch_project(&a);
        assert_eq!(recent_projects()[..2], [a.clone(), b.clone()]);
        forget_project(&a);
        assert_eq!(recent_projects()[0], b);

        let session = Session { files: vec![(a.join("x.rs"), 3, 4)], active: 0 };
        save_session(&a, &session);
        assert_eq!(load_session(&a), Some(session));
        assert_eq!(load_session(&b.join("nope")), None);
    }

    #[test]
    fn keeps_claudes_notes_and_log_per_project() {
        let root = PathBuf::from("/tmp/ghostwrite-memory-test");
        save_memory(&root, "Prefers small steps.");
        assert_eq!(load_memory(&root), "Prefers small steps.");
        assert_eq!(load_memory(Path::new("/tmp/elsewhere")), "");
        for i in 0..3 {
            log_interaction(&root, &json!({ "request": i }));
        }
        let last: Vec<Value> = recent_interactions(&root, 2);
        assert_eq!(last, [json!({ "request": 1 }), json!({ "request": 2 })]);
    }
}
