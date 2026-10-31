//! The file tree as a model: which folders are open, the rows that makes,
//! where the cursor is, and what Claude proposes doing to a file. The app
//! drives it from tree mode and draws it; nothing here knows about keys or
//! pixels.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::files::list_dir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    /// Left out by `.gitignore` (or in a folder that is): shown, but dimmed.
    pub ignored: bool,
}

/// A story step about a file or folder, shown in the tree where it applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// A new file, or a folder if the path ends in `/`.
    Create(PathBuf),
    Remove(PathBuf),
    /// This file or folder, to the new path (relative to the project).
    Rename(PathBuf, String),
}

impl Proposal {
    /// The existing row it's about (none for a creation).
    pub fn row_path(&self) -> Option<&Path> {
        match self {
            Proposal::Create(_) => None,
            Proposal::Remove(p) | Proposal::Rename(p, _) => Some(p),
        }
    }
}

/// Where a proposed new entry shows: before row `before` (or at the end), at
/// `depth`, reading `name` (relative to its nearest existing folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ghost {
    pub before: usize,
    pub depth: usize,
    pub name: String,
}

/// The folders to list: the root and the open ones.
#[derive(Debug, Clone)]
pub struct Listing {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
}

impl Listing {
    /// The rows, read from disk.
    pub fn list(&self) -> Vec<Row> {
        fn walk(dir: &Path, depth: usize, in_ignored: bool, expanded: &HashSet<PathBuf>, rows: &mut Vec<Row>) {
            for (path, is_dir, ignored) in list_dir(dir) {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let (open, ignored) = (is_dir && expanded.contains(&path), in_ignored || ignored);
                rows.push(Row { path: path.clone(), name, depth, is_dir, ignored });
                if open {
                    walk(&path, depth + 1, ignored, expanded, rows);
                }
            }
        }
        let mut rows = vec![];
        walk(&self.root, 0, false, &self.expanded, &mut rows);
        rows
    }
}

