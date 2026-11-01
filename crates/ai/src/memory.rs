//! What Claude keeps between stories, per project (in Ghostwrite's
//! support folder, not the project): its own condensed notes, and a log of
//! recent interactions. And the last exchange, for when a step won't place.

use std::path::Path;

use ghostwrite_project::store;
use serde_json::Value;

/// Interactions kept in a project's log: it's for recent context; older
/// knowledge lives in the notes.
const MAX_LOGGED: usize = 200;

/// Claude's notes about the project and the user, as it last condensed them.
pub fn notes(root: &Path) -> String {
    store::read(&format!("{}/memory.md", store::project_dir(root))).unwrap_or_default()
}

/// Keep Claude's notes: at most [`MAX_NOTES`] bytes of them (they're asked
/// to be under 300 words, and go into every request after).
pub fn save_notes(root: &Path, notes: &str) {
    let notes = &notes[..notes.floor_char_boundary(MAX_NOTES)];
    store::write(&format!("{}/memory.md", store::project_dir(root)), notes);
}

/// The most of Claude's notes kept: well past 300 words, short of a runaway.
pub const MAX_NOTES: usize = 4_000;

/// Add an interaction to the project's log.
pub(crate) fn log(root: &Path, entry: &Value) {
    let name = format!("{}/interactions.jsonl", store::project_dir(root));
    let mut entries = store::lines(&name);
    entries.push(entry.to_string());
    store::write_lines(&name, &entries, MAX_LOGGED);
}

/// The last `n` interactions, oldest first.
pub fn recent(root: &Path, n: usize) -> Vec<Value> {
    let entries = store::lines(&format!("{}/interactions.jsonl", store::project_dir(root)));
    entries[entries.len().saturating_sub(n)..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// The last request to Claude and its replies.
pub fn save_transcript(text: &str) {
    store::write("last-reply.txt", text);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_claudes_notes_and_log_per_project() {
        store::redirect_for_tests();
        let root = PathBuf::from("/tmp/ghostwrite-memory-test");
        save_notes(&root, "Prefers small steps.");
        assert_eq!(notes(&root), "Prefers small steps.");
        assert_eq!(notes(Path::new("/tmp/elsewhere")), "");
        for i in 0..3 {
            log(&root, &json!({ "request": i }));
        }
        assert_eq!(recent(&root, 2), [json!({ "request": 1 }), json!({ "request": 2 })]);
    }
}
