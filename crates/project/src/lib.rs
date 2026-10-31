//! The project on disk: what's in it ([`files`]), changing it ([`fs`]),
//! running commands in it ([`shell`]) and the places their output points at ([`stops`]),
//! the user's own shell ([`terminal`]),
//! the file tree ([`tree`]), noticing changes made by other programs
//! ([`watch`]), and what Ghostwrite remembers between launches ([`store`]).

pub mod files;
pub mod fs;
pub mod shell;
pub mod stops;
pub mod store;
pub mod terminal;
pub mod tree;
pub mod watch;

use std::path::{Path, PathBuf};

/// The project a path belongs to: the enclosing git repository, else the
/// folder itself, else the file's folder.
pub fn root_for(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = if path.is_dir() { path.clone() } else { path.parent().map_or(path.clone(), Path::to_path_buf) };
    dir.ancestors().find(|d| d.join(".git").exists()).map_or(dir.clone(), Path::to_path_buf)
}

/// `~/` is home; anything relative is from the project root. `.` and `..`
/// are taken out.
pub fn resolve(arg: &str, root: &Path) -> PathBuf {
    let path = match arg.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(arg),
    };
    clean(&if path.is_absolute() { path } else { root.join(path) })
}

/// `path` made absolute without `.` and `..` (by its words, not following symlinks).
pub fn clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `a` and `b` are the same file, however they're spelled
/// (through symlinks, `..`, or a canonical path a language server sends).
pub fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    // A file that isn't there yet: its folder, followed, and its name.
    let real =
        |p: &Path| std::fs::canonicalize(p).ok().or_else(|| Some(std::fs::canonicalize(p.parent()?).ok()?.join(p.file_name()?))).unwrap_or_else(|| clean(p));
    real(a) == real(b)
}

/// `path` for people: relative to `root` if it's inside, `~/…` if it's in home.
pub fn display(path: &Path, root: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.display().to_string();
    }
    match std::env::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The scratch folder a test works in, made fresh.
#[doc(hidden)]
pub fn scratch_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("ghostwrite-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("the temp folder is writable");
    dir
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn paths_are_the_same_file_however_spelled() {
        let dir = scratch_dir("same");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("src"), dir.join("link")).unwrap();
        assert_eq!(resolve("src/../src/./a.rs", &dir), dir.join("src/a.rs"));
        assert!(same_file(&dir.join("link/a.rs"), &dir.join("src/a.rs")));
        assert!(same_file(&dir.join("link/new.rs"), &dir.join("src/new.rs")), "not there yet");
        assert!(!same_file(&dir.join("src/a.rs"), &dir.join("src/b.rs")));
    }
}
