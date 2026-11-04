//! The project on disk: what's in it ([`files`]), changing it ([`fs`]),
//! the file tree ([`tree`]), noticing changes made by other programs
//! ([`watch`]), and what Ghostwrite remembers between launches ([`store`]).

pub mod files;
pub mod fs;
pub mod git;
pub mod merge;
pub mod platform;
pub mod store;
pub mod tree;
pub mod watch;

use std::path::{Path, PathBuf};

/// Run `work`, and if it panics, say so instead of taking the app and any
/// unsaved work with it: on the system's worker threads nothing else would
/// catch it, and the process would abort.
pub fn contained<T>(what: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
        let why = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
        format!("{what} failed unexpectedly: {}", why.unwrap_or_else(|| "no reason given".into()))
    })
}

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

/// Whether `path` is in the project at `root`, below it: by its words (not
/// absolute elsewhere, no `..` out, not the root itself), and on disk (its
/// nearest existing folder doesn't lead out through a symlink). What a
/// reply names, it may only touch if it is.
pub fn within(root: &Path, path: &Path) -> bool {
    let (root, path) = (clean(root), clean(&root.join(path)));
    if path == root || !path.starts_with(&root) {
        return false;
    }
    let Ok(real_root) = platform::real(&root) else { return false };
    let existing = path.ancestors().find(|p| p.exists());
    existing.and_then(|p| platform::real(p).ok()).is_some_and(|real| real.starts_with(&real_root))
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
        return platform::with_slashes(rel);
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn within_the_project_means_below_it() {
        let root = scratch_dir("within");
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert!(within(&root, Path::new("src/new.rs")), "a new file in it");
        assert!(within(&root, Path::new("./src")));
        assert!(!within(&root, Path::new(".")), "not the project itself");
        assert!(!within(&root, Path::new("src/../..")), "not out by `..`");
        assert!(!within(&root, Path::new("/etc/hosts")), "not elsewhere");
        std::os::unix::fs::symlink("/tmp", root.join("out")).unwrap();
        assert!(!within(&root, Path::new("out/x")), "not out through a symlink");
    }

    #[test]
    fn a_panic_in_contained_work_is_only_an_error() {
        assert_eq!(contained("adding", || 1 + 1), Ok(2));
        let text = "→";
        let why = contained("slicing", || text[1..].len()).unwrap_err();
        assert!(why.starts_with("slicing failed unexpectedly: "), "{why}");
    }
}
