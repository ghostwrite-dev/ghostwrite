//! Debugging a program from the editor: `:break` on the cursor's line, `:debug
//! program`, then `:over`, `:step`, `:finish`, `:continue` and `:stop`. Those are
//! commands, so that they can be given keys of your own. When it stops, the cursor goes
//! there, and what's called what and the variables show in the side view, beside the
//! code (on top of a plugin's panel, if there's one), with what it printed: `space g`
//! goes there.
//! The debugger is whatever the config's `debugger` says, Xcode's by default.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt as _;
use ghostwrite_core::Range;
use ghostwrite_dap::{Debugger, Event, Frame, Variable};
use gpui::{AnyElement, AppContext as _, Context, Task, div, prelude::*};

use super::{Autoscroll, Editor};
use crate::theme::{pt, theme};

/// How much of what it printed is kept.
const MAX_OUTPUT: usize = 20_000;
/// Its height in the side view, over a panel.
const HEIGHT: f32 = 260.;

#[derive(Default)]
pub(super) struct DebugPane {
    /// By file, its real path, and zero-based line. They stay on their line numbers as the text changes.
    pub(super) breakpoints: HashMap<PathBuf, BTreeSet<usize>>,
    pub(super) session: Option<Session>,
}

pub(super) struct Session {
    debugger: Arc<Debugger>,
    /// Where it's stopped, if it is.
    pub(super) stopped: Option<Stopped>,
    pub(super) output: String,
    _task: Task<()>,
}

pub(super) struct Stopped {
    thread: i64,
    pub(super) reason: String,
    pub(super) frames: Vec<Frame>,
    pub(super) locals: Vec<Variable>,
}

