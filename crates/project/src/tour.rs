//! A tour: places to Tab through. A finished `:sh` command's output can be
//! one (each error it points at is a stop, or without errors each page of
//! output); so are a file's diagnostics and a project search's matches.
//! This is the bookkeeping: where the tour is, what the panel shows, and
//! what it says when it's over. Showing the stops is the editor's.

use std::ops::Range;
use std::path::Path;

use crate::files::Found;
use crate::shell::{Location, exit_label, locations};

/// Output lines per page.
pub const PAGE_ROWS: usize = 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// A page of output, from this line.
    Output(usize),
    Error(Location),
    /// A search match: where, and its bytes in the line.
    Match(Location, Range<usize>),
}

impl Stop {
    pub fn location(&self) -> Option<&Location> {
        match self {
            Stop::Error(loc) | Stop::Match(loc, _) => Some(loc),
            Stop::Output(_) => None,
        }
    }
}

/// What the tour is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Command,
    Diagnostics,
    Search,
}

/// Where a move left the tour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// At this stop now.
    To(usize),
    /// Nowhere to go (the first stop).
    Stayed,
    /// Past the last stop: the tour is over.
    Finished,
}

#[derive(Debug)]
pub struct Tour {
    /// The command, or what the tour is (`3 diagnostics`).
    pub title: String,
    source: Source,
    output: Vec<String>,
    /// The command's exit code (`None`: stopped).
    exit: Option<i32>,
    stops: Vec<Stop>,
    current: usize,
    /// Claude is being asked about something; the tour waits here.
    paused: bool,
}

impl Tour {
    /// A command that has exited: the errors its output points at (or its
    /// pages) are the stops. `None` if there's nothing to tour.
    pub fn command(command: String, output: Vec<String>, exit: Option<i32>, root: &Path) -> Option<Self> {
        let errors = locations(&output.join("\n"), root);
        let stops: Vec<Stop> =
            if errors.is_empty() { (0..output.len()).step_by(PAGE_ROWS).map(Stop::Output).collect() } else { errors.into_iter().map(Stop::Error).collect() };
        (!stops.is_empty()).then_some(Self { title: command, source: Source::Command, output, exit, stops, current: 0, paused: false })
    }

    pub fn diagnostics(locations: Vec<Location>) -> Self {
        let n = locations.len();
        let title = format!("{n} diagnostic{}", if n == 1 { "" } else { "s" });
        let stops = locations.into_iter().map(Stop::Error).collect();
        Self { title, source: Source::Diagnostics, output: vec![], exit: Some(0), stops, current: 0, paused: false }
    }

