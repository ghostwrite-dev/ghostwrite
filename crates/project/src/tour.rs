//! A tour: places to Tab through. A `:sh` command's output becomes one once
//! it exits (each error it points at is a stop, or without errors each page
//! of output); so do a file's diagnostics and a project search's matches.
//! This is the bookkeeping: where the tour is, what the panel shows, and
//! what it says when it's over. Running the command and showing the stops
//! is the editor's.

use std::ops::Range;
use std::path::Path;

use crate::files::Found;
use crate::shell::{Location, locations};

/// Output lines per page, and shown while the command runs.
pub const PAGE_ROWS: usize = 14;
const TAIL_ROWS: usize = 8;

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
    /// Nowhere to go (the first stop, or still running).
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
    /// `None` while the command runs; then its exit code (`None`: stopped).
    exit: Option<Option<i32>>,
    stops: Vec<Stop>,
    current: usize,
    /// Handing the current stop to Claude (or asking about something else).
    paused: bool,
    /// Once Claude is done, go on to the next stop (it was fixing this one).
    advance: bool,
}

/// What Enter hands to Claude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandOff {
    /// Output to include with the question (an error, a page, the tail so far).
    pub output: Option<String>,
    /// It's an error to fix: the prompt starts with "fix this", and the tour
    /// moves on once it's done.
    pub fix: bool,
}

impl Tour {
    /// A command that's starting; its stops come when it exits.
    pub fn command(command: String) -> Self {
        Self { title: command, source: Source::Command, output: vec![], exit: None, stops: vec![], current: 0, paused: false, advance: false }
    }

    pub fn diagnostics(locations: Vec<Location>) -> Self {
        let n = locations.len();
        let title = format!("{n} diagnostic{}", if n == 1 { "" } else { "s" });
        let stops = locations.into_iter().map(Stop::Error).collect();
        Self { title, source: Source::Diagnostics, output: vec![], exit: Some(Some(0)), stops, current: 0, paused: false, advance: false }
    }

