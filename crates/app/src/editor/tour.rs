//! Tours in the editor: `:sh <command>` runs it with its output in a panel
//! above the command line; once it exits, its errors are stops to Tab
//! through. `space d` tours the file's diagnostics, `:grep` / `space /` the
//! project's matches. Enter hands the stop to Claude.
//!
//! [`Tour`] keeps the bookkeeping; [`TourState`] adds the running command
//! and shows each stop in its file. The editor switches tabs.

use futures::StreamExt as _;
use ghostwrite_core::{Mode, Range, Review};
use ghostwrite_project::files::{MAX_FOUND, grep, search_pattern};
use ghostwrite_project::shell::{Location, Run};
use ghostwrite_project::tour::{Moved, Source, Stop, Tour};
use gpui::{AnyElement, AppContext as _, Context, Task, Window, div, prelude::*};

use super::element::Decoration;
use super::{Autoscroll, Document, Editor, Project};
use crate::theme::Theme;

pub(super) struct TourState {
    pub(super) tour: Tour,
    /// The command, while it runs (dropping it stops it).
    run: Option<Run>,
    _task: Option<Task<()>>,
}

impl Drop for TourState {
    fn drop(&mut self) {
        if let Some(run) = &self.run {
            run.stop();
        }
    }
}

impl TourState {
    fn new(tour: Tour) -> Self {
        Self { tour, run: None, _task: None }
    }

    /// Select the current stop in its file (opening it); the tab to show.
    fn show(&self, project: &mut Project) -> Option<usize> {
        let stop = self.tour.current().filter(|_| !self.tour.paused())?;
        let loc = stop.location()?;
        let ix = project.open(&loc.path)?;
        let tab = project.tabs.get_mut(ix)?;
        let start = tab.doc.buffer.line_start(loc.line);
        let range = match stop {
            Stop::Match(_, bytes) => Range::new(start + bytes.start, start + bytes.end),
            _ => Range::point(start),
        };
        tab.select(range, Autoscroll::Center(loc.line..loc.line + 1));
        Some(ix)
    }

    /// The current stop's mark in `doc`: an error's line, or a match.
    pub(super) fn decorations(&self, doc: &Document, theme: &Theme) -> Vec<Decoration> {
        let Some(stop) = self.tour.current().filter(|_| !self.tour.paused()) else { return vec![] };
        let Some(loc) = stop.location().filter(|l| doc.path.as_deref() == Some(l.path.as_path())) else { return vec![] };
        let line = doc.buffer.lines_range(loc.line..loc.line + 1);
        match stop {
            Stop::Match(_, bytes) => {
                let range = (line.start + bytes.start).min(doc.buffer.len())..(line.start + bytes.end).min(doc.buffer.len());
                vec![Decoration::background(range, theme.warning.opacity(0.3))]
            }
            _ => vec![Decoration::background(line, theme.danger.opacity(0.14))],
        }
    }

    /// The output (or the current error) in a panel, with the keys under it.
    pub(super) fn panel(&self, theme: &Theme) -> Option<AnyElement> {
        let tour = &self.tour;
        if tour.paused() {
            return None;
        }
        let color = if tour.running() || tour.source() == Source::Search {
            theme.muted_foreground
        } else if tour.failed() || tour.source() != Source::Command {
            theme.danger
        } else {
            theme.success
        };
        // Command output keeps its lines; an error's message wraps to the panel.
        let wrap = matches!(tour.current(), Some(Stop::Error(_)));
        let rows = tour.rows().into_iter().map(|l| div().when(!wrap, |d| d.whitespace_nowrap().overflow_hidden()).child(if l.is_empty() { " ".to_string() } else { l }));
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(theme.border)
                .font_family(theme.mono_font.clone())
                .text_size(theme.mono_size)
                .text_color(theme.muted_foreground)
                .children(rows)
                .child(div().pt_1().text_xs().text_color(color).child(tour.hint()))
                .into_any_element(),
        )
    }
}

