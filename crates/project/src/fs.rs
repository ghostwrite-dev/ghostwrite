//! Changing the project's files: creating, renaming, and moving to the Trash.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Create `name` inside `dir`: a folder when it ends in `/`, else an empty
/// file. Missing folders on the way are made; an existing entry is an error.
pub fn create(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    check_name(name)?;
    let path = dir.join(name.trim_end_matches('/'));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let made = if name.ends_with('/') {
        std::fs::create_dir_all(&path)
    } else {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop))
    };
    made.map(|()| path).map_err(|e| format!("couldn't create {name}: {e}"))
}

/// Rename `from` to `name` within its folder (or below it, if `name` has `/`).
pub fn rename(from: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_end_matches('/');
    check_name(name)?;
    move_to(from, &from.parent().unwrap_or(from).join(name))
}

/// Move `from` to `to`, making the folders on the way; an existing `to` is an error.
pub fn move_to(from: &Path, to: &Path) -> Result<PathBuf, String> {
    if to == from {
        return Ok(to.to_path_buf());
    }
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    to.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(from, to))
        .map(|()| to.to_path_buf())
        .map_err(|e| format!("couldn't move {}: {e}", from.display()))
}

static DELETE_INSTEAD: AtomicBool = AtomicBool::new(false);

/// From now on, [`trash`] deletes outright instead of filling the real Trash (tests).
pub fn delete_instead_of_trash() {
    DELETE_INSTEAD.store(true, Ordering::Relaxed);
}

/// Move `path` to the Trash.
pub fn trash(path: &Path) -> Result<(), String> {
    let moved = if DELETE_INSTEAD.load(Ordering::Relaxed) || cfg!(test) {
        if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| e.to_string())
    } else {
        ::trash::delete(path).map_err(|e| e.to_string())
    };
    moved.map_err(|e| format!("couldn't move {} to the Trash: {e}", path.display()))
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.split('/').any(|part| part == "..") {
        return Err("give a name without `..`".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_renames_entries() {
        let dir = crate::scratch_dir("ops");
        let file = create(&dir, "src/lib.rs").unwrap();
        assert!(file.is_file());
        assert!(create(&dir, "docs/").unwrap().is_dir());
        assert!(create(&dir, "src/lib.rs").is_err());
        assert!(create(&dir, "../escape").is_err());
        let moved = rename(&file, "main.rs").unwrap();
        assert_eq!(moved, dir.join("src/main.rs"));
        assert!(moved.is_file() && !file.exists());
        assert!(rename(&moved, "../../x").is_err());
        trash(&moved).unwrap();
        assert!(!moved.exists());
    }
}
