//! `:sh <command>`: run it in the project, its output in a panel above the
//! command line. When it's done, each error it points at is a stop: Tab jumps
//! the editor to it (opening its file, marking the line), Enter hands it to
//! Claude, and the last Tab finishes. Without errors, the output pages.
//! The same tour walks `space d` diagnostics and `space /` search matches.

use std::process::Child;
use std::sync::Mutex;

use super::review::line_bytes;
use super::*;
use crate::shell::{self, Location};

/// Output lines shown per page, and while the command runs.
const PAGE_ROWS: usize = 14;
const TAIL_ROWS: usize = 8;

pub(super) enum Stop {
    /// A page of output, from this line.
    Output(usize),
    Error(Location),
    /// A search match: where, and its bytes in the line.
    Match(Location, std::ops::Range<usize>),
}

impl Stop {
    fn location(&self) -> Option<&Location> {
        match self {
            Stop::Error(loc) | Stop::Match(loc, _) => Some(loc),
            Stop::Output(_) => None,
        }
    }
}

pub(super) struct ShellRun {
    command: String,
    /// A tour that isn't a command (`space d`): what to call it.
    label: Option<String>,
    output: Vec<String>,
    exit: Option<Option<i32>>,
    stops: Vec<Stop>,
    current: usize,
    shown: Option<usize>,
    /// Handing the current error to Claude; resumes after.
    pub(super) paused: bool,
    /// The buffer the error line mark is in.
    gap: Option<(Entity<EditorState>, TextDecorationCollection)>,
    child: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

impl ShellRun {
    fn running(&self) -> bool {
        self.exit.is_none()
    }

    /// Still running (tests wait on this).
    #[cfg(test)]
    pub(super) fn stops_pending(&self) -> bool {
        self.running()
    }

    fn exit_label(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        match self.exit {
            None => "running".into(),
            Some(Some(0)) => "✓ exit 0".into(),
            Some(Some(code)) => format!("✗ exit {code}"),
            Some(None) => "✗ stopped".into(),
        }
    }
}

impl TextEditor {
    /// `:sh`: start a command (stopping any still running).
    pub(super) fn start_shell(&mut self, command: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let command = command.or_else(|| self.last_shell.clone()).ok_or("usage: :sh <command>")?;
        self.last_shell = Some(command.clone());
        self.close_shell(window, cx);
        let child = shell::spawn(&command, &self.root).map_err(|e| format!("couldn't run `{command}`: {e}"))?;
        let child = Arc::new(Mutex::new(Some(child)));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Result<String, Option<i32>>>();
        let (reader, stop) = (child.clone(), cancel.clone());
        cx.background_executor()
            .spawn(async move {
                let lines = tx.clone();
                let code = shell::stream(reader, &stop, &mut |line| drop(lines.unbounded_send(Ok(line))));
                tx.unbounded_send(Err(code)).ok();
            })
            .detach();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let done = msg.is_err();
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        let Some(run) = this.shell_run.as_mut() else { return false };
                        match msg {
                            Ok(line) => run.output.push(line),
                            Err(code) => {
                                run.exit = Some(code);
                                let text = run.output.join("\n");
                                let errors = shell::locations(&text, &this.root);
                                let run = this.shell_run.as_mut().expect("still running");
                                run.stops = if errors.is_empty() {
                                    (0..run.output.len()).step_by(PAGE_ROWS).map(Stop::Output).collect()
                                } else {
                                    errors.into_iter().map(Stop::Error).collect()
                                };
                                if run.stops.is_empty() {
                                    let label = run.exit_label();
                                    this.status = Some(format!("`{}`: {label}, no output", this.last_shell.clone().unwrap_or_default()).into());
                                    this.close_shell(window, cx);
                                    return false;
                                }
                                this.settle_shell(window, cx);
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        });
        self.shell_run = Some(ShellRun {
            command,
            label: None,
            output: vec![],
            exit: None,
            stops: vec![],
            current: 0,
            shown: None,
            paused: false,
            gap: None,
            child,
            cancel,
            _task: Some(task),
        });
        cx.notify();
        Ok(())
    }