impl Editor {
    /// `:sh`: start a command (stopping any still running).
    pub(super) fn start_shell(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.last_shell = Some(command.clone());
        let run = Run::start(&command, &self.project.root).map_err(|e| format!("couldn't run `{command}`: {e}"))?;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Result<String, Option<i32>>>();
        let reader = run.handle();
        cx.background_spawn(async move {
            let lines = tx.clone();
            let code = reader.stream(&mut |line| drop(lines.unbounded_send(Ok(line))));
            tx.unbounded_send(Err(code)).ok();
        })
        .detach();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = message.is_err();
                let alive = this.update(cx, |this, cx| this.shell_output(message, cx)).unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        });
        let mut state = TourState::new(Tour::command(command));
        state.run = Some(run);
        state._task = Some(task);
        self.begin_tour(state);
        cx.notify();
        Ok(())
    }

    /// A line of output, or the exit. `false` once there's no command to feed.
    fn shell_output(&mut self, message: Result<String, Option<i32>>, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.tour.as_mut().filter(|s| s.tour.source() == Source::Command) else { return false };
        match message {
            Ok(line) => state.tour.push_output(line),
            Err(code) => {
                state.run = None;
                if state.tour.exited(code, &self.project.root) {
                    self.show_stop(cx);
                } else {
                    let said = format!("`{}`: {}, no output", state.tour.title, state.tour.exit_label());
                    self.project.say(said);
                    self.end_tour();
                }
            }
        }
        cx.notify();
        true
    }

    /// `space d`: tour this file's diagnostics.
    pub(super) fn diagnostics_tour(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.tab().map(|t| &t.doc) else { return };
        let Some(path) = doc.path.clone() else { return };
        let stops: Vec<Location> = doc
            .diagnostics
            .iter()
            .map(|d| {
                let message = format!("{}: {}", d.severity.label(), d.message);
                Location { path: path.clone(), line: doc.buffer.line_of(d.range.start), message: message.lines().map(str::to_string).collect() }
            })
            .collect();
        if stops.is_empty() {
            return self.project.say("no diagnostics in this file");
        }
        self.begin_tour(TourState::new(Tour::diagnostics(stops)));
        self.show_stop(cx);
    }

    /// `:grep` and `space /`: search the project (on a background thread),
    /// then tour the matches.
    pub(super) fn start_search(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let pattern = search_pattern(typed)?;
        let open = self.project.tabs.iter().filter_map(|t| Some((t.doc.path.clone()?, t.doc.text().to_string()))).collect();
        let root = self.project.root.clone();
        let search = cx.background_spawn(async move { grep(&root, &pattern, &open) });
        let typed = typed.to_string();
        self.project.say(format!("searching for {typed}…"));
        cx.spawn_in(window, async move |this, cx| {
            let found = search.await;
            this.update(cx, |this, cx| {
                if found.is_empty() {
                    this.project.say(format!("no matches for {typed}"));
                } else {
                    this.project.status = None;
                    let complete = found.len() < MAX_FOUND;
                    this.begin_tour(TourState::new(Tour::search(&typed, found, complete)));
                    this.show_stop(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    /// Tour mode, for this tour (replacing any other).
    fn begin_tour(&mut self, state: TourState) {
        self.leave_insert();
        self.tour = Some(state);
        self.modal.enter(Mode::Tour);
    }

    fn end_tour(&mut self) {
        self.tour = None;
        self.modal.leave(Mode::Tour);
    }

    /// Bring the tour's current stop into view.
    fn show_stop(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.tour.as_ref().and_then(|s| s.show(&mut self.project)) else { return };
        if ix != self.project.tabs.active_index() {
            self.activate(ix, cx);
        }
    }

    /// A review key for the tour, if one is up: Tab on (the last finishes),
    /// Shift+Tab back, Enter hands the stop to Claude, Esc stops or closes.
    pub(super) fn tour_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.tour.as_mut().filter(|s| !s.tour.paused()) else { return };
        let moved = match key {
            // Esc stops a running command, and closes a finished tour.
            Review::Stop => {
                let running = state.run.as_ref().map(Run::stop).is_some();
                if !running {
                    self.end_tour();
                }
                return;
            }
            Review::Next => state.tour.forward(),
            Review::Back => state.tour.back(),
            Review::Ask => {
                let handed = state.tour.hand_off();
                match handed.output {
                    Some(output) if handed.fix => self.open_story(Some(output), "fix this", window, cx),
                    // A question about the output: asked, and answered, in the panel.
                    Some(output) => self.open_story_in_panel(output, window, cx),
                    None => self.open_story(None, "", window, cx),
                }
                return;
            }
            Review::Skip => return,
        };
        self.tour_moved(moved, cx);
    }

    fn tour_moved(&mut self, moved: Moved, cx: &mut Context<Self>) {
        match moved {
            Moved::To(_) => self.show_stop(cx),
            Moved::Stayed => {}
            Moved::Finished => {
                if let Some(summary) = self.tour.as_ref().and_then(|s| s.tour.summary()) {
                    self.project.say(summary);
                }
                self.end_tour();
            }
        }
    }

    /// Claude is done with a stop: on to the next one.
    pub(super) fn resume_tour(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.tour.as_mut().filter(|s| s.tour.paused()) else { return };
        let moved = state.tour.resume();
        self.tour_moved(moved, cx);
    }
}
