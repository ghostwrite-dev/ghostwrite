//! How a file differs from what git has, line by line, for the marks in the gutter.

use std::path::Path;
use std::process::Stdio;

use similar::TextDiff;

/// What happened at some lines of the file as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    /// Lines went from just before `lines.start`: `lines` is empty.
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub lines: std::ops::Range<usize>,
    pub kind: ChangeKind,
}

/// The file at `path` as of the last commit, if git has it there.
pub fn committed(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let spec = format!("HEAD:./{}", crate::platform::with_slashes(rel));
    let out =
        crate::platform::command("git").arg("-C").arg(root).args(["show", "--no-ext-diff", "--no-textconv", &spec]).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"))
}

/// Where `text` differs from `base`, in the lines of `text`, in order.
pub fn changes(base: &str, text: &str) -> Vec<Change> {
    let diff = TextDiff::from_lines(base, text);
    let mut found = vec![];
    for group in diff.ops() {
        let (old, new) = (group.old_range(), group.new_range());
        match (old.is_empty(), new.is_empty()) {
            (true, true) => {}
            _ if group.tag() == similar::DiffTag::Equal => {}
            (true, false) => found.push(Change { lines: new, kind: ChangeKind::Added }),
            (false, true) => found.push(Change { lines: new, kind: ChangeKind::Removed }),
            (false, false) => found.push(Change { lines: new, kind: ChangeKind::Modified }),
        }
    }
    // `ChangeTag` is what each line of an op is; here only whole ops are told.
    found
}

/// What's marked at `line`, in `changes` (sorted by line).
pub fn at_line(changes: &[Change], line: usize) -> Option<ChangeKind> {
    let ix = changes.partition_point(|c| c.lines.end <= line && !(c.lines.is_empty() && c.lines.start == line));
    let change = changes.get(ix)?;
    (change.lines.contains(&line) || (change.lines.is_empty() && change.lines.start == line)).then_some(change.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_told_as_added_modified_and_removed() {
        let base = "one\ntwo\nthree\nfour\n";
        assert_eq!(changes(base, base), vec![]);
        assert_eq!(changes(base, "one\nnew\ntwo\nthree\nfour\n"), vec![Change { lines: 1..2, kind: ChangeKind::Added }]);
        assert_eq!(changes(base, "one\nTWO\nthree\nfour\n"), vec![Change { lines: 1..2, kind: ChangeKind::Modified }]);
        assert_eq!(changes(base, "one\nthree\nfour\n"), vec![Change { lines: 1..1, kind: ChangeKind::Removed }]);
    }

    #[test]
    fn a_line_finds_its_mark() {
        let found = vec![
            Change { lines: 1..3, kind: ChangeKind::Added },
            Change { lines: 5..5, kind: ChangeKind::Removed },
            Change { lines: 7..8, kind: ChangeKind::Modified },
        ];
        let marks: Vec<_> = (0..9).map(|line| at_line(&found, line)).collect();
        use ChangeKind::*;
        assert_eq!(marks, [None, Some(Added), Some(Added), None, None, Some(Removed), None, Some(Modified), None]);
    }
}