    /// Tab through `locations` (diagnostics) the way `:sh` errors work.
    pub(super) fn start_tour(&mut self, label: String, locations: Vec<Location>, window: &mut Window, cx: &mut Context<Self>) {
        self.start_stops(label, locations.into_iter().map(Stop::Error).collect(), window, cx);
    }

    /// `space /` and `:grep`: search the project (on a background thread),
    /// then tour the matches; Enter asks Claude about the one you're at.
    pub(super) fn start_search(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let pattern = crate::project::search_pattern(typed)?;
        let open = self.buffers.iter().filter_map(|b| Some((b.path.clone()?, b.state.read(cx).text().to_string()))).collect();
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { crate::project::grep(&root, &pattern, &open) });
        let typed = typed.to_string();
        self.status = Some(format!("searching for {typed}…").into());
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await;
            this.update_in(cx, |this, window, cx| {
                if found.is_empty() {
                    this.status = Some(format!("no matches for {typed}").into());
                    return cx.notify();
                }
                this.status = None;
                let n = found.len();
                let more = if n >= crate::project::MAX_FOUND { "+" } else { "" };
                let label = format!("{n}{more} match{} for {typed}", if n == 1 { "" } else { "es" });
                let stops = found.into_iter().map(|f| Stop::Match(Location { path: f.path, line: f.line, message: vec![f.text] }, f.bytes)).collect();
                this.start_stops(label, stops, window, cx);
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    fn start_stops(&mut self, label: String, stops: Vec<Stop>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_shell(window, cx);
        self.shell_run = Some(ShellRun {
            command: label.clone(),
            label: Some(label),
            output: vec![],
            exit: Some(Some(0)),
            stops,
            current: 0,
            shown: None,
            paused: false,
            gap: None,
            child: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
            _task: None,
        });
        self.settle_shell(window, cx);
        cx.notify();
    }

    /// Bring the current stop into view: an error opens its file and centers
    /// its line; output pages just turn in the panel.
    fn settle_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.shell_run.as_mut() else { return };
        if run.paused || run.shown == Some(run.current) {
            return;
        }
        run.shown = Some(run.current);
        let Some(loc) = run.stops.get(run.current).and_then(Stop::location) else { return cx.notify() };
        let (path, line) = (loc.path.clone(), loc.line);
        // A match is selected, as helix leaves a search; an error puts the cursor at its line.
        let found = match &run.stops[run.current] {
            Stop::Match(_, bytes) => Some(bytes.clone()),
            _ => None,
        };
        self.open_paths(vec![path], window, cx);
        let Some(buffer) = self.buffers.get(self.active) else { return };
        let state = buffer.state.clone();
        state.update(cx, |st, cx| {
            let text = st.text().to_string();
            let start = line_bytes(&text, line..line + 1).start;
            let selected = found.map_or(start..start, |b| (start + b.start).min(text.len())..(start + b.end).min(text.len()));
            self.modal.select_bytes(selected, st, cx);
            st.center_on_lines(line..line + 1, px(0.), cx);
        });
    }

    /// Keys while a run is up: Tab / Shift+Tab move between stops (the last
    /// Tab finishes), Enter hands an error to Claude, Esc stops or closes.
    pub(super) fn shell_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.shell_run.as_mut().filter(|r| !r.paused) else { return false };
        if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
            return false;
        }
        match keystroke.key.as_str() {
            "escape" if run.running() => run.cancel.store(true, Ordering::SeqCst),
            "escape" => self.close_shell(window, cx),
            "tab" if run.running() => {}
            "tab" if keystroke.modifiers.shift => {
                run.current = run.current.saturating_sub(1);
                self.settle_shell(window, cx);
            }
            "tab" if run.current + 1 >= run.stops.len() => self.finish_shell(window, cx),
            "tab" => {
                run.current += 1;
                self.settle_shell(window, cx);
            }
            "enter" if !run.running() && matches!(run.stops.get(run.current), Some(Stop::Error(_))) => {
                let Some(Stop::Error(loc)) = run.stops.get(run.current) else { return true };
                let context = format!("$ {}\n{}", run.command, loc.message.join("\n"));
                run.paused = true;
                self.clear_shell_marks(cx);
                self.open_assist_with(Some(context), "fix this", window, cx);
            }
            // At a match: ask Claude about it (or to change it); the tour goes on after.
            "enter" if matches!(run.stops.get(run.current), Some(Stop::Match(..))) => {
                run.paused = true;
                self.clear_shell_marks(cx);
                self.open_assist_with(None, "", window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Claude is done with an error: carry on with the next stop.
    pub(super) fn resume_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.shell_run.as_mut().filter(|r| r.paused) else { return };
        run.paused = false;
        run.shown = None;
        if run.current + 1 >= run.stops.len() {
            return self.finish_shell(window, cx);
        }
        run.current += 1;
        self.settle_shell(window, cx);
    }

    fn finish_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.shell_run.as_ref() else { return };
        if matches!(run.stops.first(), Some(Stop::Match(..))) {
            return self.close_shell(window, cx);
        }
        let errors = run.stops.iter().filter(|s| matches!(s, Stop::Error(_))).count();
        let summary = match errors {
            0 => format!("`{}`: {}", run.command, run.exit_label()),
            n => format!("`{}`: {}, {n} location{}", run.command, run.exit_label(), if n == 1 { "" } else { "s" }),
        };
        self.close_shell(window, cx);
        self.status = Some(summary.into());
    }

    fn close_shell(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.clear_shell_marks(cx);
        if let Some(run) = self.shell_run.take() {
            run.cancel.store(true, Ordering::SeqCst);
            if let Ok(mut child) = run.child.lock()
                && let Some(child) = child.as_mut()
            {
                child.kill().ok();
            }
        }
        cx.notify();
    }

    fn clear_shell_marks(&mut self, cx: &mut Context<Self>) {
        if let Some((_, mark)) = self.shell_run.as_mut().and_then(|r| r.gap.take()) {
            mark.clear(cx);
        }
    }

    /// The lines to fold out now: the tail while running, else the current stop.
    fn shell_rows(&self, cx: &App) -> Vec<String> {
        let Some(run) = self.shell_run.as_ref() else { return vec![] };
        let width = self.area.get().map_or(px(640.), |a| a.size.width) - px(200.);
        let cols = (f32::from(width) / f32::from(cx.theme().mono_font_size * 0.6)) as usize;
        match run.stops.get(run.current) {
            _ if run.running() => run.output.iter().rev().take(TAIL_ROWS).rev().cloned().collect(),
            Some(Stop::Output(from)) => run.output.iter().skip(*from).take(PAGE_ROWS).cloned().collect(),
            Some(Stop::Error(loc)) => assist::wrap(&loc.message.join("\n"), cols).into_iter().take(PAGE_ROWS).collect(),
            // The match is right there in the editor.
            Some(Stop::Match(..)) | None => vec![],
        }
    }

    /// Mark the current error's line in `buffer`, if that's where it is.
    pub(super) fn mark_shell_line(&mut self, buffer: &Entity<EditorState>, cx: &mut Context<Self>) {
        let Some(run) = self.shell_run.as_ref() else { return };
        let target = match run.stops.get(run.current) {
            Some(stop @ (Stop::Error(_) | Stop::Match(..))) if !run.running() && !run.paused => {
                let loc = stop.location().expect("a location");
                self.buffers.iter().find(|b| b.path.as_deref() == Some(loc.path.as_path())).map(|b| (b.state.clone(), loc.line))
            }
            _ => None,
        };
        let found = match run.stops.get(run.current) {
            Some(Stop::Match(_, bytes)) => Some(bytes.clone()),
            _ => None,
        };
        let stale = run.gap.as_ref().is_some_and(|(s, _)| target.as_ref().is_none_or(|(t, _)| t != s));
        if stale {
            self.clear_shell_marks(cx);
        }
        let Some((state, line)) = target.filter(|(s, _)| s == buffer) else { return };
        let text = state.read(cx).text().to_string();
        let line_range = line_bytes(&text, line..line + 1);
        // An error marks its line; a match, just what matched.
        let marked = match found {
            Some(b) => {
                let style = HighlightStyle { background_color: Some(cx.theme().warning.opacity(0.3)), ..Default::default() };
                let range = (line_range.start + b.start).min(text.len())..(line_range.start + b.end).min(text.len());
                vec![TextDecoration::new(range, style)]
            }
            None => vec![TextDecoration::new(line_range, HighlightStyle { background_color: Some(cx.theme().danger.opacity(0.14)), ..Default::default() })],
        };
        let run = self.shell_run.as_mut().expect("checked above");
        if run.gap.is_none() {
            let mark = state.update(cx, |st, cx| st.create_decorations_collection(vec![], cx));
            run.gap = Some((state.clone(), mark));
        }
        if let Some((_, mark)) = &run.gap {
            mark.set(marked, cx);
        }
    }

    /// The run's output (or the current error's message) in a panel above the
    /// command line, with a faint hint at the end.
    pub(super) fn render_shell_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let run = self.shell_run.as_ref().filter(|r| !r.paused)?;
        let rows = self.shell_rows(cx);
        let theme = cx.theme();
        let failed = matches!(run.exit, Some(Some(code)) if code != 0) || matches!(run.exit, Some(None));
        let label = run.exit_label();
        let progress = if run.running() { String::new() } else { format!("  {}/{}", run.current + 1, run.stops.len()) };
        let hint = match run.stops.get(run.current) {
            _ if run.running() => format!("$ {} · esc stop", run.command),
            Some(Stop::Error(_)) => format!("{label} · ⇥ next · ⇧⇥ back · ⏎ fix with Claude{progress}"),
            Some(Stop::Match(..)) => format!("{label} · ⇥ next · ⇧⇥ back · ⏎ ask Claude here · esc{progress}"),
            _ if run.current + 1 >= run.stops.len() => format!("{label} · ⇥ done · ⇧⇥ back{progress}"),
            _ => format!("{label} · ⇥ more · ⇧⇥ back{progress}"),
        };
        let searching = matches!(run.stops.first(), Some(Stop::Match(..)));
        let label_color = if run.running() || searching { theme.muted_foreground } else if failed || run.label.is_some() { theme.danger } else { theme.success };
        let title = if run.label.is_some() { hint.clone() } else { format!("$ {}  ·  {hint}", run.command) };
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(theme.border)
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size)
                .text_color(theme.muted_foreground)
                .children(rows.into_iter().map(|l| div().whitespace_nowrap().overflow_hidden().child(if l.is_empty() { " ".to_string() } else { l })))
                .child(div().pt_1().text_xs().text_color(label_color).child(title))
                .into_any_element(),
        )
    }

    pub(super) fn shell_next(&mut self, _: &ShellNext, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.shell_run.as_mut() else { return };
        if run.running() {
            return;
        }
        if run.current + 1 >= run.stops.len() {
            return self.finish_shell(window, cx);
        }
        run.current += 1;
        cx.notify();
    }

    pub(super) fn shell_close(&mut self, _: &ShellClose, window: &mut Window, cx: &mut Context<Self>) {
        match self.shell_run.as_ref() {
            Some(run) if run.running() => run.cancel.store(true, Ordering::SeqCst),
            _ => self.close_shell(window, cx),
        }
    }
}