    /// The matches of `typed` (`complete`: the search didn't stop early).
    pub fn search(typed: &str, found: Vec<Found>, complete: bool) -> Self {
        let n = found.len();
        let title = format!("{n}{} match{} for {typed}", if complete { "" } else { "+" }, if n == 1 { "" } else { "es" });
        let stops = found.into_iter().map(|f| Stop::Match(Location { path: f.path, line: f.line, message: vec![f.text] }, f.bytes)).collect();
        Self { title, source: Source::Search, output: vec![], exit: Some(Some(0)), stops, current: 0, paused: false, advance: false }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn running(&self) -> bool {
        self.exit.is_none()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn push_output(&mut self, line: String) {
        self.output.push(line);
    }

    /// The command exited: the errors its output points at (or its pages)
    /// become the stops. `false` if there's nothing to tour.
    pub fn exited(&mut self, code: Option<i32>, root: &Path) -> bool {
        self.exit = Some(code);
        let errors = locations(&self.output.join("\n"), root);
        self.stops = if errors.is_empty() {
            (0..self.output.len()).step_by(PAGE_ROWS).map(Stop::Output).collect()
        } else {
            errors.into_iter().map(Stop::Error).collect()
        };
        !self.stops.is_empty()
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> Option<&Stop> {
        self.stops.get(self.current).filter(|_| !self.running())
    }

    pub fn forward(&mut self) -> Moved {
        if self.running() {
            return Moved::Stayed;
        }
        if self.current + 1 >= self.stops.len() {
            return Moved::Finished;
        }
        self.current += 1;
        Moved::To(self.current)
    }

    pub fn back(&mut self) -> Moved {
        if self.running() || self.current == 0 {
            return Moved::Stayed;
        }
        self.current -= 1;
        Moved::To(self.current)
    }

    /// Enter: hand the current stop to Claude, pausing here. An error is to
    /// fix; anything else is asked about, with what's in the panel.
    pub fn hand_off(&mut self) -> HandOff {
        let (output, fix) = match self.current() {
            Some(Stop::Error(loc)) => (Some(loc.message.join("\n")), true),
            Some(Stop::Match(..)) => (None, false),
            _ => (Some(self.rows().join("\n")), false),
        };
        self.paused = true;
        self.advance = fix;
        HandOff { output: output.map(|o| format!("$ {}\n{o}", self.title)), fix }
    }

    /// Pause here while Claude is asked about something else.
    pub fn pause(&mut self) {
        self.paused = true;
        self.advance = false;
    }

    /// Claude is done: on to the next stop after a fix, else back to this one.
    pub fn resume(&mut self) -> Moved {
        self.paused = false;
        match std::mem::take(&mut self.advance) {
            true => self.forward(),
            false if self.running() => Moved::Stayed,
            false => Moved::To(self.current),
        }
    }

    pub fn exit_label(&self) -> String {
        if self.source != Source::Command {
            return self.title.clone();
        }
        match self.exit {
            None => "running".into(),
            Some(Some(0)) => "✓ exit 0".into(),
            Some(Some(code)) => format!("✗ exit {code}"),
            Some(None) => "✗ stopped".into(),
        }
    }

    pub fn failed(&self) -> bool {
        matches!(self.exit, Some(Some(code)) if code != 0) || matches!(self.exit, Some(None))
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

    /// The panel's lines: the tail while running, the current page, or the
    /// current error's message. A match shows in the editor instead.
    pub fn rows(&self) -> Vec<String> {
        if self.running() {
            return self.output.iter().rev().take(TAIL_ROWS).rev().cloned().collect();
        }
        match self.current() {
            Some(Stop::Output(from)) => self.output.iter().skip(*from).take(PAGE_ROWS).cloned().collect(),
            Some(Stop::Error(loc)) => loc.message.iter().take(PAGE_ROWS).cloned().collect(),
            Some(Stop::Match(..)) | None => vec![],
        }
    }

    /// The keys, under the panel.
    pub fn hint(&self) -> String {
        if self.running() {
            return format!("$ {} · esc stop", self.title);
        }
        let label = self.exit_label();
        let progress = format!("  {}/{}", self.current + 1, self.stops.len());
        let hint = match self.current() {
            Some(Stop::Error(_)) => format!("{label} · ⇥ next · ⇧⇥ back · ⏎ fix with Claude{progress}"),
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
        let mut tour = Tour::command("check".into());
        assert_eq!(tour.forward(), Moved::Stayed, "nothing to tour while it runs");
        for line in ["error: bad", "  --> a.py:2", "", "error: worse", "  --> a.py:1"] {
            tour.push_output(line.into());
        }
        assert_eq!(tour.rows().len(), 5, "the tail while running");
        assert!(tour.exited(Some(1), &root));
        assert_eq!(tour.current().and_then(Stop::location).map(|l| l.line), Some(1));
        assert_eq!(tour.rows(), ["error: bad", "  --> a.py:2"]);
        assert_eq!(tour.forward(), Moved::To(1));
        assert_eq!(tour.back(), Moved::To(0));
        assert_eq!(tour.hand_off(), HandOff { output: Some("$ check\nerror: bad\n  --> a.py:2".into()), fix: true });
        assert!(tour.paused());
        assert_eq!(tour.resume(), Moved::To(1), "fixed: on to the next");
        tour.pause();
        assert_eq!(tour.resume(), Moved::To(1), "asked about something else: still here");
        assert_eq!(tour.forward(), Moved::Finished);
        assert_eq!(tour.summary().as_deref(), Some("`check`: ✗ exit 1, 2 locations"));
    }

    #[test]
    fn output_without_errors_pages() {
        let mut tour = Tour::command("ls".into());
        for i in 0..20 {
            tour.push_output(format!("line {i}"));
        }
        assert!(tour.exited(Some(0), Path::new("/nowhere")));
        assert_eq!(tour.rows().len(), PAGE_ROWS);
        assert_eq!(tour.forward(), Moved::To(1));
        assert_eq!(tour.rows().len(), 6);
        assert_eq!(tour.hand_off(), HandOff { output: Some(format!("$ ls\n{}", (14..20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"))), fix: false }, "a page is asked about");
        assert_eq!(tour.resume(), Moved::To(1));
        assert_eq!(tour.forward(), Moved::Finished);
        assert_eq!(tour.summary().as_deref(), Some("`ls`: ✓ exit 0"));
        assert!(!Tour::command("true".into()).exited(Some(0), Path::new("/")), "no output: nothing to tour");
    }

    #[test]
    fn matches_are_asked_about() {
        let found = vec![Found { path: "/p/a.rs".into(), line: 3, bytes: 4..9, text: "let total = 1;".into() }];
        let mut tour = Tour::search("total", found, true);
        assert_eq!(tour.title, "1 match for total");
        assert!(tour.rows().is_empty(), "the match shows in the editor");
        assert_eq!(tour.hand_off(), HandOff { output: None, fix: false });
        assert_eq!(tour.resume(), Moved::To(0), "asking about a match stays at it");
        assert_eq!(tour.summary(), None);
    }
}