    /// The matches of `typed` (`complete`: the search didn't stop early).
    pub fn search(typed: &str, found: Vec<Found>, complete: bool) -> Self {
        let n = found.len();
        let title = format!("{n}{} match{} for {typed}", if complete { "" } else { "+" }, if n == 1 { "" } else { "es" });
        let stops = found.into_iter().map(|f| Stop::Match(Location { path: f.path, line: f.line, message: vec![f.text] }, f.bytes)).collect();
        Self { title, source: Source::Search, output: vec![], exit: Some(0), stops, current: 0, paused: false }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> Option<&Stop> {
        self.stops.get(self.current)
    }

    pub fn forward(&mut self) -> Moved {
        if self.current + 1 >= self.stops.len() {
            return Moved::Finished;
        }
        self.current += 1;
        Moved::To(self.current)
    }

    pub fn back(&mut self) -> Moved {
        if self.current == 0 {
            return Moved::Stayed;
        }
        self.current -= 1;
        Moved::To(self.current)
    }

    /// Pause here while Claude is asked about something.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Claude is done: back to this stop.
    pub fn resume(&mut self) -> Moved {
        self.paused = false;
        Moved::To(self.current)
    }

    pub fn exit_label(&self) -> String {
        if self.source != Source::Command {
            return self.title.clone();
        }
        exit_label(self.exit)
    }

    pub fn failed(&self) -> bool {
        self.exit != Some(0)
    }

    /// What to say once it's over (a search just ends).
    pub fn summary(&self) -> Option<String> {
        let errors = self.stops.iter().filter(|s| matches!(s, Stop::Error(_))).count();
        match self.source {
            Source::Search => None,
            Source::Diagnostics => Some(self.title.clone()),
            Source::Command if errors == 0 => Some(format!("`{}`: {}", self.title, self.exit_label())),
            Source::Command => Some(format!("`{}`: {}, {errors} location{}", self.title, self.exit_label(), if errors == 1 { "" } else { "s" })),
        }
    }

    /// The panel's lines: the current page, or the current error's message.
    /// A match shows in the editor instead.
    pub fn rows(&self) -> Vec<String> {
        match self.current() {
            Some(Stop::Output(from)) => self.output.iter().skip(*from).take(PAGE_ROWS).cloned().collect(),
            Some(Stop::Error(loc)) => loc.message.iter().take(PAGE_ROWS).cloned().collect(),
            Some(Stop::Match(..)) | None => vec![],
        }
    }

    /// The keys, under the panel.
    pub fn hint(&self) -> String {
        let label = self.exit_label();
        let progress = format!("  {}/{}", self.current + 1, self.stops.len());
        let hint = match self.current() {
            Some(Stop::Error(_)) => format!("{label} · ⇥ next · ⇧⇥ back · ⏎ ask Claude here · esc{progress}"),
            Some(Stop::Match(..)) => format!("{label} · ⇥ next · ⇧⇥ back · ⏎ ask Claude here · esc{progress}"),
            _ if self.current + 1 >= self.stops.len() => format!("{label} · ⇥ done · ⇧⇥ back{progress}"),
            _ => format!("{label} · ⇥ more · ⇧⇥ back{progress}"),
        };
        if self.source == Source::Command { format!("$ {}  ·  {hint}", self.title) } else { hint }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commands_errors_are_its_stops() {
        let root = crate::scratch_dir("tour");
        std::fs::write(root.join("a.py"), "one\ntwo\n").unwrap();
        let output = ["error: bad", "  --> a.py:2", "", "error: worse", "  --> a.py:1"].map(String::from).to_vec();
        let mut tour = Tour::command("check".into(), output, Some(1), &root).unwrap();
        assert_eq!(tour.current().and_then(Stop::location).map(|l| l.line), Some(1));
        assert_eq!(tour.rows(), ["error: bad", "  --> a.py:2"]);
        assert_eq!(tour.forward(), Moved::To(1));
        assert_eq!(tour.back(), Moved::To(0));
        tour.pause();
        assert!(tour.paused());
        assert_eq!(tour.resume(), Moved::To(0), "after asking Claude: still here");
        assert_eq!(tour.forward(), Moved::To(1));
        assert_eq!(tour.forward(), Moved::Finished);
        assert_eq!(tour.summary().as_deref(), Some("`check`: ✗ exit 1, 2 locations"));
    }

    #[test]
    fn output_without_errors_pages() {
        let output = (0..20).map(|i| format!("line {i}")).collect();
        let mut tour = Tour::command("ls".into(), output, Some(0), Path::new("/nowhere")).unwrap();
        assert_eq!(tour.rows().len(), PAGE_ROWS);
        assert_eq!(tour.forward(), Moved::To(1));
        assert_eq!(tour.rows().len(), 6);
        assert_eq!(tour.forward(), Moved::Finished);
        assert_eq!(tour.summary().as_deref(), Some("`ls`: ✓ exit 0"));
        assert!(Tour::command("true".into(), vec![], Some(0), Path::new("/")).is_none(), "no output: nothing to tour");
    }

    #[test]
    fn matches_are_asked_about() {
        let found = vec![Found { path: "/p/a.rs".into(), line: 3, bytes: 4..9, text: "let total = 1;".into() }];
        let mut tour = Tour::search("total", found, true);
        assert_eq!(tour.title, "1 match for total");
        assert!(tour.rows().is_empty(), "the match shows in the editor");
        tour.pause();
        assert_eq!(tour.resume(), Moved::To(0), "asking about a match stays at it");
        assert_eq!(tour.summary(), None);
    }
}
