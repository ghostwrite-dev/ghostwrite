//! A finished command's stops: the places its output points at (each error
//! it names), or, when it names none, its pages. The editor shows them as a
//! story of notes, one at a time.

use std::path::Path;

use crate::shell::{Location, locations};

/// Output lines per page.
pub const PAGE_ROWS: usize = 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Error(Location),
    /// A page of output.
    Page(Vec<String>),
}

/// The stops in a command's `output`, for files under `root`.
pub fn command_stops(output: &[String], root: &Path) -> Vec<Stop> {
    let errors = locations(&output.join("\n"), root);
    if errors.is_empty() {
        return output.chunks(PAGE_ROWS).map(|page| Stop::Page(page.to_vec())).collect();
    }
    errors.into_iter().map(Stop::Error).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commands_errors_are_its_stops() {
        let root = crate::scratch_dir("stops");
        std::fs::write(root.join("a.py"), "one\ntwo\n").unwrap();
        let output = ["error: bad", "  --> a.py:2", "", "error: worse", "  --> a.py:1"].map(String::from).to_vec();
        let lines: Vec<usize> = command_stops(&output, &root).iter().map(|s| if let Stop::Error(l) = s { l.line } else { usize::MAX }).collect();
        assert_eq!(lines, [1, 0]);
    }

    #[test]
    fn without_errors_its_pages_are() {
        let root = crate::scratch_dir("pages");
        let output: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let stops = command_stops(&output, &root);
        assert_eq!(stops.len(), 2);
        assert!(matches!(&stops[1], Stop::Page(page) if page.len() == 20 - PAGE_ROWS));
    }
}
