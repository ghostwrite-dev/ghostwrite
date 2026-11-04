//! The items of stories the editor gives: a note at each diagnostic or
//! search match, and a step for each line `:replace` changes. Searching, and
//! starting the story, is the app's.

use std::path::Path;

use ghostwrite_ai::Item;
use ghostwrite_ai::protocol::{Note, Step};

/// A note at `line` of `path`, whose text there is `text`.
pub fn note_at(root: &Path, path: &Path, line: usize, text: &str, say: String) -> Item {
    let file = ghostwrite_project::platform::with_slashes(path.strip_prefix(root).unwrap_or(path));
    // No text (a line too long to keep): the note is at the line by its number.
    let at = if text.is_empty() { vec![] } else { vec![text.to_string()] };
    Item::Note(Note { file: Some(file), at, text: say, line: Some(line) })
}

/// Each matching line once, with the matches in it replaced: a step for each that changes.
pub fn replacing(root: &Path, pattern: &regex::Regex, with: &str, found: &[ghostwrite_project::files::Found]) -> Vec<Item> {
    let mut items: Vec<Item> = vec![];
    let mut last = None;
    for f in found {
        if f.text.is_empty() || last == Some((&f.path, f.line)) {
            continue;
        }
        last = Some((&f.path, f.line));
        let new = pattern.replace_all(&f.text, with).into_owned();
        if new != f.text {
            let file = ghostwrite_project::platform::with_slashes(f.path.strip_prefix(root).unwrap_or(&f.path));
            let say = format!("{} → {}", pattern.as_str().trim_start_matches("(?i)"), with);
            items.push(Item::Step(Step { file: Some(file), old: vec![f.text.clone()], line: Some(f.line), new: vec![new], say, op: None, complete: true }));
        }
    }
    items
}