#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    cursor: usize,
    /// The file being edited, highlighted.
    active: Option<PathBuf>,
    proposal: Option<Proposal>,
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let mut tree = Self { root, expanded: HashSet::new(), rows: vec![], cursor: 0, active: None, proposal: None };
        tree.refresh();
        tree
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    pub fn active(&self) -> Option<&Path> {
        self.active.as_deref()
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn proposal(&self) -> Option<&Proposal> {
        self.proposal.as_ref()
    }

    /// Re-list from disk, keeping the cursor on the same path if it's still there.
    pub fn refresh(&mut self) {
        let rows = self.listing().list();
        self.set_rows(rows);
    }

    /// What re-listing reads, to do it away from the UI; [`Tree::listed`] takes the result.
    pub fn listing(&self) -> Listing {
        Listing { root: self.root.clone(), expanded: self.expanded.clone() }
    }

    /// The rows `listing` read, if the same folders are still open (else re-list).
    pub fn listed(&mut self, listing: &Listing, rows: Vec<Row>) {
        if listing.expanded == self.expanded { self.set_rows(rows) } else { self.refresh() }
    }

    fn set_rows(&mut self, rows: Vec<Row>) {
        let at = self.selected().map(|r| r.path.clone());
        self.rows = rows;
        self.cursor = at.and_then(|p| self.index_of(&p)).unwrap_or(self.cursor).min(self.rows.len().saturating_sub(1));
    }

    /// Whether any of `paths` is (or would be) a row: its folder is the root or open.
    pub fn shows_any(&self, paths: &[PathBuf]) -> bool {
        paths.iter().filter_map(|p| p.parent()).any(|dir| dir == self.root || self.expanded.contains(dir))
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|r| r.path == path)
    }

    pub fn move_to(&mut self, ix: usize) {
        self.cursor = ix.min(self.rows.len().saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize) {
        self.move_to(self.cursor.saturating_add_signed(delta));
    }

    /// Open or close the folder at `ix` (putting the cursor there).
    pub fn toggle(&mut self, ix: usize) {
        let Some(row) = self.rows.get(ix) else { return };
        self.cursor = ix;
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
            }
            self.refresh();
        }
    }

    /// `l`: open a closed folder, step into an open one. A file is returned to preview.
    pub fn expand(&mut self) -> Option<PathBuf> {
        let row = self.selected()?.clone();
        match (row.is_dir, self.expanded.contains(&row.path)) {
            (true, true) => self.move_by(1),
            (true, false) => self.toggle(self.cursor),
            (false, _) => return Some(row.path),
        }
        None
    }

    /// `h`: close the folder, or move up to the folder holding this row.
    pub fn collapse(&mut self) {
        let Some(row) = self.selected().cloned() else { return };
        if row.is_dir && self.expanded.remove(&row.path) {
            self.refresh();
        } else if let Some(parent) = row.path.parent().and_then(|p| self.index_of(p)) {
            self.cursor = parent;
        }
    }

    /// The folder new entries go in: the one under the cursor, or the cursor's file's folder.
    pub fn target_dir(&self) -> PathBuf {
        match self.selected() {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row.path.parent().map_or(self.root.clone(), Path::to_path_buf),
            None => self.root.clone(),
        }
    }

    /// Open the folders above `path` (those that exist), re-list, and put
    /// the cursor on it if it's a row.
    pub fn reveal(&mut self, path: &Path) {
        for dir in path.ancestors().skip(1).take_while(|d| d.starts_with(&self.root) && *d != self.root) {
            if dir.is_dir() {
                self.expanded.insert(dir.to_path_buf());
            }
        }
        self.refresh();
        if let Some(ix) = self.index_of(path) {
            self.cursor = ix;
        }
    }

    /// A folder (or file) moved: keep it open under its new name.
    pub fn renamed(&mut self, from: &Path, to: &Path) {
        if self.expanded.remove(from) {
            self.expanded.insert(to.to_path_buf());
        }
        self.reveal(to);
    }

    /// Highlight the file being edited, opening the folders above it.
    pub fn set_active(&mut self, path: Option<PathBuf>) {
        if path == self.active {
            return;
        }
        if let Some(p) = path.as_ref().filter(|p| p.starts_with(&self.root)) {
            self.reveal(p);
        }
        self.active = path;
    }

    /// Show (or clear) a proposal: its row revealed, or the folders a new
    /// entry would go in opened.
    pub fn propose(&mut self, proposal: Option<Proposal>) {
        match &proposal {
            Some(Proposal::Create(p)) => self.reveal(p),
            Some(Proposal::Remove(p) | Proposal::Rename(p, _)) => self.reveal(p),
            None => {}
        }
        self.proposal = proposal;
    }

    /// Where a proposed new entry shows, if one is proposed.
    pub fn ghost(&self) -> Option<Ghost> {
        let Some(Proposal::Create(path)) = &self.proposal else { return None };
        let parent = path.ancestors().skip(1).find(|d| d.is_dir())?;
        let rel = path.strip_prefix(parent).ok()?.display().to_string();
        let name = if path.to_string_lossy().ends_with('/') { format!("{rel}/") } else { rel };
        if parent == self.root {
            return Some(Ghost { before: 0, depth: 0, name });
        }
        let ix = self.index_of(parent)?;
        Some(Ghost { before: ix + 1, depth: self.rows[ix].depth + 1, name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tree: &Tree) -> Vec<String> {
        tree.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.name)).collect()
    }

    #[test]
    fn expands_collapses_and_follows_the_active_file() {
        let root = crate::scratch_dir("model");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/deep/x.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        let mut tree = Tree::new(root.clone());
        assert_eq!(names(&tree), ["src", "README.md"]);
        assert_eq!(tree.expand(), None);
        assert_eq!(names(&tree), ["src", "  deep", "README.md"]);
        tree.set_active(Some(root.join("src/deep/x.rs")));
        assert_eq!(names(&tree), ["src", "  deep", "    x.rs", "README.md"]);
        assert_eq!(tree.cursor(), 2);
        assert_eq!(tree.expand(), Some(root.join("src/deep/x.rs")), "a file is previewed");
        tree.collapse();
        assert_eq!(tree.cursor(), 1, "h on a file climbs to its folder");
        tree.collapse();
        assert_eq!(names(&tree), ["src", "  deep", "README.md"]);
    }

    #[test]
    fn a_proposed_file_shows_under_its_folder() {
        let root = crate::scratch_dir("ghost");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let mut tree = Tree::new(root.clone());
        tree.propose(Some(Proposal::Create(root.join("src/new/util.rs"))));
        assert_eq!(tree.ghost(), Some(Ghost { before: 1, depth: 1, name: "new/util.rs".into() }));
        tree.propose(None);
        assert_eq!(tree.ghost(), None);
    }
}
