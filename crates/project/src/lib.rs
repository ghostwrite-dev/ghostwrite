//! The project on disk: what's in it ([`files`]), changing it ([`fs`]),
//! running commands in it ([`shell`]) and the places their output points at ([`stops`]),
//! the file tree ([`tree`]), noticing changes made by other programs
//! ([`watch`]), and what Ghostwrite remembers between launches ([`store`]).

pub mod files;
pub mod fs;
pub mod shell;
pub mod stops;
pub mod store;
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

/// `~/` is home; anything relative is from the project root.
pub fn resolve(arg: &str, root: &Path) -> PathBuf {
    let path = match arg.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(arg),
    };
    if path.is_absolute() { path } else { root.join(path) }
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