/// As it's compiled into the program, whichever way it's reached.
fn real(path: &Path) -> PathBuf {
    ghostwrite_project::platform::real(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Editor {
    /// `:break`: a breakpoint on the cursor's line, or none if there was one.
    pub(super) fn toggle_breakpoint(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let tab = self.tab().ok_or("no file open")?;
        let path = real(tab.doc.path.as_deref().ok_or("save it first: a breakpoint is in a file")?);
        let line = tab.doc.buffer.line_of(tab.cursor());
        let lines = self.debug.breakpoints.entry(path.clone()).or_default();
        if !lines.remove(&line) {
            lines.insert(line);
        }
        let lines = lines.clone();
        if let Some(debugger) = self.debug.session.as_ref().map(|session| session.debugger.clone()) {
            cx.background_spawn(async move { ghostwrite_project::contained("setting a breakpoint", || debugger.set_breakpoints(&path, &lines)).ok() }).detach();
        }
        cx.notify();
        Ok(())
    }

    /// `:debug program args`: run it, stopping at the breakpoints.
    pub(super) fn start_debugging(&mut self, line: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let mut words = line.split_whitespace();
        let program = words.next().ok_or("debug what? :debug path/to/program")?;
        let program = ghostwrite_project::resolve(program, &self.project.root);
        if !program.is_file() {
            return Err(format!("there's no program at {}: build it first, with its debug info", program.display()));
        }
        let args: Vec<String> = words.map(str::to_string).collect();
        let (tell, mut events) = futures::channel::mpsc::unbounded();
        let debugger = Debugger::start(&crate::config::get().debugger, move |event| drop(tell.unbounded_send(event)))?;
        let debugger = Arc::new(debugger);
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this.update(cx, |this, cx| this.debugger_said(event, cx)).is_err() {
                    break;
                }
            }
        });
        self.debug.session = Some(Session { debugger: debugger.clone(), stopped: None, output: String::new(), _task: task });
        let (cwd, breakpoints) = (self.project.root.clone(), self.debug.breakpoints.clone());
        let launching = cx.background_spawn(async move {
            ghostwrite_project::contained("starting the debugger", || debugger.launch(&program, &args, &cwd, &breakpoints)).and_then(|started| started)
        });
        cx.spawn(async move |this, cx| {
            if let Err(why) = launching.await {
                this.update(cx, |this, cx| {
                    this.debug.session = None;
                    this.project.say(format!("couldn't debug it: {why}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        self.project.say("running it in the debugger…");
        cx.notify();
        Ok(())
    }

    /// `:over`, `:step`, `:finish`, `:continue`: go on from where it's stopped.
    pub(super) fn debugger_go(&mut self, how: &'static str, cx: &mut Context<Self>) -> Result<(), String> {
        let session = self.debug.session.as_mut().ok_or("nothing's being debugged: :debug path/to/program")?;
        let stopped = session.stopped.take().ok_or("it's running: it can only go on once it's stopped")?;
        let debugger = session.debugger.clone();
        cx.background_spawn(async move { ghostwrite_project::contained("telling the debugger", || debugger.go(how, stopped.thread)).ok() }).detach();
        cx.notify();
        Ok(())
    }

    /// `:stop`: stop the program and the debugger.
    pub(super) fn stop_debugging(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.debug.session.take().ok_or("nothing's being debugged")?;
        self.project.say("stopped debugging");
        cx.notify();
        Ok(())
    }

    fn debugger_said(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(session) = self.debug.session.as_mut() else { return };
        match event {
            Event::Output(text) => {
                session.output.push_str(&text);
                if session.output.len() > MAX_OUTPUT {
                    let from = session.output.ceil_char_boundary(session.output.len() - MAX_OUTPUT);
                    session.output.drain(..from);
                }
            }
            Event::Continued => session.stopped = None,
            Event::Stopped { reason, thread } => {
                let debugger = session.debugger.clone();
                let looking = cx.background_spawn(async move {
                    ghostwrite_project::contained("looking at the program", || {
                        let frames = debugger.frames(thread)?;
                        let locals = frames.first().map_or(Ok(vec![]), |frame| debugger.locals(frame.id))?;
                        Ok::<_, String>((frames, locals))
                    })
                    .and_then(|found| found)
                });
                cx.spawn(async move |this, cx| {
                    let found = looking.await;
                    this.update(cx, |this, cx| this.stopped_at(thread, reason, found, cx)).ok();
                })
                .detach();
            }
            Event::Exited(code) => {
                let code = code.map_or(String::new(), |code| format!(" with {code}"));
                self.debug.session = None;
                self.project.say(format!("the program ended{code}"));
            }
            Event::Gone => {
                if self.debug.session.take().is_some() {
                    self.project.say("the debugger stopped");
                }
            }
        }
        cx.notify();
    }

    fn stopped_at(&mut self, thread: i64, reason: String, found: Result<(Vec<Frame>, Vec<Variable>), String>, cx: &mut Context<Self>) {
        let (frames, locals) = match found {
            Ok(found) => found,
            Err(why) => return self.project.say(format!("couldn't look at the program: {why}")),
        };
        // To the innermost place that's a file here: a library's has none to show.
        let place = frames.iter().find_map(|frame| Some((frame.path.clone().filter(|path| path.is_file())?, frame.line?)));
        let Some(session) = self.debug.session.as_mut() else { return };
        session.stopped = Some(Stopped { thread, reason, frames, locals });
        if let Some((path, line)) = place
            && let Some(ix) = self.project.open(&path)
        {
            self.activate(ix, cx);
            if let Some(tab) = self.project.tabs.get_mut(ix) {
                let at = tab.doc.buffer.line_start(line.min(tab.doc.buffer.last_line()));
                tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
            }
        }
        cx.notify();
    }

    /// The line it's stopped on, if that's in the file of `path`.
    pub(super) fn stopped_line(&self, path: &Path) -> Option<usize> {
        let stopped = self.debug.session.as_ref()?.stopped.as_ref()?;
        let path = real(path);
        stopped
            .frames
            .iter()
            .find(|frame| frame.path.is_some() && frame.line.is_some())
            .filter(|frame| frame.path.as_deref().map(real).as_deref() == Some(&path))?
            .line
    }

    /// Whether there's a breakpoint at `line` of the file of `path`.
    pub(super) fn breakpoints_in(&self, path: &Path) -> Option<&BTreeSet<usize>> {
        self.debug.breakpoints.get(&real(path))
    }

    /// In the side view, while something's being debugged: where it's stopped, the
    /// variables, and what it printed, one under another. `alone`, it has the whole
    /// height; over a plugin's panel, some of it.
    pub(super) fn render_debug(&self, alone: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let session = self.debug.session.as_ref()?;
        let theme = theme(cx);
        let section =
            |title: String| div().min_w_0().overflow_hidden().px_3().flex().flex_col().child(div().text_xs().text_color(theme.muted_foreground).child(title));
        let row = |text: String| div().whitespace_nowrap().overflow_hidden().child(text);
        let (place, values) = match &session.stopped {
            Some(stopped) => (
                section(format!("stopped: {}", stopped.reason)).children(stopped.frames.iter().map(|frame| {
                    let name = frame
                        .path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map_or(String::new(), |file| format!("  {}:{}", file.to_string_lossy(), frame.line.map_or(0, |line| line + 1)));
                    row(format!("{}{name}", frame.name))
                })),
                section("variables".to_string()).children(stopped.locals.iter().map(|variable| row(format!("{} = {}", variable.name, variable.value)))),
            ),
            None => (section("running…".to_string()), section("variables".to_string())),
        };
        let printed = session.output.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().map(|line| row(line.to_string()));
        Some(
            div()
                .map(|d| if alone { d.flex_1().min_h_0() } else { d.flex_none().h(pt(HEIGHT)).border_b_1().border_color(theme.border) })
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap_2()
                .py_2()
                .text_color(theme.foreground)
                .font_family(theme.mono_font.clone())
                .text_size(theme.mono_size * 0.9)
                .debug_selector(|| "debug-pane".to_string())
                .child(place)
                .child(values)
                .child(section("it printed".to_string()).children(printed))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
impl Editor {
    /// Something being debugged by `adapter` (which need only answer `initialize`),
    /// stopped where `frames` say, with `locals`: what the side view shows of it, for the tests.
    pub(super) fn pretend_debugging(&mut self, adapter: &[String], frames: Vec<Frame>, locals: Vec<Variable>, cx: &mut Context<Self>) -> Result<(), String> {
        let debugger = Arc::new(Debugger::start(adapter, |_| {})?);
        let stopped = Some(Stopped { thread: 1, reason: "breakpoint".into(), frames, locals });
        self.debug.session = Some(Session { debugger, stopped, output: "it ran\n".into(), _task: Task::ready(()) });
        cx.notify();
        Ok(())
    }
}
