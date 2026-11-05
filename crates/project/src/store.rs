//! What Ghostwrite remembers between launches, in its Application Support
//! folder. Here are the recent projects and each project's open files; the
//! rest is kept by what it belongs to, through [`read`] and [`write`] (a
//! project's own under [`project_dir`]): the editor's `:` history and pane
//! sizes, the model's notes and log.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

static MAX_RECENT: OnceLock<usize> = OnceLock::new();

static REDIRECT: OnceLock<PathBuf> = OnceLock::new();

/// How many recent projects to keep (20 unless set), once, at launch.
pub fn set_max_recent(n: usize) {
    MAX_RECENT.set(n).ok();
}

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
    crate::platform::data_dir()
}

fn path_of(name: &str) -> Option<PathBuf> {
    Some(support_dir()?.join(name))
}

/// What's kept as `name` (a path in the folder), if anything.
pub fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(path_of(name)?).ok()
}

/// Keep `text` as `name`, never half-written.
pub fn write(name: &str, text: &str) {
    let Some(path) = path_of(name) else { return };
    let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| crate::fs::write_atomic(&path, text.as_bytes()));
    if let Err(e) = saved {
        eprintln!("couldn't save {}: {e}", path.display());
    }
}

/// Stop keeping `name`.
pub fn remove(name: &str) {
    if let Some(path) = path_of(name) {
        std::fs::remove_file(path).ok();
    }
}

/// The files kept in the folder `dir`, by name in the folder.
pub fn listed(dir: &str) -> Vec<String> {
    let Some(entries) = path_of(dir).and_then(|path| std::fs::read_dir(path).ok()) else { return vec![] };
    let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|name| !name.starts_with('.')).collect();
    names.sort();
    names
}

/// What's kept as `name`, as lines.
pub fn lines(name: &str) -> Vec<String> {
    read(name).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Keep `lines` as `name`, the last `max` of them.
pub fn write_lines(name: &str, lines: &[String], max: usize) {
    let lines = &lines[lines.len().saturating_sub(max)..];
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
    projects.truncate(*MAX_RECENT.get_or_init(|| 20));
    write_lines("projects", &projects.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(), usize::MAX);
}

pub fn forget_project(project: &Path) {
    let projects: Vec<String> = recent_projects().into_iter().filter(|p| p != project).map(|p| p.display().to_string()).collect();
    write_lines("projects", &projects, usize::MAX);
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

// What's kept for one project: in its own folder, rather than in the
// project, so nothing lands in the user's repository.

/// The folder kept for the project at `root`: `project-state/<folder
/// name>-<hash of its path>`, stable across launches.
pub fn project_dir(root: &Path) -> String {
    // FNV-1a: std's hasher isn't guaranteed stable between Rust releases.
    let hash = root.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("project-state/{name}-{hash:016x}")
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
    fn keeps_lines_and_a_folder_per_project() {
        let name = format!("{}/log", project_dir(Path::new("/tmp/ghostwrite-store-test")));
        write_lines(&name, &["a".into(), "b".into(), "c".into()], 2);
        assert_eq!(lines(&name), ["b", "c"]);
        assert_ne!(project_dir(Path::new("/a/x")), project_dir(Path::new("/b/x")), "same name, other folder");
    }
}
