//! What's a link in the terminal's text: a web address, or a file that's there,
//! named as compilers print them (`src/main.rs:12:5`).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTo {
    /// A web address, for the browser.
    Web(String),
    /// A file, at a line and column (from zero), for the editor.
    File(PathBuf, usize, usize),
}

/// The link at column `col` of a row whose cells are `row` (column, char): a web
/// address, or a file that's there (`src/main.rs:12:5`, as compilers print them).
pub fn link_in(row: &[(usize, char)], col: usize, root: &Path) -> Option<(std::ops::Range<usize>, LinkTo)> {
    let width = row.iter().map(|(col, _)| col + 1).max()?;
    let mut chars = vec![' '; width];
    for &(at, c) in row {
        chars[at] = c;
    }
    if chars.get(col).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    // The word under the mouse: what's between spaces.
    let start = chars[..col].iter().rposition(|c| c.is_whitespace()).map_or(0, |at| at + 1);
    let end = chars[col..].iter().position(|c| c.is_whitespace()).map_or(width, |at| col + at);
    let word: String = chars[start..end].iter().collect();
    if let Some(from) = ["https://", "http://", "file://"].iter().filter_map(|scheme| word.find(scheme)).min() {
        let address = trim_address(&word[from..]);
        let (first, last) = (start + word[..from].chars().count(), start + word[..from].chars().count() + address.chars().count());
        return (first..last).contains(&col).then(|| (first..last, LinkTo::Web(address.to_string())));
    }
    // A path: the chars paths are made of, around the mouse.
    let first = chars[..=col].iter().rposition(|c| !in_path(*c)).map_or(0, |at| at + 1);
    let last = chars[col..].iter().position(|c| !in_path(*c)).map_or(width, |at| col + at);
    if first > col {
        return None;
    }
    let named: String = chars[first..last].iter().collect();
    let (path, line, column) = located(&named, root, None)?;
    // What's underlined is the path and its numbers, not the punctuation after.
    let shown = named.trim_end_matches([':', '.']).chars().count();
    Some((first..first + shown, LinkTo::File(path, line, column)))
}

/// A web address as printed in text: punctuation after it isn't part of it, nor is a
/// closing bracket it doesn't open (`(see https://a.b/c)`).
fn trim_address(address: &str) -> &str {
    let mut address = address.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"', '>']);
    for (open, close) in [('(', ')'), ('[', ']')] {
        while address.ends_with(close) && address.matches(close).count() > address.matches(open).count() {
            address = &address[..address.len() - 1];
        }
    }
    address.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"'])
}

/// The chars a path (and the line and column after it) is made of.
fn in_path(c: char) -> bool {
    c.is_alphanumeric() || "/._-~+@:\\".contains(c)
}

/// The path under `at`, with its `:line:column` if it has them.
pub fn path_at(text: &str, at: usize) -> String {
    let start = text[..at].char_indices().rev().take_while(|(_, c)| in_path(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !in_path(*c)).map_or(text.len(), |(i, _)| at + i);
    text[start..end].to_string()
}

/// The file `named` is (`path`, `path:line`, `path:line:column`, as compilers
/// write them), if there is one: the line and column from zero.
pub fn located(named: &str, root: &Path, near: Option<&Path>) -> Option<(PathBuf, usize, usize)> {
    let named = named.trim_start_matches(|c: char| !in_path(c)).trim_end_matches(|c: char| !in_path(c) || c == ':' || c == '.');
    let mut parts = named.split(':');
    let path = parts.next().filter(|p| !p.is_empty())?;
    let mut number = || parts.next().and_then(|n| n.parse::<usize>().ok()).map(|n| n.saturating_sub(1));
    let (line, column) = (number().unwrap_or(0), number().unwrap_or(0));
    let path = match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(path),
    };
    let candidates = if path.is_absolute() { vec![path] } else { [Some(root), near].into_iter().flatten().map(|dir| dir.join(&path)).collect() };
    candidates.into_iter().find(|path| path.is_file()).map(|path| (path, line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_read_with_its_line_and_column() {
        let text = "error: oops\n --> src/main.rs:12:5\n";
        let at = text.find("main").unwrap();
        assert_eq!(path_at(text, at), "src/main.rs:12:5");
        let root = std::env::temp_dir().join(format!("ghostwrite-gf-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let root = root.as_path();
        assert_eq!(located("src/main.rs:12:5", root, None), Some((root.join("src/main.rs"), 11, 4)));
        assert_eq!(located("(src/main.rs:3).", root, None), Some((root.join("src/main.rs"), 2, 0)), "what's around it goes");
        assert_eq!(located("main.rs", root, Some(&root.join("src"))), Some((root.join("src/main.rs"), 0, 0)), "or by the file");
        assert_eq!(located("nowhere.rs:1", root, None), None);
    }

    #[test]
    fn links_on_the_terminal_are_web_addresses_and_files_that_are_there() {
        let root = std::env::temp_dir().join(format!("ghostwrite-terminal-links-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let row = |text: &str| text.chars().enumerate().collect::<Vec<_>>();
        let line = row("see (https://example.com/a_(b)), then");
        let (cols, to) = link_in(&line, 10, &root).unwrap();
        assert_eq!(to, LinkTo::Web("https://example.com/a_(b)".into()), "the brackets it opens, not the one around it");
        assert_eq!(cols, 5..30);
        assert_eq!(link_in(&line, 2, &root), None, "a word that's neither");
        assert_eq!(link_in(&line, 3, &root), None, "a space");
        let line = row("error: --> src/main.rs:1:4: here");
        let (cols, to) = link_in(&line, 14, &root).unwrap();
        assert_eq!(to, LinkTo::File(root.join("src/main.rs"), 0, 3), "where the compiler said");
        assert_eq!(cols, 11..26, "the path and its numbers, not the colon after");
        assert_eq!(link_in(&row("src/nothere.rs"), 3, &root), None, "a file that isn't there isn't one");
    }
}
