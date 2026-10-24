//! Tours in the editor: `:output` tours the last `:sh` command's errors
//! (or its pages of output), `space d` the file's diagnostics, `:grep` /
//! `space /` the project's matches. Tab goes on, Shift+Tab back; Enter asks
//! Claude about the stop, as any question, and the tour waits.
//!
//! [`Tour`] keeps the bookkeeping; [`TourState`] shows each stop in its
//! file. The editor switches tabs.

use ghostwrite_core::{Mode, Range, Review};
use ghostwrite_project::files::{MAX_FOUND, grep, search_pattern};
use ghostwrite_project::shell::Location;
use ghostwrite_project::tour::{Moved, Source, Stop, Tour};
use gpui::{AnyElement, AppContext as _, Context, Window, div, prelude::*};

use super::element::Decoration;
use super::{Autoscroll, Document, Editor, Project};
use crate::theme::Theme;

pub(super) struct TourState {
    pub(super) tour: Tour,
}

impl TourState {
    fn new(tour: Tour) -> Self {
        Self { tour }
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
        let color = if tour.source() == Source::Search {
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
    /// `:output`: tour the last command's errors, or its output.
    pub(super) fn output_tour(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let tour = self.shell.as_ref().ok_or("no command has run yet (:sh ‹command›)")?.tour(&self.project.root)?;
        self.begin_tour(TourState::new(tour));
        self.show_stop(cx);
        Ok(())
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
    /// Shift+Tab back, Enter asks Claude about the stop, Esc closes.
    pub(super) fn tour_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.tour.as_mut().filter(|s| !s.tour.paused()) else { return };
        let moved = match key {
            Review::Stop => return self.end_tour(),
            Review::Next => state.tour.forward(),
            Review::Back => state.tour.back(),
            // A question like any other, about the lines at the stop; the
            // command and its output go along anyway.
            Review::Ask => return self.open_story("", window, cx),
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

    /// Claude is done: back to the stop.
    pub(super) fn resume_tour(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.tour.as_mut().filter(|s| s.tour.paused()) else { return };
        let moved = state.tour.resume();
        self.tour_moved(moved, cx);
    }
}
