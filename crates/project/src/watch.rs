//! Noticing when something else (a shell, git, Claude Code, another editor)
//! changes the project's files, so the tree and open files can catch up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

/// How long the files must be quiet before a batch of changes is reported.
const SETTLE: Duration = Duration::from_millis(150);

/// Watches a project folder while it lives.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// Watch everything under `root`, calling `changed` (on the watcher's
/// thread) with the paths that changed, spelled under `root` as given.
/// Changes inside `.git` are left out.
pub fn watch(root: &Path, changed: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify_debouncer_mini::notify::Result<Watcher> {
    // The platform reports real paths (`/private/var/…` for `/var/…`).
    let real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let spelled = root.to_path_buf();
    let mut debouncer = new_debouncer(SETTLE, move |result: DebounceEventResult| {
        let Ok(events) = result else { return };
        let mut paths: Vec<PathBuf> = events
            .into_iter()
            .filter_map(|e| {
                let rel = e.path.strip_prefix(&real).or_else(|_| e.path.strip_prefix(&spelled)).ok()?;
                (!rel.components().any(|c| c.as_os_str() == ".git")).then(|| spelled.join(rel))
            })
            .collect();
        paths.sort();
        paths.dedup();
        if !paths.is_empty() {
            changed(paths);
        }
    })?;
    debouncer.watcher().watch(root, RecursiveMode::Recursive)?;
    Ok(Watcher { _debouncer: debouncer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_under_the_root_but_not_in_git() {
        let root = crate::scratch_dir("watch");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _watcher = watch(&root, move |paths| tx.send(paths).unwrap()).unwrap();
        // FSEvents may deliver the folder's creation late; let it settle.
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join(".git/index"), "x").unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let mut seen = vec![];
        while let Ok(paths) = rx.recv_timeout(Duration::from_secs(5)) {
            seen.extend(paths);
            if seen.contains(&root.join("a.txt")) {
                break;
            }
        }
        assert!(seen.contains(&root.join("a.txt")), "{seen:?}");
        assert!(seen.iter().all(|p| !p.starts_with(root.join(".git"))), "{seen:?}");
    }
}
